use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, ensure};
use aos_agent::{
    LlmToolCallList, SessionConfig, SessionId, SessionIngress, SessionIngressKind, SessionState,
    ToolAvailabilityRule, ToolExecutor, ToolParallelismHint, ToolSpec, WorkspaceApplyMode,
    WorkspaceBinding, WorkspaceSnapshot, WorkspaceSnapshotReady,
};
use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::{
    LlmGenerateParams, LlmGenerateReceipt, LlmOutputEnvelope, LlmRuntimeArgs, LlmToolChoice,
};
use aos_effects::{EffectIntent, EffectKind, ReceiptStatus};
use aos_host::adapters::llm::LlmAdapter;
use aos_host::adapters::traits::AsyncEffectAdapter;
use aos_host::config::{HostConfig, LlmAdapterConfig, LlmApiKind, ProviderConfig};
use aos_store::Store;
use aos_sys::{WorkspaceCommit, WorkspaceCommitMeta, WorkspaceEntry, WorkspaceTree};
use clap::ValueEnum;
use serde_json::{Value, json};
use tokio::runtime::Builder;
use walkdir::WalkDir;

use crate::example_host::{ExampleHost, HarnessConfig};

const WORKFLOW_NAME: &str = "aos.agent/SessionWorkflow@1";
const EVENT_SCHEMA: &str = "aos.agent/SessionIngress@1";
const FIXTURE_ROOT: &str = "crates/aos-smoke/fixtures/22-agent-live";
const SDK_AIR_ROOT: &str = "crates/aos-agent/air";
const SDK_WASM_PACKAGE: &str = "aos-agent";
const SDK_WASM_BIN: &str = "session_workflow";
const WORKSPACE_COMMIT_SCHEMA: &str = "sys/WorkspaceCommit@1";
const AGENT_WORKSPACE_NAME: &str = "agent-live";
const AGENT_WORKSPACE_DIR: &str = "agent-ws";
const DEFAULT_PROMPT_PACK: &str = "default";
const SEARCH_TOOL_NAME: &str = "search_step";
const SEARCH_TOOL_PROFILE: &str = "search-live";

const SESSION_ID: &str = "22222222-2222-2222-2222-222222222222";

const OPENAI_KEY_ENV: &str = "OPENAI_API_KEY";
const OPENAI_MODEL_ENV: &str = "OPENAI_LIVE_MODEL";
const OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";

const ANTHROPIC_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const ANTHROPIC_MODEL_ENV: &str = "ANTHROPIC_LIVE_MODEL";
const ANTHROPIC_BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LiveProvider {
    Openai,
    Anthropic,
}

#[derive(Debug, Clone)]
struct ProviderRuntime {
    provider_id: String,
    api_kind: LlmApiKind,
    api_key: String,
    model: String,
    base_url: String,
}

#[derive(Debug, Clone)]
struct SearchWorld {
    start_cursor: String,
    first_cursor: String,
    second_cursor: String,
    target_cursor: String,
    clue_one: String,
    clue_two: String,
    clue_three: String,
    target_value: String,
}

#[derive(Debug, Default)]
struct SearchStats {
    tool_rounds: usize,
    tool_calls: usize,
}

#[derive(Debug, Clone)]
enum SearchPhase {
    Looking,
    NeedPrimaryAnswer,
    NeedFollowUp,
    Done,
}

#[derive(Debug, Clone, Default)]
struct StepContext {
    correlation_id: Option<String>,
    message_refs: Vec<String>,
    temperature: Option<String>,
    top_p: Option<String>,
    tool_refs: Option<Vec<String>>,
    tool_choice: Option<LlmToolChoice>,
    stop_sequences: Option<Vec<String>>,
    metadata: Option<indexmap::IndexMap<String, String>>,
    provider_options_ref: Option<String>,
    response_format_ref: Option<String>,
    api_key: Option<String>,
}

pub fn run(provider: LiveProvider, model_override: Option<String>) -> Result<()> {
    let provider = resolve_provider(provider, model_override)?;
    let fixture_root = crate::workspace_root().join(FIXTURE_ROOT);
    let assets_root = fixture_root.join("air");
    let sdk_air_root = crate::workspace_root().join(SDK_AIR_ROOT);
    let import_roots = vec![sdk_air_root];
    let mut host = ExampleHost::prepare_with_imports_host_config_and_module_bin(
        HarnessConfig {
            example_root: &fixture_root,
            assets_root: Some(&assets_root),
            workflow_name: WORKFLOW_NAME,
            event_schema: EVENT_SCHEMA,
            module_crate: "",
        },
        &import_roots,
        Some(HostConfig {
            llm: None,
            ..HostConfig::default()
        }),
        SDK_WASM_PACKAGE,
        SDK_WASM_BIN,
    )?;

    let adapter = make_adapter(host.store(), &provider);
    let runtime = Builder::new_current_thread().enable_all().build()?;

    let world = SearchWorld::new();

    println!(
        "→ Agent Live smoke (provider={} model={})",
        provider.provider_id, provider.model
    );

    let mut event_clock = 0_u64;
    let workspace_root_hash =
        seed_workspace_commit(&mut host, &fixture_root.join(AGENT_WORKSPACE_DIR))?;
    let workspace_binding = WorkspaceBinding {
        workspace: AGENT_WORKSPACE_NAME.into(),
        version: None,
    };
    send_session_event(
        &mut host,
        &mut event_clock,
        SessionIngressKind::WorkspaceSyncRequested {
            workspace_binding: workspace_binding.clone(),
            prompt_pack: Some(DEFAULT_PROMPT_PACK.into()),
        },
    )?;
    let snapshot_ready = build_workspace_snapshot_ready(
        &host,
        &fixture_root.join(AGENT_WORKSPACE_DIR),
        workspace_root_hash,
    )?;
    send_session_event(
        &mut host,
        &mut event_clock,
        SessionIngressKind::WorkspaceSnapshotReady(snapshot_ready),
    )?;
    let synced_state: SessionState = host.read_state()?;
    ensure!(
        synced_state.pending_workspace_snapshot.is_some(),
        "expected pending workspace snapshot after sync"
    );
    send_session_event(
        &mut host,
        &mut event_clock,
        SessionIngressKind::WorkspaceApplyRequested {
            mode: WorkspaceApplyMode::NextRun,
        },
    )?;
    configure_search_tool_registry(
        &mut host,
        &mut event_clock,
        &fixture_root.join(AGENT_WORKSPACE_DIR),
    )?;

    send_session_event(
        &mut host,
        &mut event_clock,
        SessionIngressKind::RunRequested {
            input_ref: fake_hash('a'),
            run_overrides: Some(SessionConfig {
                provider: provider.provider_id.clone(),
                model: provider.model.clone(),
                reasoning_effort: None,
                max_tokens: Some(768),
                workspace_binding: Some(workspace_binding),
                default_prompt_pack: Some(DEFAULT_PROMPT_PACK.into()),
                default_prompt_refs: None,
                default_tool_profile: Some(SEARCH_TOOL_PROFILE.into()),
                default_tool_enable: None,
                default_tool_disable: None,
                default_tool_force: None,
            }),
        },
    )?;
    let state_after_run_request: SessionState = host.read_state()?;
    ensure!(
        state_after_run_request
            .active_workspace_snapshot
            .as_ref()
            .map(|snap| snap.workspace.as_str())
            == Some(AGENT_WORKSPACE_NAME),
        "expected active workspace snapshot applied on next run"
    );
    let mut history: Vec<Value> = vec![json!({
        "role": "user",
        "content": format!(
            "You are operating a search agent. Use tool `search_step` repeatedly until found=true. \
             Start with cursor '{start}'. Do not invent cursor values. \
             When found=true, reply in plain text with: TARGET=<value>; SECOND_CLUE=<text>; STEPS=<n>.",
            start = world.start_cursor
        )
    })];
    println!(
        "   turn 1 user: {}",
        preview(
            history[0]
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
        )
    );

    let mut phase = SearchPhase::Looking;
    let mut stats = SearchStats::default();
    let mut llm_turn = 1_u64;
    let mut expected_followup_clue: Option<String> = None;

    for _ in 0..24 {
        let state: SessionState = host.read_state()?;
        ensure!(
            state.active_run_config.is_some(),
            "agent-live expected active run config"
        );

        let history_ref = store_history_blob(&host, &history)?;
        let turn_tool_refs = if matches!(phase, SearchPhase::Looking) {
            state.effective_tools.tool_refs()
        } else {
            None
        };
        let step_ctx = StepContext {
            correlation_id: Some(format!("live-run-turn-{llm_turn}")),
            message_refs: vec![history_ref],
            temperature: None,
            top_p: None,
            tool_refs: turn_tool_refs,
            tool_choice: Some(match phase {
                SearchPhase::Looking => LlmToolChoice::Required,
                SearchPhase::NeedPrimaryAnswer | SearchPhase::NeedFollowUp | SearchPhase::Done => {
                    LlmToolChoice::NoneChoice
                }
            }),
            stop_sequences: None,
            metadata: None,
            provider_options_ref: None,
            response_format_ref: None,
            api_key: Some(provider.api_key.clone()),
        };
        let params = to_core_llm_params(
            state
                .active_run_config
                .as_ref()
                .ok_or_else(|| anyhow!("missing active_run_config"))?,
            state.active_workspace_snapshot.as_ref(),
            &step_ctx,
        )?;

        let envelope = execute_llm(&runtime, &adapter, &host, &params)?;

        if let Some(tool_calls_ref) = envelope.tool_calls_ref.as_ref() {
            let calls = load_tool_calls(&host, tool_calls_ref.as_str())?;
            ensure!(!calls.is_empty(), "expected non-empty tool call batch");
            stats.tool_rounds = stats.tool_rounds.saturating_add(1);
            stats.tool_calls = stats.tool_calls.saturating_add(calls.len());

            println!(
                "   turn {} assistant: requested {} tool call(s)",
                llm_turn,
                calls.len()
            );

            history.push(json!({
                "role": "assistant",
                "content": calls.iter().map(|call| {
                    json!({
                        "type": "tool_call",
                        "id": call.call_id,
                        "name": call.tool_name,
                        "arguments": tool_call_args_json(&host, call)
                    })
                }).collect::<Vec<_>>()
            }));

            let mut found_target = false;
            for (idx, call) in calls.iter().enumerate() {
                let args = tool_call_args_json(&host, call);
                let output = execute_search_tool(&world, &call.tool_name, &args)?;
                if output.get("found").and_then(Value::as_bool) == Some(true) {
                    found_target = true;
                }
                println!(
                    "      tool {}: {} args={} result={}",
                    idx + 1,
                    call.tool_name,
                    preview(&args.to_string()),
                    preview(&output.to_string())
                );
                history.push(json!({
                    "type": "function_call_output",
                    "call_id": call.call_id,
                    "output": output.clone()
                }));
            }
            if found_target && matches!(phase, SearchPhase::Looking) {
                let nudge = "Tool output is now found=true. Stop calling tools and answer now in the format: TARGET=<value>; SECOND_CLUE=<text>; STEPS=<n>.";
                println!("   turn {} user: {}", llm_turn + 1, preview(nudge));
                history.push(json!({"role":"user","content": nudge}));
                phase = SearchPhase::NeedPrimaryAnswer;
            }
            llm_turn = llm_turn.saturating_add(1);
            continue;
        }

        let text = envelope.assistant_text.unwrap_or_default();
        ensure!(
            !text.trim().is_empty(),
            "expected assistant text or tool calls"
        );

        match phase {
            SearchPhase::Looking | SearchPhase::NeedPrimaryAnswer => {
                println!("   turn {} assistant: {}", llm_turn, preview(&text));
                ensure!(
                    answer_matches_primary(&text, &world),
                    "primary answer missing expected facts"
                );
                expected_followup_clue = extract_reported_clue(&text, &world).map(str::to_string);
                history.push(json!({"role":"assistant","content":text}));

                let followup_prompt = "Follow-up: repeat the clue text you used for SECOND_CLUE, and what cursor produced the final target?";
                println!(
                    "   turn {} user: {}",
                    llm_turn + 1,
                    preview(followup_prompt)
                );
                history.push(json!({"role":"user","content":followup_prompt}));

                phase = SearchPhase::NeedFollowUp;
                llm_turn = llm_turn.saturating_add(1);
            }
            SearchPhase::NeedFollowUp => {
                println!("   turn {} assistant: {}", llm_turn, preview(&text));
                let clue = expected_followup_clue
                    .as_deref()
                    .unwrap_or(world.clue_two.as_str());
                ensure!(
                    answer_matches_followup(&text, &world, clue),
                    "follow-up answer missing expected facts"
                );
                history.push(json!({"role":"assistant","content":text}));
                phase = SearchPhase::Done;
                break;
            }
            SearchPhase::Done => break,
        }
    }

    ensure!(
        matches!(phase, SearchPhase::Done),
        "agent did not complete flow"
    );
    ensure!(
        stats.tool_rounds >= 2,
        "expected >=2 tool rounds, got {}",
        stats.tool_rounds
    );
    ensure!(
        stats.tool_calls >= 3,
        "expected >=3 tool calls, got {}",
        stats.tool_calls
    );

    send_session_event(
        &mut host,
        &mut event_clock,
        SessionIngressKind::RunCompleted,
    )?;

    let final_state: SessionState = host.read_state()?;
    ensure!(
        final_state.active_run_id.is_none() && final_state.active_run_config.is_none(),
        "expected active run cleared after completion"
    );

    let key = host.single_keyed_cell_key()?;
    host.finish_with_keyed_samples(Some(WORKFLOW_NAME), &[key])?
        .verify_replay()?;

    println!(
        "   agent live smoke: OK (tool_rounds={} tool_calls={})",
        stats.tool_rounds, stats.tool_calls
    );

    Ok(())
}

fn tool_call_args_json(host: &ExampleHost, call: &aos_agent::ToolCallObserved) -> Value {
    if let Some(arguments_ref) = call.arguments_ref.as_ref()
        && let Ok(value) = load_json_blob(host.store().as_ref(), arguments_ref)
    {
        return value;
    }

    serde_json::from_str(&call.arguments_json).unwrap_or_else(|_| json!({}))
}

fn send_session_event(
    host: &mut ExampleHost,
    clock: &mut u64,
    kind: SessionIngressKind,
) -> Result<()> {
    *clock = clock.saturating_add(1);
    let event = SessionIngress {
        session_id: SessionId(SESSION_ID.into()),
        observed_at_ns: *clock,
        ingress: kind,
    };
    host.send_event(&event)
}

fn configure_search_tool_registry(
    host: &mut ExampleHost,
    clock: &mut u64,
    workspace_dir: &Path,
) -> Result<()> {
    let tool_catalog_path = workspace_dir.join("tools/catalogs/default.json");
    let tool_catalog_bytes = fs::read(&tool_catalog_path)
        .with_context(|| format!("read tool catalog {}", tool_catalog_path.display()))?;
    let tool_ref = host
        .store()
        .put_blob(&tool_catalog_bytes)
        .context("store search tool catalog blob")?
        .to_hex();

    let mut registry = BTreeMap::new();
    registry.insert(
        SEARCH_TOOL_NAME.into(),
        ToolSpec {
            tool_name: SEARCH_TOOL_NAME.into(),
            tool_ref,
            description: "Traverse one hop in the search graph.".into(),
            args_schema_json: "{\"type\":\"object\",\"properties\":{\"cursor\":{\"type\":\"string\"}},\"required\":[\"cursor\"]}".into(),
            mapper: aos_agent::ToolMapper::HostExec,
            executor: ToolExecutor::HostLoop {
                bridge: SEARCH_TOOL_NAME.into(),
            },
            availability_rules: vec![ToolAvailabilityRule::Always],
            parallelism_hint: ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        },
    );

    let mut profiles = BTreeMap::new();
    profiles.insert(SEARCH_TOOL_PROFILE.into(), vec![SEARCH_TOOL_NAME.into()]);

    send_session_event(
        host,
        clock,
        SessionIngressKind::ToolRegistrySet {
            registry,
            profiles: Some(profiles),
            default_profile: Some(SEARCH_TOOL_PROFILE.into()),
        },
    )
}

fn to_core_llm_params(
    run_config: &aos_agent::RunConfig,
    active_workspace_snapshot: Option<&aos_agent::WorkspaceSnapshot>,
    step_ctx: &StepContext,
) -> Result<LlmGenerateParams> {
    let provider = run_config.provider.trim();
    ensure!(!provider.is_empty(), "run provider missing");
    let model = run_config.model.trim();
    ensure!(!model.is_empty(), "run model missing");

    let mut message_refs = if let Some(explicit) = &run_config.prompt_refs {
        explicit.clone()
    } else if let Some(prompt_pack_ref) =
        active_workspace_snapshot.and_then(|snapshot| snapshot.prompt_pack_ref.clone())
    {
        vec![prompt_pack_ref]
    } else {
        Vec::new()
    };
    message_refs.extend(step_ctx.message_refs.clone());
    ensure!(!message_refs.is_empty(), "message_refs must not be empty");

    let message_refs = message_refs
        .into_iter()
        .map(|value| HashRef::new(value).map_err(|err| anyhow!("invalid message ref: {err}")))
        .collect::<Result<Vec<_>>>()?;

    let tool_refs = step_ctx
        .tool_refs
        .clone()
        .map(|values| {
            values
                .into_iter()
                .map(|value| HashRef::new(value).map_err(|err| anyhow!("invalid tool ref: {err}")))
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?;
    let provider_options_ref = step_ctx
        .provider_options_ref
        .clone()
        .map(|value| {
            HashRef::new(value).map_err(|err| anyhow!("invalid provider_options_ref: {err}"))
        })
        .transpose()?;
    let response_format_ref = step_ctx
        .response_format_ref
        .clone()
        .map(|value| {
            HashRef::new(value).map_err(|err| anyhow!("invalid response_format_ref: {err}"))
        })
        .transpose()?;

    Ok(LlmGenerateParams {
        correlation_id: step_ctx.correlation_id.clone(),
        provider: provider.into(),
        model: model.into(),
        message_refs,
        runtime: LlmRuntimeArgs {
            temperature: step_ctx.temperature.clone(),
            top_p: step_ctx.top_p.clone(),
            max_tokens: run_config.max_tokens,
            tool_refs,
            tool_choice: step_ctx.tool_choice.clone(),
            reasoning_effort: run_config.reasoning_effort.map(reasoning_effort_text),
            stop_sequences: step_ctx.stop_sequences.clone(),
            metadata: step_ctx.metadata.clone(),
            provider_options_ref,
            response_format_ref,
        },
        api_key: step_ctx.api_key.clone(),
    })
}

fn reasoning_effort_text(value: aos_agent::ReasoningEffort) -> String {
    match value {
        aos_agent::ReasoningEffort::Low => "low".into(),
        aos_agent::ReasoningEffort::Medium => "medium".into(),
        aos_agent::ReasoningEffort::High => "high".into(),
    }
}

fn execute_llm(
    runtime: &tokio::runtime::Runtime,
    adapter: &LlmAdapter<aos_store::FsStore>,
    host: &ExampleHost,
    params: &LlmGenerateParams,
) -> Result<LlmOutputEnvelope> {
    let intent = build_llm_intent(params)?;
    let receipt = runtime
        .block_on(adapter.execute(&intent))
        .context("execute live llm")?;

    let payload: LlmGenerateReceipt =
        serde_cbor::from_slice(&receipt.payload_cbor).context("decode llm receipt")?;
    if receipt.status != ReceiptStatus::Ok {
        let error_text = load_text_blob(host.store().as_ref(), payload.output_ref.as_str())
            .unwrap_or_else(|_| "<unable to decode provider error>".into());
        return Err(anyhow!(
            "llm receipt failed status={:?} adapter={} error={}",
            receipt.status,
            receipt.adapter_id,
            error_text
        ));
    }

    let value = load_json_blob(host.store().as_ref(), payload.output_ref.as_str())?;
    serde_json::from_value(value).context("decode llm output envelope")
}

fn build_llm_intent(params: &LlmGenerateParams) -> Result<EffectIntent> {
    let params_cbor = serde_cbor::to_vec(params).context("encode llm params cbor")?;
    EffectIntent::from_raw_params(EffectKind::llm_generate(), "cap", params_cbor, [0u8; 32])
        .context("build llm intent")
}

fn make_adapter(
    store: std::sync::Arc<aos_store::FsStore>,
    provider: &ProviderRuntime,
) -> LlmAdapter<aos_store::FsStore> {
    let mut providers = HashMap::new();
    providers.insert(
        provider.provider_id.clone(),
        ProviderConfig {
            base_url: provider.base_url.clone(),
            timeout: std::time::Duration::from_secs(120),
            api_kind: provider.api_kind,
        },
    );
    let config = LlmAdapterConfig {
        providers,
        default_provider: provider.provider_id.clone(),
    };
    LlmAdapter::new(store, config)
}

fn resolve_provider(
    provider: LiveProvider,
    model_override: Option<String>,
) -> Result<ProviderRuntime> {
    match provider {
        LiveProvider::Openai => {
            let api_key = env_or_dotenv_var(OPENAI_KEY_ENV)
                .ok_or_else(|| anyhow!("missing {} (env or .env)", OPENAI_KEY_ENV))?;
            let model = model_override.unwrap_or_else(|| {
                env_or_dotenv_var(OPENAI_MODEL_ENV).unwrap_or_else(|| "gpt-5.2".into())
            });
            let base_url = env_or_dotenv_var(OPENAI_BASE_URL_ENV)
                .unwrap_or_else(|| "https://api.openai.com/v1".into());
            Ok(ProviderRuntime {
                provider_id: "openai-responses".into(),
                api_kind: LlmApiKind::Responses,
                api_key,
                model,
                base_url,
            })
        }
        LiveProvider::Anthropic => {
            let api_key = env_or_dotenv_var(ANTHROPIC_KEY_ENV)
                .ok_or_else(|| anyhow!("missing {} (env or .env)", ANTHROPIC_KEY_ENV))?;
            let model = model_override.unwrap_or_else(|| {
                env_or_dotenv_var(ANTHROPIC_MODEL_ENV).unwrap_or_else(|| "claude-sonnet-4-5".into())
            });
            let base_url = env_or_dotenv_var(ANTHROPIC_BASE_URL_ENV)
                .unwrap_or_else(|| "https://api.anthropic.com/v1".into());
            Ok(ProviderRuntime {
                provider_id: "anthropic".into(),
                api_kind: LlmApiKind::AnthropicMessages,
                api_key,
                model,
                base_url,
            })
        }
    }
}

fn store_history_blob(host: &ExampleHost, history: &[Value]) -> Result<String> {
    let bytes = serde_json::to_vec(&Value::Array(history.to_vec())).context("encode history")?;
    let hash = host
        .store()
        .put_blob(&bytes)
        .context("store history blob")?;
    Ok(hash.to_hex())
}

fn seed_workspace_commit(host: &mut ExampleHost, workspace_dir: &Path) -> Result<String> {
    let root_hash = build_workspace_root_hash(host.store().as_ref(), workspace_dir)?;
    let commit = WorkspaceCommit {
        workspace: AGENT_WORKSPACE_NAME.into(),
        expected_head: None,
        meta: WorkspaceCommitMeta {
            root_hash: root_hash.clone(),
            owner: "aos-smoke".into(),
            created_at: now_nonce(),
        },
    };
    host.send_event_as(WORKSPACE_COMMIT_SCHEMA, &commit)?;
    Ok(root_hash)
}

fn build_workspace_snapshot_ready(
    host: &ExampleHost,
    workspace_dir: &Path,
    root_hash: String,
) -> Result<WorkspaceSnapshotReady> {
    let index_path = workspace_dir.join("agent.workspace.json");
    let prompt_pack_path = workspace_dir.join("prompts/packs/default.json");

    let index_bytes = fs::read(&index_path)
        .with_context(|| format!("read workspace index {}", index_path.display()))?;
    let prompt_pack_bytes = fs::read(&prompt_pack_path)
        .with_context(|| format!("read prompt pack {}", prompt_pack_path.display()))?;

    let store = host.store();
    let index_ref = store
        .put_blob(&index_bytes)
        .context("store workspace index blob")?
        .to_hex();
    let prompt_pack_ref = store
        .put_blob(&prompt_pack_bytes)
        .context("store workspace prompt pack blob")?
        .to_hex();

    Ok(WorkspaceSnapshotReady {
        snapshot: WorkspaceSnapshot {
            workspace: AGENT_WORKSPACE_NAME.into(),
            version: Some(1),
            root_hash: Some(root_hash),
            index_ref: Some(index_ref),
            prompt_pack: Some(DEFAULT_PROMPT_PACK.into()),
            prompt_pack_ref: Some(prompt_pack_ref),
        },
        prompt_pack_bytes: Some(prompt_pack_bytes),
    })
}

#[derive(Default)]
struct WorkspaceDirNode {
    dirs: BTreeMap<String, WorkspaceDirNode>,
    files: BTreeMap<String, Vec<u8>>,
}

fn build_workspace_root_hash(store: &aos_store::FsStore, workspace_dir: &Path) -> Result<String> {
    let mut root = WorkspaceDirNode::default();
    for entry in WalkDir::new(workspace_dir) {
        let entry = entry.context("walk workspace dir entry")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let full_path = entry.path();
        let rel = full_path
            .strip_prefix(workspace_dir)
            .with_context(|| format!("strip workspace prefix for {}", full_path.display()))?;
        let rel_text = rel
            .to_str()
            .ok_or_else(|| anyhow!("non-utf8 workspace path {}", rel.display()))?
            .replace('\\', "/");
        let bytes = fs::read(full_path)
            .with_context(|| format!("read workspace file {}", full_path.display()))?;
        insert_workspace_file(&mut root, &rel_text, bytes)?;
    }
    ensure!(
        !root.files.is_empty() || !root.dirs.is_empty(),
        "workspace dir is empty: {}",
        workspace_dir.display()
    );
    encode_workspace_dir(store, &root)
}

fn insert_workspace_file(
    root: &mut WorkspaceDirNode,
    rel_path: &str,
    bytes: Vec<u8>,
) -> Result<()> {
    let mut segments = rel_path.split('/').peekable();
    let mut cursor = root;
    while let Some(segment) = segments.next() {
        if segment.is_empty() {
            return Err(anyhow!("invalid workspace path segment in {rel_path}"));
        }
        if segments.peek().is_none() {
            cursor.files.insert(segment.to_string(), bytes);
            return Ok(());
        }
        cursor = cursor.dirs.entry(segment.to_string()).or_default();
    }
    Err(anyhow!("empty workspace file path"))
}

fn encode_workspace_dir(store: &aos_store::FsStore, node: &WorkspaceDirNode) -> Result<String> {
    let mut entries: Vec<WorkspaceEntry> = Vec::new();
    for (name, child) in &node.dirs {
        let child_hash = encode_workspace_dir(store, child)?;
        entries.push(WorkspaceEntry {
            name: name.clone(),
            kind: "dir".into(),
            hash: child_hash,
            size: 0,
            mode: 0o755,
        });
    }
    for (name, bytes) in &node.files {
        let blob_hash = store
            .put_blob(bytes)
            .with_context(|| format!("store workspace file blob {name}"))?;
        entries.push(WorkspaceEntry {
            name: name.clone(),
            kind: "file".into(),
            hash: blob_hash.to_hex(),
            size: bytes.len() as u64,
            mode: 0o644,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    let tree = WorkspaceTree { entries };
    let hash = store.put_node(&tree).context("store workspace tree node")?;
    Ok(hash.to_hex())
}

fn load_tool_calls(host: &ExampleHost, tool_calls_ref: &str) -> Result<LlmToolCallList> {
    let value = load_json_blob(host.store().as_ref(), tool_calls_ref)?;
    serde_json::from_value(value).context("decode sdk llm tool call list")
}

fn execute_search_tool(world: &SearchWorld, tool_name: &str, args: &Value) -> Result<Value> {
    if tool_name != "search_step" {
        return Ok(json!({
            "ok": false,
            "error": "unknown_tool",
            "detail": format!("unsupported tool {tool_name}")
        }));
    }

    let cursor = args
        .get("cursor")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    if cursor == world.start_cursor {
        return Ok(json!({
            "ok": true,
            "found": false,
            "cursor": cursor,
            "clue": world.clue_one,
            "next_cursor": world.first_cursor
        }));
    }
    if cursor == world.first_cursor {
        return Ok(json!({
            "ok": true,
            "found": false,
            "cursor": cursor,
            "clue": world.clue_two,
            "next_cursor": world.second_cursor
        }));
    }
    if cursor == world.second_cursor {
        return Ok(json!({
            "ok": true,
            "found": false,
            "cursor": cursor,
            "clue": world.clue_three,
            "next_cursor": world.target_cursor
        }));
    }
    if cursor == world.target_cursor {
        return Ok(json!({
            "ok": true,
            "found": true,
            "cursor": cursor,
            "target": world.target_value,
            "steps": 4
        }));
    }

    Ok(json!({
        "ok": false,
        "found": false,
        "cursor": cursor,
        "error": "unknown_cursor",
        "hint": "Use the exact next_cursor returned by the previous tool result"
    }))
}

impl SearchWorld {
    fn new() -> Self {
        let nonce = now_nonce();
        let cursor_a = format!("c-{nonce}-a");
        let cursor_b = format!("c-{nonce}-b");
        let cursor_c = format!("c-{nonce}-c");
        let cursor_t = format!("c-{nonce}-target");
        let target = format!("cobalt-{:04}", (nonce % 10_000));
        Self {
            start_cursor: "cursor:start".into(),
            first_cursor: cursor_a,
            second_cursor: cursor_b,
            target_cursor: cursor_t,
            clue_one: format!("branch gamma -> check node {cursor_c}"),
            clue_two: "shelf-7 has the traversal manifest".into(),
            clue_three: "final marker says use the target cursor".into(),
            target_value: target,
        }
    }
}

fn answer_matches_primary(answer: &str, world: &SearchWorld) -> bool {
    let lower = answer.to_ascii_lowercase();
    lower.contains(&world.target_value.to_ascii_lowercase())
        && extract_reported_clue(answer, world).is_some()
        && (lower.contains("step") || lower.contains("steps"))
}

fn extract_reported_clue<'a>(answer: &str, world: &'a SearchWorld) -> Option<&'a str> {
    let lower = answer.to_ascii_lowercase();
    [world.clue_two.as_str(), world.clue_three.as_str()]
        .into_iter()
        .find(|clue| lower.contains(&clue.to_ascii_lowercase()))
}

fn answer_matches_followup(answer: &str, world: &SearchWorld, clue: &str) -> bool {
    let lower = answer.to_ascii_lowercase();
    lower.contains(&clue.to_ascii_lowercase())
        && lower.contains(&world.target_cursor.to_ascii_lowercase())
}

fn now_nonce() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(42)
}

fn fake_hash(ch: char) -> String {
    let mut out = String::from("sha256:");
    for _ in 0..64 {
        out.push(ch);
    }
    out
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

fn store_json_blob(host: &ExampleHost, value: &Value) -> Result<String> {
    let bytes = serde_json::to_vec(value).context("encode json blob")?;
    let hash = host.store().put_blob(&bytes).context("store json blob")?;
    Ok(hash.to_hex())
}

fn preview(text: &str) -> String {
    const MAX: usize = 160;
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = compact.trim();
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
