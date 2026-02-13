//! Mock adapters for testing.
//!
//! This module provides mock harnesses for intercepting and responding to effects in tests:
//!
//! - [`MockHttpHarness`]: Intercepts `http.request` effects and provides mock responses
//! - [`MockLlmHarness`]: Intercepts `llm.generate` effects and provides synthetic responses

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_air_exec::{Value as ExprValue, ValueKey};
use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::{HeaderMap, HttpRequestParams, LlmGenerateParams, LlmRuntimeArgs};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::Kernel;
use aos_store::Store;
use sha2::{Digest, Sha256};
use tracing::debug;

// ---------------------------------------------------------------------------
// MockHttpHarness: HTTP effect interception
// ---------------------------------------------------------------------------

const MOCK_HTTP_ADAPTER_ID: &str = "http.mock";

/// Context for an HTTP request intercepted by the mock harness.
#[derive(Debug, Clone)]
pub struct HttpRequestContext {
    pub intent: EffectIntent,
    pub params: HttpRequestParams,
}

/// Mock HTTP response for testing.
#[derive(Debug, Clone)]
pub struct MockHttpResponse {
    pub status: i64,
    pub headers: HeaderMap,
    pub body: String,
}

impl MockHttpResponse {
    /// Create a JSON response with the given status and body.
    pub fn json(status: i64, body: impl Into<String>) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type".into(),
            "application/json; charset=utf-8".into(),
        );
        Self {
            status,
            headers,
            body: body.into(),
        }
    }

    /// Create a plain text response.
    pub fn text(status: i64, body: impl Into<String>) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert("content-type".into(), "text/plain; charset=utf-8".into());
        Self {
            status,
            headers,
            body: body.into(),
        }
    }
}

/// Mock HTTP harness for testing HTTP effect flows.
///
/// This harness intercepts `http.request` effects and allows tests to
/// provide mock responses.
pub struct MockHttpHarness;

impl MockHttpHarness {
    pub fn new() -> Self {
        Self
    }

    /// Collect all pending HTTP requests from the kernel.
    pub fn collect_requests<S: Store + 'static>(
        &mut self,
        kernel: &mut Kernel<S>,
    ) -> Result<Vec<HttpRequestContext>> {
        let mut out = Vec::new();
        loop {
            let intents = kernel.drain_effects()?;
            if intents.is_empty() {
                break;
            }
            for intent in intents {
                match intent.kind.as_str() {
                    EffectKind::HTTP_REQUEST => {
                        let params: HttpRequestParams = serde_cbor::from_slice(&intent.params_cbor)
                            .context("decode http request params")?;
                        out.push(HttpRequestContext { intent, params });
                    }
                    other => {
                        return Err(anyhow!("unexpected effect kind {other}"));
                    }
                }
            }
        }
        Ok(out)
    }

    /// Respond to an HTTP request with a mock response.
    pub fn respond_with<S: Store + 'static>(
        &self,
        kernel: &mut Kernel<S>,
        ctx: HttpRequestContext,
        response: MockHttpResponse,
    ) -> Result<()> {
        self.respond_with_body(kernel, None::<&S>, ctx, response)
    }

    /// Respond to an HTTP request, optionally storing the body in a store.
    pub fn respond_with_body<S: Store + 'static>(
        &self,
        kernel: &mut Kernel<S>,
        store: Option<&impl Store>,
        ctx: HttpRequestContext,
        response: MockHttpResponse,
    ) -> Result<()> {
        let receipt_value =
            build_http_receipt_value(response.status, &response.headers, response.body, store)?;
        let receipt = EffectReceipt {
            intent_hash: ctx.intent.intent_hash,
            adapter_id: MOCK_HTTP_ADAPTER_ID.into(),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt_value)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        };
        kernel.handle_receipt(receipt)?;
        kernel.tick_until_idle()?;
        Ok(())
    }
}

impl Default for MockHttpHarness {
    fn default() -> Self {
        Self::new()
    }
}

fn build_http_receipt_value(
    status: i64,
    headers: &HeaderMap,
    body: String,
    store: Option<&impl Store>,
) -> Result<ExprValue> {
    let mut record = indexmap::IndexMap::new();
    record.insert("status".into(), ExprValue::Int(status));
    record.insert("headers".into(), headers_to_value(&redact_headers(headers)));
    record.insert("body_preview".into(), ExprValue::Text(body.clone()));
    if let Some(store) = store {
        let hash = store
            .put_blob(body.as_bytes())
            .context("store http response body")?;
        record.insert("body_ref".into(), ExprValue::Text(hash.to_hex()));
    }
    let mut timings = indexmap::IndexMap::new();
    timings.insert("start_ns".into(), ExprValue::Nat(10));
    timings.insert("end_ns".into(), ExprValue::Nat(20));
    record.insert("timings".into(), ExprValue::Record(timings));
    record.insert(
        "adapter_id".into(),
        ExprValue::Text(MOCK_HTTP_ADAPTER_ID.into()),
    );
    Ok(ExprValue::Record(record))
}

fn headers_to_value(headers: &HeaderMap) -> ExprValue {
    let mut map = aos_air_exec::ValueMap::new();
    for (key, value) in headers {
        map.insert(ValueKey::Text(key.clone()), ExprValue::Text(value.clone()));
    }
    ExprValue::Map(map)
}

fn redact_headers(headers: &HeaderMap) -> HeaderMap {
    let mut redacted = HeaderMap::new();
    for (k, v) in headers {
        let lower = k.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "authorization" | "proxy-authorization" | "x-api-key" | "api-key"
        ) {
            redacted.insert(k.clone(), "<redacted>".into());
        } else {
            redacted.insert(k.clone(), v.clone());
        }
    }
    redacted
}

// ---------------------------------------------------------------------------
// MockLlmHarness: LLM effect interception
// ---------------------------------------------------------------------------

const MOCK_LLM_ADAPTER_ID: &str = "llm.mock";

/// Context for an LLM request intercepted by the mock harness.
#[derive(Debug, Clone)]
pub struct LlmRequestContext {
    pub intent: EffectIntent,
    pub params: LlmGenerateParams,
}

/// Mock LLM harness for testing LLM effect flows.
///
/// This harness intercepts `llm.generate` effects, validates parameters,
/// and generates synthetic responses for testing purposes.
pub struct MockLlmHarness<S: Store> {
    store: Arc<S>,
    expected_api_key: Option<String>,
}

impl<S: Store + 'static> MockLlmHarness<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            expected_api_key: None,
        }
    }

    pub fn with_expected_api_key(mut self, key: impl Into<String>) -> Self {
        self.expected_api_key = Some(key.into());
        self
    }

    /// Collect all pending LLM requests from the kernel.
    pub fn collect_requests(&mut self, kernel: &mut Kernel<S>) -> Result<Vec<LlmRequestContext>> {
        let mut out = Vec::new();
        loop {
            let intents = kernel.drain_effects()?;
            if intents.is_empty() {
                break;
            }
            for intent in intents {
                match intent.kind.as_str() {
                    EffectKind::LLM_GENERATE => {
                        let raw: serde_cbor::Value = serde_cbor::from_slice(&intent.params_cbor)
                            .context("decode llm.generate params value")?;
                        let params = llm_params_from_cbor(raw)?;
                        out.push(LlmRequestContext { intent, params });
                    }
                    other => {
                        return Err(anyhow!("unexpected effect kind {other}"));
                    }
                }
            }
        }
        Ok(out)
    }

    /// Respond to an LLM request with a synthetic response.
    pub fn respond_with(&self, kernel: &mut Kernel<S>, ctx: LlmRequestContext) -> Result<()> {
        if ctx.params.message_refs.is_empty() {
            return Err(anyhow!("llm.mock missing message_refs"));
        }
        let mut prompt_parts = Vec::with_capacity(ctx.params.message_refs.len());
        for reference in &ctx.params.message_refs {
            let prompt_hash = hash_from_ref(reference)?;
            let prompt_bytes = self
                .store
                .get_blob(prompt_hash)
                .context("load message blob for llm.generate")?;
            prompt_parts.push(message_text_from_bytes(&prompt_bytes)?);
        }
        let prompt_text = prompt_parts.join("\n");

        if let Some(api_key) = &ctx.params.api_key {
            let fingerprint = hash_key(api_key);
            debug!(
                "llm.mock using api_key (len={} bytes) fingerprint={}",
                api_key.len(),
                fingerprint
            );
            if let Some(expected) = &self.expected_api_key {
                if expected != api_key {
                    return Err(anyhow!(
                        "llm.mock api_key mismatch: expected {}, got {}",
                        hash_key(expected),
                        fingerprint
                    ));
                }
            }
        } else {
            debug!("llm.mock no api_key provided (likely placeholder)");
            if self.expected_api_key.is_some() {
                return Err(anyhow!("llm.mock missing api_key but expected one"));
            }
        }

        let summary_text = summarize(&prompt_text);
        let output_message = serde_json::json!({
            "role": "assistant",
            "content": [
                { "type": "text", "text": summary_text }
            ]
        });
        let output_bytes =
            serde_json::to_vec(&output_message).context("encode llm.generate output message")?;
        let output_hash = self
            .store
            .put_blob(&output_bytes)
            .context("store llm.generate output blob")?;
        let output_ref = HashRef::new(output_hash.to_hex())?;

        let receipt_value = build_receipt_value(&output_ref, &summary_text);
        let receipt = EffectReceipt {
            intent_hash: ctx.intent.intent_hash,
            adapter_id: MOCK_LLM_ADAPTER_ID.into(),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt_value)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        };
        kernel.handle_receipt(receipt)?;
        kernel.tick_until_idle()?;
        Ok(())
    }
}

fn summarize(prompt: &str) -> String {
    let prefix: String = prompt.chars().take(120).collect();
    let digest = Sha256::digest(prompt.as_bytes());
    let suffix = hex::encode(digest)[..8].to_string();
    format!("{prefix} …{suffix}")
}

fn llm_params_from_cbor(value: serde_cbor::Value) -> Result<LlmGenerateParams> {
    let map = match value {
        serde_cbor::Value::Map(m) => m,
        other => {
            return Err(anyhow!(
                "llm.generate params must be a map, got {:?}",
                other
            ));
        }
    };
    let text = |field: &str| -> Result<String> {
        match map.get(&serde_cbor::Value::Text(field.into())) {
            Some(serde_cbor::Value::Text(t)) => Ok(t.clone()),
            Some(other) => Err(anyhow!("field '{field}' must be text, got {:?}", other)),
            None => Err(anyhow!("field '{field}' missing from llm.generate params")),
        }
    };
    let opt_text = |field: &str| -> Result<Option<String>> {
        match map.get(&serde_cbor::Value::Text(field.into())) {
            Some(serde_cbor::Value::Text(t)) => Ok(Some(t.clone())),
            Some(serde_cbor::Value::Null) | None => Ok(None),
            Some(other) => Err(anyhow!("field '{field}' must be text or null, got {:?}", other)),
        }
    };
    let message_refs = match map.get(&serde_cbor::Value::Text("message_refs".into())) {
        Some(serde_cbor::Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                serde_cbor::Value::Text(t) => HashRef::new(t.clone()).context("parse hash ref"),
                other => Err(anyhow!(
                    "message_refs entries must be hash text, got {:?}",
                    other
                )),
            })
            .collect::<Result<Vec<_>>>()?,
        Some(other) => {
            return Err(anyhow!(
                "field 'message_refs' must be list<hash>, got {:?}",
                other
            ));
        }
        None => {
            return Err(anyhow!(
                "field 'message_refs' missing from llm.generate params"
            ));
        }
    };
    let runtime_map = match map.get(&serde_cbor::Value::Text("runtime".into())) {
        Some(serde_cbor::Value::Map(m)) => m,
        Some(other) => {
            return Err(anyhow!(
                "field 'runtime' must be record/map, got {:?}",
                other
            ));
        }
        None => {
            return Err(anyhow!("field 'runtime' missing from llm.generate params"));
        }
    };
    let runtime_opt_nat = |field: &str| -> Result<Option<u64>> {
        match runtime_map.get(&serde_cbor::Value::Text(field.into())) {
            Some(serde_cbor::Value::Integer(n)) if *n >= 0 => Ok(Some(*n as u64)),
            Some(serde_cbor::Value::Null) | None => Ok(None),
            Some(other) => Err(anyhow!(
                "runtime field '{field}' must be nat or null, got {:?}",
                other
            )),
        }
    };
    let runtime_opt_text = |field: &str| -> Result<Option<String>> {
        match runtime_map.get(&serde_cbor::Value::Text(field.into())) {
            Some(serde_cbor::Value::Text(t)) => Ok(Some(t.clone())),
            Some(serde_cbor::Value::Null) | None => Ok(None),
            Some(other) => Err(anyhow!(
                "runtime field '{field}' must be text or null, got {:?}",
                other
            )),
        }
    };
    let tool_refs = match runtime_map.get(&serde_cbor::Value::Text("tool_refs".into())) {
        Some(serde_cbor::Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                serde_cbor::Value::Text(t) => Ok(HashRef::new(t.clone())?),
                other => Err(anyhow!(
                    "tool_refs entries must be hash text, got {:?}",
                    other
                )),
            })
            .collect::<Result<Vec<_>>>()?,
        Some(serde_cbor::Value::Null) | None => Vec::new(),
        Some(other) => {
            return Err(anyhow!(
                "field 'tool_refs' must be list<hash> or null, got {:?}",
                other
            ));
        }
    };
    let api_key = decode_api_key(map.get(&serde_cbor::Value::Text("api_key".into())))?;

    Ok(LlmGenerateParams {
        correlation_id: opt_text("correlation_id")?,
        provider: text("provider")?,
        model: text("model")?,
        message_refs,
        runtime: LlmRuntimeArgs {
            temperature: runtime_opt_text("temperature")?,
            top_p: runtime_opt_text("top_p")?,
            max_tokens: runtime_opt_nat("max_tokens")?,
            tool_refs: if tool_refs.is_empty() {
                None
            } else {
                Some(tool_refs)
            },
            tool_choice: None,
            reasoning_effort: runtime_opt_text("reasoning_effort")?,
            stop_sequences: None,
            metadata: None,
            provider_options_ref: None,
            response_format_ref: None,
        },
        api_key,
    })
}

fn decode_api_key(value: Option<&serde_cbor::Value>) -> Result<Option<String>> {
    match value {
        None => Ok(None),
        Some(serde_cbor::Value::Null) => Ok(None),
        Some(serde_cbor::Value::Text(t)) => Ok(Some(t.clone())),
        Some(serde_cbor::Value::Map(m))
            if m.get(&serde_cbor::Value::Text("$tag".into()))
                == Some(&serde_cbor::Value::Text("secret".into())) =>
        {
            Ok(Some("demo-llm-api-key".into()))
        }
        Some(serde_cbor::Value::Map(m))
            if m.get(&serde_cbor::Value::Text("$tag".into()))
                == Some(&serde_cbor::Value::Text("literal".into())) =>
        {
            match m.get(&serde_cbor::Value::Text("$value".into())) {
                Some(serde_cbor::Value::Text(t)) => Ok(Some(t.clone())),
                Some(serde_cbor::Value::Bytes(b)) => Ok(Some(
                    std::str::from_utf8(b)
                        .map_err(|e| anyhow!("api_key bytes not utf8: {e}"))?
                        .to_string(),
                )),
                _ => Ok(None),
            }
        }
        Some(serde_cbor::Value::Map(m)) if m.len() == 1 => {
            if let Some((serde_cbor::Value::Text(tag), val)) = m.iter().next() {
                if tag == "literal" {
                    return match val {
                        serde_cbor::Value::Text(t) => Ok(Some(t.clone())),
                        serde_cbor::Value::Bytes(b) => Ok(Some(
                            std::str::from_utf8(b)
                                .map_err(|e| anyhow!("api_key bytes not utf8: {e}"))?
                                .to_string(),
                        )),
                        _ => Ok(None),
                    };
                }
            }
            Ok(None)
        }
        Some(other) => Err(anyhow!(
            "field 'api_key' must be text/secret/null, got {:?}",
            other
        )),
    }
}

fn hash_key(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    format!("sha256:{}", hex::encode(digest))
}

fn build_receipt_value(output_ref: &HashRef, summary: &str) -> ExprValue {
    let mut token_usage = indexmap::IndexMap::new();
    token_usage.insert("prompt".into(), ExprValue::Nat(120));
    token_usage.insert("completion".into(), ExprValue::Nat(42));
    token_usage.insert("total".into(), ExprValue::Nat(162));

    let mut finish_reason = indexmap::IndexMap::new();
    finish_reason.insert("reason".into(), ExprValue::Text("stop".into()));
    finish_reason.insert("raw".into(), ExprValue::Null);

    let mut record = indexmap::IndexMap::new();
    record.insert(
        "output_ref".into(),
        ExprValue::Text(output_ref.as_str().to_string()),
    );
    record.insert("raw_output_ref".into(), ExprValue::Null);
    record.insert("provider_response_id".into(), ExprValue::Null);
    record.insert("finish_reason".into(), ExprValue::Record(finish_reason));
    record.insert("token_usage".into(), ExprValue::Record(token_usage.clone()));
    record.insert("usage_details".into(), ExprValue::Null);
    record.insert("warnings_ref".into(), ExprValue::Null);
    record.insert("rate_limit_ref".into(), ExprValue::Null);
    record.insert("cost_cents".into(), ExprValue::Nat(0));
    record.insert(
        "summary_preview".into(),
        ExprValue::Text(summary.to_string()),
    );
    record.insert("tokens_prompt".into(), ExprValue::Nat(120));
    record.insert("tokens_completion".into(), ExprValue::Nat(42));
    record.insert("cost_millis".into(), ExprValue::Nat(250));
    record.insert(
        "provider_id".into(),
        ExprValue::Text(MOCK_LLM_ADAPTER_ID.into()),
    );
    ExprValue::Record(record)
}

fn hash_from_ref(reference: &HashRef) -> Result<Hash> {
    Hash::from_hex_str(reference.as_str()).context("parse hash from ref")
}

fn message_text_from_bytes(bytes: &[u8]) -> Result<String> {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
        match value {
            serde_json::Value::Object(_) => return message_text_from_value(&value),
            serde_json::Value::Array(items) => {
                let mut parts = Vec::with_capacity(items.len());
                for item in items {
                    parts.push(message_text_from_value(&item)?);
                }
                return Ok(parts.join("\n"));
            }
            _ => {}
        }
    }
    Ok(String::from_utf8(bytes.to_vec())?)
}

fn message_text_from_value(value: &serde_json::Value) -> Result<String> {
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("message blob must be an object"))?;
    let role = obj
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("message");
    let content = obj
        .get("content")
        .ok_or_else(|| anyhow!("message blob missing content"))?;
    let text = match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => {
            let mut buf = String::new();
            for part in parts {
                let part_obj = part
                    .as_object()
                    .ok_or_else(|| anyhow!("content parts must be objects"))?;
                let part_type = part_obj
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("text");
                match part_type {
                    "text" => {
                        if let Some(text) = part_obj.get("text").and_then(|v| v.as_str()) {
                            if !buf.is_empty() {
                                buf.push(' ');
                            }
                            buf.push_str(text);
                        }
                    }
                    "image" => {
                        let mime = part_obj
                            .get("mime")
                            .and_then(|v| v.as_str())
                            .unwrap_or("image");
                        if !buf.is_empty() {
                            buf.push(' ');
                        }
                        buf.push_str(&format!("[image:{mime}]"));
                    }
                    "audio" => {
                        let mime = part_obj
                            .get("mime")
                            .and_then(|v| v.as_str())
                            .unwrap_or("audio");
                        if !buf.is_empty() {
                            buf.push(' ');
                        }
                        buf.push_str(&format!("[audio:{mime}]"));
                    }
                    other => {
                        if !buf.is_empty() {
                            buf.push(' ');
                        }
                        buf.push_str(&format!("[{other}]"));
                    }
                }
            }
            buf
        }
        _ => return Err(anyhow!("message content must be string or list")),
    };
    Ok(format!("{role}: {text}"))
}
