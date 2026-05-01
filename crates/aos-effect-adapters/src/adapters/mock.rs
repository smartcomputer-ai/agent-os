//! Mock adapters for testing.
//!
//! This module provides mock harnesses for intercepting and responding to effects in tests:
//!
//! - [`MockHttpHarness`]: Intercepts `http.request` effects and provides mock responses
//! - [`MockLlmHarness`]: Intercepts `llm.generate` effects and provides synthetic responses

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_air_exec::Value as ExprValue;
use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::{
    HeaderMap, HttpRequestParams, HttpRequestReceipt, LlmCompactParams, LlmCompactReceipt,
    LlmCompactionArtifactKind, LlmGenerateParams, LlmWindowItem, LlmWindowItemKind, RequestTimings,
    TextOrSecretRef, TokenUsage,
};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus, effect_ops};
use aos_kernel::Kernel;
use aos_kernel::Store;
use sha2::{Digest, Sha256};
use tracing::debug;

// ---------------------------------------------------------------------------
// MockHttpHarness: HTTP effect interception
// ---------------------------------------------------------------------------

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
                match intent.effect.as_str() {
                    effect_ops::HTTP_REQUEST => {
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
        let store = kernel.store();
        self.respond_with_body(kernel, Some(store.as_ref()), ctx, response)
    }

    /// Respond to an HTTP request, optionally storing the body in a store.
    pub fn respond_with_body<S: Store + 'static>(
        &self,
        kernel: &mut Kernel<S>,
        store: Option<&impl Store>,
        ctx: HttpRequestContext,
        response: MockHttpResponse,
    ) -> Result<()> {
        let receipt_payload =
            build_http_receipt_payload(response.status, &response.headers, response.body, store)?;
        let receipt = EffectReceipt {
            intent_hash: ctx.intent.intent_hash,
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt_payload)?,
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

fn build_http_receipt_payload(
    status: i64,
    headers: &HeaderMap,
    body: String,
    store: Option<&impl Store>,
) -> Result<HttpRequestReceipt> {
    let body_ref = if let Some(store) = store {
        let hash = store
            .put_blob(body.as_bytes())
            .context("store http response body")?;
        Some(HashRef::new(hash.to_hex()).context("hash http response body")?)
    } else {
        None
    };
    Ok(HttpRequestReceipt {
        status: status as i32,
        headers: redact_headers(headers),
        body_ref,
        timings: RequestTimings {
            start_ns: 10,
            end_ns: 20,
        },
    })
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

const MOCK_LLM_ROUTE_ID: &str = "llm.mock";

/// Context for an LLM request intercepted by the mock harness.
#[derive(Debug, Clone)]
pub struct LlmRequestContext {
    pub intent: EffectIntent,
    pub params: LlmGenerateParams,
}

#[derive(Debug, Clone)]
pub struct LlmCompactRequestContext {
    pub intent: EffectIntent,
    pub params: LlmCompactParams,
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
                match intent.effect.as_str() {
                    effect_ops::LLM_GENERATE => {
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

    pub fn collect_compact_requests(
        &mut self,
        kernel: &mut Kernel<S>,
    ) -> Result<Vec<LlmCompactRequestContext>> {
        let mut out = Vec::new();
        loop {
            let intents = kernel.drain_effects()?;
            if intents.is_empty() {
                break;
            }
            for intent in intents {
                match intent.effect.as_str() {
                    effect_ops::LLM_COMPACT => {
                        let params: LlmCompactParams = serde_cbor::from_slice(&intent.params_cbor)
                            .context("decode llm.compact params")?;
                        out.push(LlmCompactRequestContext { intent, params });
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
        let message_refs = render_window_item_refs(
            &ctx.params.window_items,
            ctx.params.provider.as_str(),
            ctx.params.model.as_str(),
        )?;
        let mut prompt_parts = Vec::with_capacity(message_refs.len());
        for reference in &message_refs {
            let prompt_hash = hash_from_ref(reference)?;
            let prompt_bytes = self
                .store
                .get_blob(prompt_hash)
                .context("load message blob for llm.generate")?;
            prompt_parts.push(message_text_from_bytes(&prompt_bytes)?);
        }
        let prompt_text = prompt_parts.join("\n");

        match ctx.params.api_key.as_ref() {
            Some(TextOrSecretRef::Literal(api_key)) => {
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
            }
            Some(TextOrSecretRef::Secret(secret)) => {
                debug!(
                    "llm.mock unresolved secret ref api_key {}@{}",
                    secret.alias, secret.version
                );
                if self.expected_api_key.is_some() {
                    return Err(anyhow!("llm.mock received unresolved api_key secret ref"));
                }
            }
            None => {
                debug!("llm.mock no api_key provided (likely placeholder)");
                if self.expected_api_key.is_some() {
                    return Err(anyhow!("llm.mock missing api_key but expected one"));
                }
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
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt_value)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        };
        kernel.handle_receipt(receipt)?;
        kernel.tick_until_idle()?;
        Ok(())
    }

    pub fn respond_compact_with_summary(
        &self,
        kernel: &mut Kernel<S>,
        ctx: LlmCompactRequestContext,
    ) -> Result<()> {
        let message_refs = render_window_item_refs(
            &ctx.params.source_window_items,
            ctx.params.provider.as_str(),
            ctx.params.model.as_str(),
        )?;
        let mut prompt_parts = Vec::with_capacity(message_refs.len());
        for reference in &message_refs {
            let prompt_hash = hash_from_ref(reference)?;
            let prompt_bytes = self
                .store
                .get_blob(prompt_hash)
                .context("load message blob for llm.compact")?;
            prompt_parts.push(message_text_from_bytes(&prompt_bytes)?);
        }
        let summary_text = summarize(&prompt_parts.join("\n"));
        let summary_message = serde_json::json!({
            "role": "user",
            "content": format!("Compacted context summary: {summary_text}")
        });
        let summary_bytes =
            serde_json::to_vec(&summary_message).context("encode llm.compact summary message")?;
        let summary_hash = self
            .store
            .put_blob(&summary_bytes)
            .context("store llm.compact summary blob")?;
        let summary_ref = HashRef::new(summary_hash.to_hex())?;
        let summary_item = LlmWindowItem {
            item_id: format!("compact:{}:summary", ctx.params.operation_id),
            kind: LlmWindowItemKind::AosSummaryRef,
            ref_: summary_ref.clone(),
            lane: Some("Summary".into()),
            source_range: ctx.params.source_range.clone(),
            source_refs: message_refs,
            provider_compatibility: None,
            estimated_tokens: ctx.params.target_tokens,
            metadata: Default::default(),
        };
        let receipt = LlmCompactReceipt {
            operation_id: ctx.params.operation_id,
            artifact_kind: LlmCompactionArtifactKind::AosSummary,
            artifact_refs: vec![summary_ref],
            source_range: ctx.params.source_range,
            compacted_through: None,
            active_window_items: vec![summary_item],
            token_usage: Some(TokenUsage {
                prompt: 120,
                completion: 42,
                total: Some(162),
            }),
            provider_metadata_ref: None,
            warnings_ref: None,
            provider_id: MOCK_LLM_ROUTE_ID.into(),
        };
        let receipt = EffectReceipt {
            intent_hash: ctx.intent.intent_hash,
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt)?,
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
    format!("{prefix} ...{suffix}")
}

fn llm_params_from_cbor(value: serde_cbor::Value) -> Result<LlmGenerateParams> {
    serde_cbor::value::from_value(value).context("decode LlmGenerateParams")
}

fn render_window_item_refs(
    items: &[LlmWindowItem],
    provider: &str,
    model: &str,
) -> Result<Vec<HashRef>> {
    let mut refs = Vec::with_capacity(items.len());
    for item in items {
        let Some(ref_) = item.renderable_message_ref(provider, model) else {
            return Err(anyhow!(
                "window item '{}' is not renderable for provider '{}' model '{}'",
                item.item_id,
                provider,
                model
            ));
        };
        refs.push(ref_.clone());
    }
    if refs.is_empty() {
        return Err(anyhow!("llm.mock missing window_items"));
    }
    Ok(refs)
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
        ExprValue::Text(MOCK_LLM_ROUTE_ID.into()),
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
