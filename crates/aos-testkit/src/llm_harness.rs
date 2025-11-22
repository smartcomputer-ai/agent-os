use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_air_exec::Value as ExprValue;
use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::LlmGenerateParams;
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::Kernel;
use aos_store::Store;
use sha2::{Digest, Sha256};
use log::debug;

const MOCK_ADAPTER_ID: &str = "llm.mock";

#[derive(Debug, Clone)]
pub struct LlmRequestContext {
    pub intent: EffectIntent,
    pub params: LlmGenerateParams,
}

pub struct MockLlmHarness<S: Store> {
    store: Arc<S>,
}

impl<S: Store + 'static> MockLlmHarness<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
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
                        let params_value: ExprValue = serde_cbor::from_slice(&intent.params_cbor)
                            .context("decode llm.generate params value")?;
                        let params = llm_params_from_value(params_value)?;
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
        } else {
            debug!("llm.mock no api_key provided (likely placeholder)");
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

fn llm_params_from_value(value: ExprValue) -> Result<LlmGenerateParams> {
    let record = match value {
        ExprValue::Record(map) => map,
        other => return Err(anyhow!("llm.generate params must be a record, got {:?}", other)),
    };
    let api_key = record_optional_text(&record, "api_key")?;
    Ok(LlmGenerateParams {
        provider: record_text(&record, "provider")?,
        model: record_text(&record, "model")?,
        temperature: record_text(&record, "temperature")?,
        max_tokens: record_nat(&record, "max_tokens")?,
        input_ref: record_hash_ref(&record, "input_ref")?,
        tools: record_list_text(&record, "tools")?,
        api_key,
    })
}

fn record_text(record: &indexmap::IndexMap<String, ExprValue>, field: &str) -> Result<String> {
    match record.get(field) {
        Some(ExprValue::Text(text)) => Ok(text.clone()),
        Some(other) => Err(anyhow!("field '{field}' must be text, got {:?}", other)),
        None => Err(anyhow!("field '{field}' missing from llm.generate params")),
    }
}

fn record_optional_text(
    record: &indexmap::IndexMap<String, ExprValue>,
    field: &str,
) -> Result<Option<String>> {
    match record.get(field) {
        Some(ExprValue::Text(text)) => Ok(Some(text.clone())),
        Some(ExprValue::Record(rec)) => {
            if rec.get("$tag") == Some(&ExprValue::Text("secret".into())) {
                // secret ref made it through; resolve via env so the demo still shows injection
                if let Ok(val) = std::env::var("LLM_API_KEY") {
                    debug!(
                        "llm.mock resolving secret ref for {field} from LLM_API_KEY env (len={} bytes)",
                        val.len()
                    );
                    Ok(Some(val))
                } else {
                    debug!("llm.mock received unresolved secret ref for {field} and no env to resolve");
                    Ok(None)
                }
            } else {
                Err(anyhow!("field '{field}' must be text or null, got {:?}", rec))
            }
        }
        Some(ExprValue::Null) | Some(ExprValue::Unit) => Ok(None),
        None => Ok(None),
        Some(other) => Err(anyhow!("field '{field}' must be text or null, got {:?}", other)),
    }
}

fn record_nat(record: &indexmap::IndexMap<String, ExprValue>, field: &str) -> Result<u64> {
    match record.get(field) {
        Some(ExprValue::Nat(n)) => Ok(*n),
        Some(other) => Err(anyhow!("field '{field}' must be nat, got {:?}", other)),
        None => Err(anyhow!("field '{field}' missing from llm.generate params")),
    }
}

fn record_hash_ref(record: &indexmap::IndexMap<String, ExprValue>, field: &str) -> Result<HashRef> {
    match record.get(field) {
        Some(ExprValue::Text(text)) => HashRef::new(text.clone()).context("parse hash ref"),
        Some(other) => Err(anyhow!("field '{field}' must be text hash ref, got {:?}", other)),
        None => Err(anyhow!("field '{field}' missing from llm.generate params")),
    }
}

fn record_list_text(record: &indexmap::IndexMap<String, ExprValue>, field: &str) -> Result<Vec<String>> {
    match record.get(field) {
        Some(ExprValue::List(values)) => values
            .iter()
            .map(|value| match value {
                ExprValue::Text(text) => Ok(text.clone()),
                other => Err(anyhow!("tools entries must be text, got {:?}", other)),
            })
            .collect(),
        Some(ExprValue::Null) | None => Ok(Vec::new()),
        Some(other) => Err(anyhow!("field '{field}' must be a list, got {:?}", other)),
    }
}

fn summarize(prompt: &str) -> String {
    let prefix: String = prompt.chars().take(120).collect();
    let digest = Sha256::digest(prompt.as_bytes());
    let suffix = hex::encode(digest)[..8].to_string();
    format!("{prefix} …{suffix}")
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
