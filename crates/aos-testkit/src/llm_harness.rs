use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_air_exec::Value as ExprValue;
use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::LlmGenerateParams;
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::Kernel;
use aos_store::Store;
use log::debug;
use sha2::{Digest, Sha256};

const MOCK_ADAPTER_ID: &str = "llm.mock";

#[derive(Debug, Clone)]
pub struct LlmRequestContext {
    pub intent: EffectIntent,
    pub params: LlmGenerateParams,
}

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

    pub fn collect_requests(&mut self, kernel: &mut Kernel<S>) -> Result<Vec<LlmRequestContext>> {
        let mut out = Vec::new();
        loop {
            let intents = kernel.drain_effects();
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

    pub fn respond_with(&self, kernel: &mut Kernel<S>, ctx: LlmRequestContext) -> Result<()> {
        let prompt_hash = hash_from_ref(&ctx.params.input_ref)?;
        let prompt_bytes = self
            .store
            .get_blob(prompt_hash)
            .context("load prompt blob for llm.generate")?;
        let prompt_text = String::from_utf8(prompt_bytes)?;

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
        let output_hash = self
            .store
            .put_blob(summary_text.as_bytes())
            .context("store llm.generate output blob")?;
        let output_ref = HashRef::new(output_hash.to_hex())?;

        let receipt_value = build_receipt_value(&output_ref, &summary_text);
        let receipt = EffectReceipt {
            intent_hash: ctx.intent.intent_hash,
            adapter_id: MOCK_ADAPTER_ID.into(),
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
    let nat = |field: &str| -> Result<u64> {
        match map.get(&serde_cbor::Value::Text(field.into())) {
            Some(serde_cbor::Value::Integer(n)) if *n >= 0 => Ok(*n as u64),
            Some(other) => Err(anyhow!("field '{field}' must be nat, got {:?}", other)),
            None => Err(anyhow!("field '{field}' missing from llm.generate params")),
        }
    };
    let input_ref = match map.get(&serde_cbor::Value::Text("input_ref".into())) {
        Some(serde_cbor::Value::Text(t)) => HashRef::new(t.clone()).context("parse hash ref")?,
        Some(other) => {
            return Err(anyhow!(
                "field 'input_ref' must be text hash ref, got {:?}",
                other
            ));
        }
        None => {
            return Err(anyhow!(
                "field 'input_ref' missing from llm.generate params"
            ));
        }
    };
    let tools = match map.get(&serde_cbor::Value::Text("tools".into())) {
        Some(serde_cbor::Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                serde_cbor::Value::Text(t) => Ok(t.clone()),
                other => Err(anyhow!("tools entries must be text, got {:?}", other)),
            })
            .collect::<Result<Vec<_>>>()?,
        Some(serde_cbor::Value::Null) | None => Vec::new(),
        Some(other) => {
            return Err(anyhow!(
                "field 'tools' must be list<text> or null, got {:?}",
                other
            ));
        }
    };
    let api_key = decode_api_key(map.get(&serde_cbor::Value::Text("api_key".into())))?;

    Ok(LlmGenerateParams {
        provider: text("provider")?,
        model: text("model")?,
        temperature: text("temperature")?,
        max_tokens: nat("max_tokens")?,
        input_ref,
        tools,
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

    let mut record = indexmap::IndexMap::new();
    record.insert(
        "output_ref".into(),
        ExprValue::Text(output_ref.as_str().to_string()),
    );
    record.insert(
        "summary_preview".into(),
        ExprValue::Text(summary.to_string()),
    );
    record.insert("token_usage".into(), ExprValue::Record(token_usage.clone()));
    record.insert("tokens_prompt".into(), ExprValue::Nat(120));
    record.insert("tokens_completion".into(), ExprValue::Nat(42));
    record.insert("cost_millis".into(), ExprValue::Nat(250));
    record.insert(
        "provider_id".into(),
        ExprValue::Text(MOCK_ADAPTER_ID.into()),
    );
    ExprValue::Record(record)
}

fn hash_from_ref(reference: &HashRef) -> Result<Hash> {
    Hash::from_hex_str(reference.as_str()).context("parse hash from ref")
}
