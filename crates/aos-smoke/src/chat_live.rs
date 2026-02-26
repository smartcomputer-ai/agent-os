use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, ensure};
use aos_cbor::Hash;
use aos_effects::ReceiptStatus;
use aos_effects::builtins::{LlmGenerateReceipt, LlmOutputEnvelope, LlmToolCallList};
use aos_kernel::journal::JournalRecord;
use aos_store::Store;
use clap::ValueEnum;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::example_host::{ExampleHost, HarnessConfig};

const WORKFLOW_NAME: &str = "demo/LiveChat@1";
const EVENT_SCHEMA: &str = "demo/LiveEvent@1";
const MODULE_CRATE: &str = "crates/aos-smoke/fixtures/21-chat-live/workflow";
const FIXTURE_ROOT: &str = "crates/aos-smoke/fixtures/21-chat-live";

const OPENAI_KEY_ENV: &str = "OPENAI_API_KEY";
const OPENAI_MODEL_ENV: &str = "OPENAI_LIVE_MODEL";
const ANTHROPIC_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const ANTHROPIC_MODEL_ENV: &str = "ANTHROPIC_LIVE_MODEL";

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LiveProvider {
    Openai,
    Anthropic,
}

#[derive(Debug, Clone)]
struct ProviderChoice {
    provider_id: &'static str,
    api_key_alias: &'static str,
    model: String,
}

#[derive(Debug, Deserialize)]
struct LiveState {
    outputs: Vec<RunOutput>,
}

#[derive(Debug, Deserialize)]
struct RunOutput {
    request_id: u64,
    output_ref: String,
}

pub fn run(provider: LiveProvider, model_override: Option<String>) -> Result<()> {
    let choice = resolve_provider(provider, model_override)?;
    let fixture_root = crate::workspace_root().join(FIXTURE_ROOT);
    let assets_root = fixture_root.join("air");

    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root: &fixture_root,
        assets_root: Some(&assets_root),
        workflow_name: WORKFLOW_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_CRATE,
    })?;

    let tool_ref = register_tool_blob(&host)?;
    let tool_refs = vec![tool_ref];

    println!(
        "→ Chat Live smoke (provider={} model={})",
        choice.provider_id, choice.model
    );

    let mut request_id = 1_u64;
    let initial_user_prompt = "Use tools before answering. Call echo_payload with {\"value\":\"alpha\"} and sum_pair with {\"a\":7,\"b\":8}. Then give one short sentence.";
    println!("   turn 1 user: {}", preview(initial_user_prompt));
    let mut history = vec![json!({
        "role": "user",
        "content": initial_user_prompt
    })];

    let mut total_tool_calls = 0_usize;
    let mut final_answer: Option<String> = None;

    for round in 0..6 {
        let envelope = dispatch_run(
            &mut host,
            &choice,
            request_id,
            &history,
            Some(tool_refs.clone()),
            Some(if round == 0 { "Required" } else { "Auto" }),
        )?;

        if let Some(tool_calls_ref) = envelope.tool_calls_ref.as_ref() {
            let calls = load_tool_calls(&host, tool_calls_ref.as_str())?;
            ensure!(!calls.is_empty(), "expected non-empty tool call list");
            total_tool_calls += calls.len();
            println!(
                "   turn 1 assistant: requested {} tool call(s)",
                calls.len()
            );

            history.push(json!({
                "role":"assistant",
                "content": calls.iter().map(|call| {
                    json!({
                        "type":"tool_call",
                        "id": call.call_id,
                        "name": call.tool_name,
                        "arguments": load_json_blob(host.store().as_ref(), call.arguments_ref.as_str()).unwrap_or_else(|_| json!({}))
                    })
                }).collect::<Vec<_>>()
            }));

            for (idx, call) in calls.iter().enumerate() {
                let args = load_json_blob(host.store().as_ref(), call.arguments_ref.as_str())?;
                let output = execute_local_tool(&call.tool_name, &args)?;
                println!(
                    "      tool {}: {} args={} result={}",
                    idx + 1,
                    call.tool_name,
                    preview(&args.to_string()),
                    preview(&output.to_string())
                );
                history.push(json!({
                    "type":"function_call_output",
                    "call_id": call.call_id,
                    "output": output
                }));
            }

            request_id = request_id.saturating_add(1);
            continue;
        }

        let text = envelope.assistant_text.unwrap_or_default();
        if !text.trim().is_empty() {
            println!("   turn 1 assistant: {}", preview(&text));
            final_answer = Some(text.clone());
            history.push(json!({"role":"assistant","content":text}));
            break;
        }

        let retry_prompt = "Return a plain-text answer now and do not call tools.";
        println!("   turn 1 user (clarify): {}", preview(retry_prompt));
        history.push(json!({
            "role":"user",
            "content":retry_prompt
        }));
        request_id = request_id.saturating_add(1);
    }

    ensure!(
        total_tool_calls >= 2,
        "expected at least 2 tool calls, got {total_tool_calls}"
    );
    let _ =
        final_answer.ok_or_else(|| anyhow!("missing final assistant answer after tool flow"))?;

    let followup_prompt = "Follow-up: what was the numeric sum and echoed value?";
    println!("   turn 2 user: {}", preview(followup_prompt));
    history.push(json!({
        "role":"user",
        "content":followup_prompt
    }));
    request_id = request_id.saturating_add(1);
    let followup = dispatch_run(
        &mut host,
        &choice,
        request_id,
        &history,
        Some(tool_refs),
        Some("Auto"),
    )?;
    let followup_text = followup.assistant_text.unwrap_or_default();
    ensure!(
        !followup_text.trim().is_empty(),
        "expected non-empty follow-up response"
    );
    println!("   turn 2 assistant: {}", preview(&followup_text));

    host.finish()?.verify_replay()?;
    println!("   live smoke: OK");
    Ok(())
}

fn dispatch_run(
    host: &mut ExampleHost,
    choice: &ProviderChoice,
    request_id: u64,
    history: &[Value],
    tool_refs: Option<Vec<String>>,
    tool_choice: Option<&str>,
) -> Result<LlmOutputEnvelope> {
    let store = host.store();
    let history_bytes =
        serde_json::to_vec(&Value::Array(history.to_vec())).context("encode history")?;
    let history_hash = store
        .put_blob(&history_bytes)
        .context("store history blob")?;

    let tool_choice_value = tool_choice.map(|tag| json!({"$tag": tag}));
    host.send_event(&json!({
        "$tag": "RunRequested",
        "$value": {
            "request_id": request_id,
            "provider": choice.provider_id,
            "model": choice.model,
            "api_key_alias": choice.api_key_alias,
            "message_refs": [history_hash.to_hex()],
            "tool_refs": tool_refs,
            "tool_choice": tool_choice_value,
            "max_tokens": 768
        }
    }))?;

    run_live_cycles_until_idle(host)?;

    let state: LiveState = host.read_state()?;
    let output_ref = state
        .outputs
        .iter()
        .find(|out| out.request_id == request_id)
        .map(|out| out.output_ref.clone());

    let Some(output_ref) = output_ref else {
        print_live_diagnostics(host)?;
        return Err(anyhow!("missing output_ref for request_id={request_id}"));
    };

    let value = load_json_blob(store.as_ref(), &output_ref)?;
    serde_json::from_value(value).context("decode llm output envelope")
}

fn run_live_cycles_until_idle(host: &mut ExampleHost) -> Result<()> {
    const MAX_CYCLES: usize = 96;
    for _ in 0..MAX_CYCLES {
        let outcome = host.run_cycle_batch()?;
        if outcome.effects_dispatched == 0
            && outcome.receipts_applied == 0
            && outcome.final_drain.idle
        {
            return Ok(());
        }
    }
    Err(anyhow!(
        "live smoke did not reach idle after {MAX_CYCLES} cycles"
    ))
}

fn resolve_provider(
    provider: LiveProvider,
    model_override: Option<String>,
) -> Result<ProviderChoice> {
    match provider {
        LiveProvider::Openai => {
            require_secret_binding(OPENAI_KEY_ENV)?;
            let model = model_override.unwrap_or_else(|| {
                env_or_dotenv_var(OPENAI_MODEL_ENV).unwrap_or_else(|| "gpt-5.2".to_string())
            });
            Ok(ProviderChoice {
                provider_id: "openai-responses",
                api_key_alias: "llm/openai_api",
                model,
            })
        }
        LiveProvider::Anthropic => {
            require_secret_binding(ANTHROPIC_KEY_ENV)?;
            let model = model_override.unwrap_or_else(|| {
                env_or_dotenv_var(ANTHROPIC_MODEL_ENV)
                    .unwrap_or_else(|| "claude-sonnet-4-5".to_string())
            });
            Ok(ProviderChoice {
                provider_id: "anthropic",
                api_key_alias: "llm/anthropic_api",
                model,
            })
        }
    }
}

fn require_secret_binding(env_key: &str) -> Result<()> {
    if env_or_dotenv_var(env_key).is_some() {
        return Ok(());
    }
    Err(anyhow!(
        "missing {} (env or .env); required for secret binding env:{}",
        env_key,
        env_key
    ))
}

fn register_tool_blob(host: &ExampleHost) -> Result<String> {
    let tools = json!({
        "tools": [
            {
                "name": "echo_payload",
                "description": "Echo a payload string",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "value": { "type": "string" }
                    },
                    "required": ["value"],
                    "additionalProperties": false
                }
            },
            {
                "name": "sum_pair",
                "description": "Return sum of two integers",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "integer" },
                        "b": { "type": "integer" }
                    },
                    "required": ["a", "b"],
                    "additionalProperties": false
                }
            }
        ]
    });

    let bytes = serde_json::to_vec(&tools).context("encode tool schema blob")?;
    let hash = host
        .store()
        .put_blob(&bytes)
        .context("store tool schema blob")?;
    Ok(hash.to_hex())
}

fn load_tool_calls(host: &ExampleHost, tool_calls_ref: &str) -> Result<LlmToolCallList> {
    let value = load_json_blob(host.store().as_ref(), tool_calls_ref)?;
    serde_json::from_value(value).context("decode tool call list")
}

fn execute_local_tool(name: &str, args: &Value) -> Result<Value> {
    match name {
        "echo_payload" => {
            let value = args
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            ensure!(!value.is_empty(), "echo_payload requires non-empty `value`");
            Ok(json!({
                "ok": true,
                "tool": "echo_payload",
                "echoed": value
            }))
        }
        "sum_pair" => {
            let a = as_i64(args.get("a"));
            let b = as_i64(args.get("b"));
            Ok(json!({
                "ok": true,
                "tool": "sum_pair",
                "sum": a + b
            }))
        }
        other => Err(anyhow!("unexpected tool `{other}`")),
    }
}

fn as_i64(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_u64().map(|v| v as i64))
            .or_else(|| n.as_f64().map(|v| v as i64))
            .unwrap_or(0),
        _ => 0,
    }
}

fn load_json_blob(store: &aos_store::FsStore, reference: &str) -> Result<Value> {
    let hash =
        Hash::from_hex_str(reference).with_context(|| format!("invalid blob ref {reference}"))?;
    let bytes = store
        .get_blob(hash)
        .with_context(|| format!("load blob {reference}"))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decode blob json {reference}"))
}

fn load_text_blob(store: &aos_store::FsStore, reference: &str) -> Result<String> {
    let hash =
        Hash::from_hex_str(reference).with_context(|| format!("invalid blob ref {reference}"))?;
    let bytes = store
        .get_blob(hash)
        .with_context(|| format!("load blob {reference}"))?;
    String::from_utf8(bytes).with_context(|| format!("decode utf8 blob {reference}"))
}

fn print_live_diagnostics(host: &mut ExampleHost) -> Result<()> {
    println!("   diagnostics: missing run result/output_ref");
    let entries = host.kernel_mut().dump_journal().context("dump journal")?;
    let store = host.store();

    for entry in &entries {
        let Ok(record) = serde_cbor::from_slice::<JournalRecord>(&entry.payload) else {
            continue;
        };
        if let JournalRecord::Custom(custom) = record
            && custom.tag == "workflow_error"
        {
            let message = String::from_utf8(custom.data)
                .unwrap_or_else(|_| "<unable to decode workflow_error payload>".to_string());
            println!("   diagnostics: workflow error {}", preview(&message));
            break;
        }
    }

    for entry in entries.iter().rev() {
        let Ok(record) = serde_cbor::from_slice::<JournalRecord>(&entry.payload) else {
            continue;
        };
        let JournalRecord::EffectReceipt(receipt) = record else {
            continue;
        };
        if !receipt.adapter_id.starts_with("host.llm.") {
            continue;
        }
        if receipt.status == ReceiptStatus::Ok {
            continue;
        }

        let message = if let Ok(payload) =
            serde_cbor::from_slice::<LlmGenerateReceipt>(&receipt.payload_cbor)
        {
            load_text_blob(store.as_ref(), payload.output_ref.as_str())
                .unwrap_or_else(|_| "<unable to decode provider error text>".to_string())
        } else {
            "<unable to decode llm receipt payload>".to_string()
        };
        println!(
            "   diagnostics: llm receipt status={:?} adapter={} error={}",
            receipt.status,
            receipt.adapter_id,
            preview(&message)
        );
        break;
    }

    Ok(())
}

fn preview(text: &str) -> String {
    const MAX: usize = 140;
    let trimmed = text.trim();
    if trimmed.len() <= MAX {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..MAX])
    }
}

fn env_or_dotenv_var(key: &str) -> Option<String> {
    if let Ok(value) = std::env::var(key) {
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

fn dotenv_candidates() -> Vec<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    vec![
        crate::workspace_root().join(".env"),
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
