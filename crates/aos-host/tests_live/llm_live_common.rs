use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::{
    LlmGenerateParams, LlmGenerateReceipt, LlmOutputEnvelope, LlmRuntimeArgs, LlmToolCallList,
};
use aos_effects::{EffectIntent, EffectKind, ReceiptStatus};
use aos_host::adapters::llm::LlmAdapter;
use aos_host::config::{LlmAdapterConfig, LlmApiKind, ProviderConfig};
use aos_store::{MemStore, Store};
use serde_json::Value;

const RUN_LIVE_ENV: &str = "RUN_LIVE_LLM_ADAPTER_TESTS";
const OPENAI_KEY_ENV: &str = "OPENAI_API_KEY";
const OPENAI_MODEL_ENV: &str = "OPENAI_LIVE_MODEL";
const OPENAI_CODEX_MODEL_ENV: &str = "OPENAI_CODEX_LIVE_MODEL";
const OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";
const ANTHROPIC_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const ANTHROPIC_MODEL_ENV: &str = "ANTHROPIC_LIVE_MODEL";
const ANTHROPIC_BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";

#[derive(Clone, Copy)]
struct ProviderCase {
    provider_id: &'static str,
    api_kind: LlmApiKind,
    api_key_env: &'static str,
    model_env: &'static str,
    default_model: &'static str,
    base_url_env: &'static str,
    default_base_url: &'static str,
    supports_response_format: bool,
    strict_multi_tool: bool,
}

#[derive(Clone)]
pub(crate) struct ProviderRuntime {
    pub(crate) provider_id: String,
    pub(crate) api_kind: LlmApiKind,
    pub(crate) api_key: String,
    pub(crate) model: String,
    pub(crate) base_url: String,
    pub(crate) supports_response_format: bool,
    pub(crate) strict_multi_tool: bool,
}

fn provider_cases() -> Vec<ProviderCase> {
    vec![
        ProviderCase {
            provider_id: "openai-responses",
            api_kind: LlmApiKind::Responses,
            api_key_env: OPENAI_KEY_ENV,
            model_env: OPENAI_MODEL_ENV,
            default_model: "gpt-5-mini",
            base_url_env: OPENAI_BASE_URL_ENV,
            default_base_url: "https://api.openai.com/v1",
            supports_response_format: true,
            strict_multi_tool: false,
        },
        ProviderCase {
            provider_id: "openai-responses",
            api_kind: LlmApiKind::Responses,
            api_key_env: OPENAI_KEY_ENV,
            model_env: OPENAI_CODEX_MODEL_ENV,
            default_model: "gpt-5.2-codex",
            base_url_env: OPENAI_BASE_URL_ENV,
            default_base_url: "https://api.openai.com/v1",
            supports_response_format: true,
            strict_multi_tool: true,
        },
        ProviderCase {
            provider_id: "anthropic",
            api_kind: LlmApiKind::AnthropicMessages,
            api_key_env: ANTHROPIC_KEY_ENV,
            model_env: ANTHROPIC_MODEL_ENV,
            default_model: "claude-sonnet-4-5",
            base_url_env: ANTHROPIC_BASE_URL_ENV,
            default_base_url: "https://api.anthropic.com/v1",
            supports_response_format: false,
            strict_multi_tool: false,
        },
    ]
}

fn live_tests_enabled() -> bool {
    match env::var(RUN_LIVE_ENV) {
        Ok(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes"
        }
        Err(_) => false,
    }
}

fn dotenv_candidates() -> Vec<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    vec![
        manifest_dir.join("../../.env"),
        manifest_dir.join(".env"),
        PathBuf::from(".env"),
    ]
}

fn parse_dotenv_value(contents: &str, key: &str) -> Option<String> {
    for raw_line in contents.lines() {
        let mut line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(stripped) = line.strip_prefix("export ") {
            line = stripped.trim();
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != key {
            continue;
        }

        let value = value.trim();
        let unquoted = if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
            || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        if !unquoted.is_empty() {
            return Some(unquoted.to_string());
        }
    }
    None
}

fn env_or_dotenv_var(key: &str) -> Option<String> {
    if let Ok(value) = env::var(key) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    for path in dotenv_candidates() {
        let Ok(contents) = fs::read_to_string(path) else {
            continue;
        };
        if let Some(value) = parse_dotenv_value(&contents, key) {
            return Some(value);
        }
    }

    None
}

fn active_provider_matrix() -> Vec<ProviderRuntime> {
    provider_cases()
        .into_iter()
        .filter_map(|case| {
            let api_key = env_or_dotenv_var(case.api_key_env)?;
            let model =
                env_or_dotenv_var(case.model_env).unwrap_or_else(|| case.default_model.into());
            let base_url = env_or_dotenv_var(case.base_url_env)
                .unwrap_or_else(|| case.default_base_url.into());
            Some(ProviderRuntime {
                provider_id: case.provider_id.to_string(),
                api_kind: case.api_kind,
                api_key,
                model,
                base_url,
                supports_response_format: case.supports_response_format,
                strict_multi_tool: case.strict_multi_tool,
            })
        })
        .collect()
}

pub(crate) fn require_live_matrix() -> Vec<ProviderRuntime> {
    if !live_tests_enabled() {
        return Vec::new();
    }
    let matrix = active_provider_matrix();
    if matrix.is_empty() {
        eprintln!(
            "skipping live adapter tests: set {} and at least one provider key ({} or {})",
            RUN_LIVE_ENV, OPENAI_KEY_ENV, ANTHROPIC_KEY_ENV
        );
    }
    matrix
}

pub(crate) fn build_intent(params: &LlmGenerateParams) -> EffectIntent {
    let params_cbor = serde_cbor::to_vec(params).expect("encode params");
    EffectIntent::from_raw_params(EffectKind::llm_generate(), "cap", params_cbor, [0u8; 32])
        .expect("build intent")
}

fn hash_from_ref(reference: &HashRef) -> Hash {
    Hash::from_hex_str(reference.as_str()).expect("valid hash ref")
}

pub(crate) fn store_json(store: &MemStore, value: &Value) -> HashRef {
    let bytes = serde_json::to_vec(value).expect("encode json blob");
    let hash = store.put_blob(&bytes).expect("store json blob");
    HashRef::new(hash.to_hex()).expect("hash ref")
}

pub(crate) fn load_json(store: &MemStore, reference: &HashRef) -> Value {
    let bytes = store
        .get_blob(hash_from_ref(reference))
        .expect("load referenced blob");
    serde_json::from_slice(&bytes).expect("decode json blob")
}

pub(crate) fn load_text(store: &MemStore, reference: &HashRef) -> String {
    let bytes = store
        .get_blob(hash_from_ref(reference))
        .expect("load referenced blob");
    String::from_utf8(bytes).expect("decode utf8 text blob")
}

pub(crate) fn default_runtime() -> LlmRuntimeArgs {
    LlmRuntimeArgs {
        // Keep baseline runtime minimal to avoid provider/model-specific parameter rejections.
        temperature: None,
        top_p: None,
        max_tokens: Some(512),
        tool_refs: None,
        tool_choice: None,
        reasoning_effort: None,
        stop_sequences: None,
        metadata: None,
        provider_options_ref: None,
        response_format_ref: None,
    }
}

pub(crate) fn make_adapter(store: Arc<MemStore>, case: &ProviderRuntime) -> LlmAdapter<MemStore> {
    let mut providers = HashMap::new();
    providers.insert(
        case.provider_id.clone(),
        ProviderConfig {
            base_url: case.base_url.clone(),
            timeout: Duration::from_secs(120),
            api_kind: case.api_kind,
        },
    );
    let config = LlmAdapterConfig {
        providers,
        default_provider: case.provider_id.clone(),
    };
    LlmAdapter::new(store, config)
}

pub(crate) fn decode_receipt_payload(receipt: &aos_effects::EffectReceipt) -> LlmGenerateReceipt {
    serde_cbor::from_slice(&receipt.payload_cbor).expect("decode llm receipt")
}

pub(crate) fn error_text_from_receipt(
    store: &MemStore,
    receipt: &aos_effects::EffectReceipt,
) -> String {
    let payload = decode_receipt_payload(receipt);
    load_text(store, &payload.output_ref)
}

pub(crate) fn decode_envelope(store: &MemStore, receipt: &LlmGenerateReceipt) -> LlmOutputEnvelope {
    let value = load_json(store, &receipt.output_ref);
    serde_json::from_value(value).expect("decode output envelope")
}

pub(crate) fn decode_tool_calls(store: &MemStore, envelope: &LlmOutputEnvelope) -> LlmToolCallList {
    let reference = envelope
        .tool_calls_ref
        .as_ref()
        .expect("missing tool_calls_ref");
    let value = load_json(store, reference);
    serde_json::from_value(value).expect("decode tool call list")
}

pub(crate) fn assert_ok_receipt(
    store: &MemStore,
    case: &ProviderRuntime,
    scenario: &str,
    receipt: &aos_effects::EffectReceipt,
) -> LlmGenerateReceipt {
    let payload = decode_receipt_payload(receipt);
    if receipt.status != ReceiptStatus::Ok {
        let provider_error = load_text(store, &payload.output_ref);
        panic!(
            "{} failed for provider={} model={}: status={:?} error={}",
            scenario, case.provider_id, case.model, receipt.status, provider_error
        );
    }
    payload
}
