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
    LlmToolChoice,
};
use aos_effects::{EffectIntent, EffectKind, ReceiptStatus};
use aos_host::adapters::llm::LlmAdapter;
use aos_host::adapters::traits::AsyncEffectAdapter;
use aos_host::config::{LlmAdapterConfig, LlmApiKind, ProviderConfig};
use aos_store::{MemStore, Store};
use serde_json::{Value, json};

const RUN_LIVE_ENV: &str = "RUN_LIVE_LLM_ADAPTER_TESTS";
const OPENAI_KEY_ENV: &str = "OPENAI_API_KEY";
const OPENAI_MODEL_ENV: &str = "OPENAI_LIVE_MODEL";
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
}

#[derive(Clone)]
struct ProviderRuntime {
    provider_id: String,
    api_kind: LlmApiKind,
    api_key: String,
    model: String,
    base_url: String,
    supports_response_format: bool,
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
            })
        })
        .collect()
}

fn build_intent(params: &LlmGenerateParams) -> EffectIntent {
    let params_cbor = serde_cbor::to_vec(params).expect("encode params");
    EffectIntent::from_raw_params(EffectKind::llm_generate(), "cap", params_cbor, [0u8; 32])
        .expect("build intent")
}

fn hash_from_ref(reference: &HashRef) -> Hash {
    Hash::from_hex_str(reference.as_str()).expect("valid hash ref")
}

fn store_json(store: &MemStore, value: &Value) -> HashRef {
    let bytes = serde_json::to_vec(value).expect("encode json blob");
    let hash = store.put_blob(&bytes).expect("store json blob");
    HashRef::new(hash.to_hex()).expect("hash ref")
}

fn load_json(store: &MemStore, reference: &HashRef) -> Value {
    let bytes = store
        .get_blob(hash_from_ref(reference))
        .expect("load referenced blob");
    serde_json::from_slice(&bytes).expect("decode json blob")
}

fn default_runtime() -> LlmRuntimeArgs {
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

fn make_adapter(store: Arc<MemStore>, case: &ProviderRuntime) -> LlmAdapter<MemStore> {
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

fn decode_receipt_payload(receipt: &aos_effects::EffectReceipt) -> LlmGenerateReceipt {
    serde_cbor::from_slice(&receipt.payload_cbor).expect("decode llm receipt")
}

fn error_text_from_receipt(store: &MemStore, receipt: &aos_effects::EffectReceipt) -> String {
    let payload = decode_receipt_payload(receipt);
    load_text(store, &payload.output_ref)
}

fn decode_envelope(store: &MemStore, receipt: &LlmGenerateReceipt) -> LlmOutputEnvelope {
    let value = load_json(store, &receipt.output_ref);
    serde_json::from_value(value).expect("decode output envelope")
}

fn load_text(store: &MemStore, reference: &HashRef) -> String {
    let bytes = store
        .get_blob(hash_from_ref(reference))
        .expect("load referenced blob");
    String::from_utf8(bytes).expect("decode utf8 text blob")
}

fn decode_tool_calls(store: &MemStore, envelope: &LlmOutputEnvelope) -> LlmToolCallList {
    let reference = envelope
        .tool_calls_ref
        .as_ref()
        .expect("missing tool_calls_ref");
    let value = load_json(store, reference);
    serde_json::from_value(value).expect("decode tool call list")
}

fn require_live_matrix() -> Vec<ProviderRuntime> {
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

fn assert_ok_receipt(
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

async fn run_plain_completion(case: &ProviderRuntime) {
    let store = Arc::new(MemStore::new());
    let adapter = make_adapter(store.clone(), case);

    let message_ref = store_json(
        &store,
        &json!({"role":"user","content":"Reply with exactly: live_adapter_ok"}),
    );
    let params = LlmGenerateParams {
        provider: case.provider_id.clone(),
        model: case.model.clone(),
        message_refs: vec![message_ref],
        runtime: default_runtime(),
        api_key: Some(case.api_key.clone()),
    };

    let receipt = adapter
        .execute(&build_intent(&params))
        .await
        .expect("execute");
    assert_eq!(receipt.adapter_id, format!("host.llm.{}", case.provider_id));

    let payload = assert_ok_receipt(&store, case, "plain completion", &receipt);
    assert_eq!(payload.provider_id, case.provider_id);
    let envelope = decode_envelope(&store, &payload);
    let text = envelope.assistant_text.unwrap_or_default();
    assert!(
        !text.trim().is_empty(),
        "expected assistant text for {}",
        case.provider_id
    );
}

async fn run_required_tool_call(case: &ProviderRuntime) -> (Arc<MemStore>, String, String, Value) {
    let store = Arc::new(MemStore::new());
    let adapter = make_adapter(store.clone(), case);

    let message_ref = store_json(
        &store,
        &json!({"role":"user","content":"Call echo_payload exactly once with an object containing a non-empty string field named `value`."}),
    );
    let tool_ref = store_json(
        &store,
        &json!({
            "tools": [
                {
                    "name": "echo_payload",
                    "description": "Echo payload for adapter contract tests",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "value": { "type": "string" }
                        },
                        "required": ["value"],
                        "additionalProperties": false
                    }
                }
            ],
            "tool_choice": "required"
        }),
    );

    let mut runtime = default_runtime();
    runtime.tool_refs = Some(vec![tool_ref]);
    runtime.tool_choice = Some(LlmToolChoice::Required);
    let params = LlmGenerateParams {
        provider: case.provider_id.clone(),
        model: case.model.clone(),
        message_refs: vec![message_ref],
        runtime,
        api_key: Some(case.api_key.clone()),
    };

    let receipt = adapter
        .execute(&build_intent(&params))
        .await
        .expect("execute");
    let payload = assert_ok_receipt(&store, case, "required tool call", &receipt);
    let envelope = decode_envelope(&store, &payload);
    let calls = decode_tool_calls(&store, &envelope);
    assert!(
        !calls.is_empty(),
        "expected at least one tool call for {}",
        case.provider_id
    );

    let call = &calls[0];
    assert_eq!(call.tool_name, "echo_payload");
    let arguments = load_json(&store, &call.arguments_ref);
    let arg_value = arguments
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !arg_value.is_empty(),
        "expected non-empty `value` tool argument for {}",
        case.provider_id
    );
    (
        store,
        call.call_id.clone(),
        call.tool_name.clone(),
        arguments,
    )
}

async fn run_tool_result_roundtrip(case: &ProviderRuntime) {
    let (store, call_id, tool_name, arguments) = run_required_tool_call(case).await;
    let adapter = make_adapter(store.clone(), case);

    let roundtrip_messages_ref = store_json(
        &store,
        &json!([
            {
                "role": "user",
                "content": "You received the tool result. Reply with a brief plain-text answer."
            },
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_call",
                        "id": call_id,
                        "name": tool_name,
                        "arguments": arguments
                    }
                ]
            },
            {
                "type": "function_call_output",
                "call_id": call_id,
                "output": {
                    "ok": true,
                    "note": "tool_result_from_live_adapter_test"
                }
            }
        ]),
    );

    let params = LlmGenerateParams {
        provider: case.provider_id.clone(),
        model: case.model.clone(),
        message_refs: vec![roundtrip_messages_ref],
        runtime: default_runtime(),
        api_key: Some(case.api_key.clone()),
    };
    let receipt = adapter
        .execute(&build_intent(&params))
        .await
        .expect("execute");
    let payload = assert_ok_receipt(&store, case, "tool result roundtrip", &receipt);
    let envelope = decode_envelope(&store, &payload);
    let text = envelope.assistant_text.unwrap_or_default();
    assert!(
        !text.trim().is_empty(),
        "expected assistant text after tool-result roundtrip for {}",
        case.provider_id
    );
}

async fn run_runtime_refs_smoke(case: &ProviderRuntime) {
    let store = Arc::new(MemStore::new());
    let adapter = make_adapter(store.clone(), case);

    let message_ref = store_json(
        &store,
        &json!({"role":"user","content":"Reply with a short confirmation message."}),
    );

    let provider_options_ref = if case.provider_id == "anthropic" {
        Some(store_json(
            &store,
            &json!({
                "anthropic": {
                    "auto_cache": false
                }
            }),
        ))
    } else {
        Some(store_json(
            &store,
            &json!({
                "openai": {
                    "parallel_tool_calls": false
                }
            }),
        ))
    };

    let response_format_ref = if case.supports_response_format {
        Some(store_json(
            &store,
            &json!({
                "type": "json_object",
                "strict": false
            }),
        ))
    } else {
        None
    };

    let mut runtime = default_runtime();
    runtime.provider_options_ref = provider_options_ref;
    runtime.response_format_ref = response_format_ref;
    let params = LlmGenerateParams {
        provider: case.provider_id.clone(),
        model: case.model.clone(),
        message_refs: vec![message_ref],
        runtime,
        api_key: Some(case.api_key.clone()),
    };

    let mut receipt = adapter
        .execute(&build_intent(&params))
        .await
        .expect("execute");
    if receipt.status != ReceiptStatus::Ok {
        let error = error_text_from_receipt(&store, &receipt).to_ascii_lowercase();
        let response_format_unsupported = error.contains("unsupported response_format")
            || error.contains("response format")
                && (error.contains("unsupported") || error.contains("not supported"));
        if case.supports_response_format && response_format_unsupported {
            // Some models (for example gpt-5-mini) may not allow structured output formats.
            // Retry without response_format_ref so we still validate provider_options_ref path.
            let mut fallback = params.clone();
            fallback.runtime.response_format_ref = None;
            receipt = adapter
                .execute(&build_intent(&fallback))
                .await
                .expect("execute fallback without response_format_ref");
        }
    }

    let payload = assert_ok_receipt(&store, case, "runtime refs", &receipt);
    let envelope = decode_envelope(&store, &payload);
    let text = envelope.assistant_text.unwrap_or_default();
    assert!(
        !text.trim().is_empty(),
        "expected assistant text for runtime refs smoke on {}",
        case.provider_id
    );
}

async fn run_invalid_api_key(case: &ProviderRuntime) {
    let store = Arc::new(MemStore::new());
    let adapter = make_adapter(store.clone(), case);

    let message_ref = store_json(&store, &json!({"role":"user","content":"hello"}));
    let params = LlmGenerateParams {
        provider: case.provider_id.clone(),
        model: case.model.clone(),
        message_refs: vec![message_ref],
        runtime: default_runtime(),
        api_key: Some("invalid-live-adapter-key".into()),
    };

    let receipt = adapter
        .execute(&build_intent(&params))
        .await
        .expect("execute");
    assert_eq!(
        receipt.status,
        ReceiptStatus::Error,
        "expected error status for invalid key on {}",
        case.provider_id
    );
    let payload = decode_receipt_payload(&receipt);
    assert_eq!(payload.provider_id, case.provider_id);
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_LLM_ADAPTER_TESTS=1 and provider keys (OPENAI_API_KEY and/or ANTHROPIC_API_KEY)"]
async fn llm_adapter_live_plain_completion_matrix() {
    let matrix = require_live_matrix();
    if matrix.is_empty() {
        return;
    }
    for case in &matrix {
        run_plain_completion(case).await;
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_LLM_ADAPTER_TESTS=1 and provider keys (OPENAI_API_KEY and/or ANTHROPIC_API_KEY)"]
async fn llm_adapter_live_required_tool_call_matrix() {
    let matrix = require_live_matrix();
    if matrix.is_empty() {
        return;
    }
    for case in &matrix {
        run_required_tool_call(case).await;
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_LLM_ADAPTER_TESTS=1 and provider keys (OPENAI_API_KEY and/or ANTHROPIC_API_KEY)"]
async fn llm_adapter_live_tool_result_roundtrip_matrix() {
    let matrix = require_live_matrix();
    if matrix.is_empty() {
        return;
    }
    for case in &matrix {
        run_tool_result_roundtrip(case).await;
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_LLM_ADAPTER_TESTS=1 and provider keys (OPENAI_API_KEY and/or ANTHROPIC_API_KEY)"]
async fn llm_adapter_live_runtime_refs_matrix() {
    let matrix = require_live_matrix();
    if matrix.is_empty() {
        return;
    }
    for case in &matrix {
        run_runtime_refs_smoke(case).await;
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_LLM_ADAPTER_TESTS=1 and provider keys (OPENAI_API_KEY and/or ANTHROPIC_API_KEY)"]
async fn llm_adapter_live_invalid_api_key_matrix() {
    let matrix = require_live_matrix();
    if matrix.is_empty() {
        return;
    }
    for case in &matrix {
        run_invalid_api_key(case).await;
    }
}
