use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail, ensure};
use aos_agent::{
    HostSessionStatus, SessionConfig, SessionId, SessionIngress, SessionIngressKind,
    SessionLifecycle, SessionState, ToolCallStatus, default_tool_registry,
};
use aos_air_types::HashRef;
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effects::builtins::{
    BlobEdge, BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt, HostExecReceipt,
    HostFsExistsReceipt, HostFsListDirReceipt, HostFsReadFileReceipt, HostFsWriteFileReceipt,
    HostInlineText, HostOutput, HostSessionOpenParams, HostSessionOpenReceipt, HostTextOutput,
    LlmFinishReason, LlmGenerateParams, LlmGenerateReceipt, LlmOutputEnvelope, LlmToolCall,
    LlmToolCallList, TokenUsage,
};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use aos_host::config::HostConfig;
use aos_store::{FsStore, Store};
use serde::Serialize;
use serde_json::{Value, json};

use crate::example_host::{ExampleHost, HarnessConfig};

const WORKFLOW_NAME: &str = "aos.agent/SessionWorkflow@1";
const EVENT_SCHEMA: &str = "aos.agent/SessionIngress@1";
const SDK_AIR_ROOT: &str = "crates/aos-agent/air";
const SDK_WASM_PACKAGE: &str = "aos-agent";
const SDK_WASM_BIN: &str = "session_workflow";
const SESSION_ID: &str = "33333333-3333-3333-3333-333333333333";
const TOOL_PROFILE: &str = "smoke-host";
const SCRIPTED_SESSION_ID: &str = "hs_tools_opened";

const CALL_OPEN: &str = "call-open";
const CALL_WRITE: &str = "call-write";
const CALL_EXISTS: &str = "call-exists";
const CALL_READ: &str = "call-read";
const CALL_LIST: &str = "call-list";
const CALL_EXEC: &str = "call-exec";

const TOOL_SESSION_OPEN: &str = "host.session.open";
const TOOL_FS_WRITE: &str = "host.fs.write_file";
const TOOL_FS_EXISTS: &str = "host.fs.exists";
const TOOL_FS_READ: &str = "host.fs.read_file";
const TOOL_FS_LIST: &str = "host.fs.list_dir";
const TOOL_EXEC: &str = "host.exec";

const TEST_FILE_PATH: &str = "notes/hello.txt";
const TEST_FILE_TEXT: &str = "hello from agent-tools";

pub fn run(example_root: &Path) -> Result<()> {
    let sdk_air_root = crate::workspace_root().join(SDK_AIR_ROOT);
    let import_roots = vec![sdk_air_root];
    let mut host = ExampleHost::prepare_with_imports_host_config_and_module_bin(
        HarnessConfig {
            example_root,
            assets_root: None,
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

    println!("→ Agent Tools demo");

    let mut clock = 0_u64;
    send_session_event(
        &mut host,
        &mut clock,
        SessionIngressKind::HostSessionUpdated {
            host_session_id: Some("hs_seed".into()),
            host_session_status: Some(HostSessionStatus::Ready),
        },
    )?;
    configure_tool_registry(&mut host, &mut clock)?;

    let input_ref = store_json_blob(
        host.store().as_ref(),
        &json!({
            "role": "user",
            "content": "Run the scripted host tool validation flow."
        }),
    )?;
    send_session_event(
        &mut host,
        &mut clock,
        SessionIngressKind::RunRequested {
            input_ref: input_ref.as_str().to_string(),
            run_overrides: Some(SessionConfig {
                provider: "openai-responses".into(),
                model: "gpt-5.2".into(),
                reasoning_effort: None,
                max_tokens: Some(512),
                workspace_binding: None,
                default_prompt_pack: None,
                default_prompt_refs: None,
                default_tool_profile: Some(TOOL_PROFILE.into()),
                default_tool_enable: None,
                default_tool_disable: None,
                default_tool_force: None,
            }),
        },
    )?;

    let mut script = AgentToolsScript::default();
    drive_scripted_effects(&mut host, &mut script)?;

    let state_after_tools: SessionState = host.read_state()?;
    ensure!(
        state_after_tools.lifecycle == SessionLifecycle::WaitingInput,
        "expected WaitingInput after scripted tool + follow-up llm flow, got {:?}",
        state_after_tools.lifecycle
    );
    let batch = state_after_tools
        .active_tool_batch
        .as_ref()
        .ok_or_else(|| anyhow!("expected active tool batch"))?;
    ensure!(batch.is_settled(), "expected tool batch settled");
    for call_id in [CALL_OPEN, CALL_WRITE, CALL_EXISTS, CALL_READ, CALL_LIST, CALL_EXEC] {
        ensure!(
            matches!(batch.call_status.get(call_id), Some(ToolCallStatus::Succeeded)),
            "expected call {call_id} status Succeeded"
        );
    }
    let expected_groups = vec![
        vec![CALL_OPEN.to_string()],
        vec![
            CALL_WRITE.to_string(),
            CALL_EXISTS.to_string(),
            CALL_READ.to_string(),
            CALL_LIST.to_string(),
        ],
        vec![CALL_EXEC.to_string()],
    ];
    ensure!(
        batch.plan.execution_plan.groups == expected_groups,
        "unexpected execution grouping: {:?}",
        batch.plan.execution_plan.groups
    );
    let expected_kinds = BTreeSet::from([
        TOOL_SESSION_OPEN.to_string(),
        TOOL_FS_WRITE.to_string(),
        TOOL_FS_EXISTS.to_string(),
        TOOL_FS_READ.to_string(),
        TOOL_FS_LIST.to_string(),
        TOOL_EXEC.to_string(),
    ]);
    ensure!(
        script.seen_tool_effect_kinds == expected_kinds,
        "unexpected seen tool kinds: {:?}",
        script.seen_tool_effect_kinds
    );
    ensure!(
        script.llm_turn == 2,
        "expected exactly 2 llm turns, got {}",
        script.llm_turn
    );

    send_session_event(&mut host, &mut clock, SessionIngressKind::RunCompleted)?;
    let final_state: SessionState = host.read_state()?;
    ensure!(
        final_state.lifecycle == SessionLifecycle::Completed,
        "expected Completed lifecycle, got {:?}",
        final_state.lifecycle
    );
    ensure!(
        final_state.active_run_id.is_none() && final_state.active_run_config.is_none(),
        "expected active run cleared after completion"
    );

    println!(
        "   tool flow validated: llm_turns={} kinds={}",
        script.llm_turn,
        script.seen_tool_effect_kinds.len()
    );

    let key = host.single_keyed_cell_key()?;
    host.finish_with_keyed_samples(Some(WORKFLOW_NAME), &[key])?
        .verify_replay()?;
    Ok(())
}

#[derive(Debug, Default)]
struct AgentToolsScript {
    llm_turn: u64,
    seen_tool_effect_kinds: BTreeSet<String>,
    opened_session_id: Option<String>,
}

impl AgentToolsScript {
    fn handle_intent(&mut self, store: &FsStore, intent: EffectIntent) -> Result<EffectReceipt> {
        match intent.kind.as_str() {
            EffectKind::BLOB_PUT => self.handle_blob_put(store, intent),
            EffectKind::BLOB_GET => self.handle_blob_get(store, intent),
            EffectKind::LLM_GENERATE => self.handle_llm_generate(store, intent),
            TOOL_SESSION_OPEN => self.handle_host_session_open(intent),
            TOOL_FS_WRITE => self.handle_host_fs_write(intent),
            TOOL_FS_EXISTS => self.handle_host_fs_exists(intent),
            TOOL_FS_READ => self.handle_host_fs_read(intent),
            TOOL_FS_LIST => self.handle_host_fs_list(intent),
            TOOL_EXEC => self.handle_host_exec(intent),
            other => bail!("unexpected effect kind in agent-tools smoke: {other}"),
        }
    }

    fn handle_blob_put(&mut self, store: &FsStore, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: BlobPutParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode blob.put params")?;
        let blob_hash = store.put_blob(&params.bytes).context("store blob.put bytes")?;
        let blob_ref = HashRef::new(blob_hash.to_hex()).context("blob_ref hash")?;

        let edge_bytes = to_canonical_cbor(&BlobEdge {
            blob_ref: blob_ref.clone(),
            refs: params.refs.unwrap_or_default(),
        })?;
        let edge_ref =
            HashRef::new(Hash::of_bytes(&edge_bytes).to_hex()).context("blob edge_ref hash")?;

        ok_receipt(
            intent,
            &BlobPutReceipt {
                blob_ref,
                edge_ref,
                size: params.bytes.len() as u64,
            },
            "adapter.blob.fake",
        )
    }

    fn handle_blob_get(&mut self, store: &FsStore, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: BlobGetParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode blob.get params")?;
        let hash = hash_from_ref(params.blob_ref.as_str())?;
        let bytes = store.get_blob(hash).context("load blob.get bytes")?;

        ok_receipt(
            intent,
            &BlobGetReceipt {
                blob_ref: params.blob_ref,
                size: bytes.len() as u64,
                bytes,
            },
            "adapter.blob.fake",
        )
    }

    fn handle_llm_generate(&mut self, store: &FsStore, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: LlmGenerateParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode llm.generate params")?;
        ensure!(!params.provider.trim().is_empty(), "llm provider must be set");
        ensure!(!params.model.trim().is_empty(), "llm model must be set");

        self.llm_turn = self.llm_turn.saturating_add(1);
        let output_ref = match self.llm_turn {
            1 => build_first_llm_output_ref(store)?,
            2 => build_second_llm_output_ref(store)?,
            turn => bail!("unexpected llm turn {turn}"),
        };

        ok_receipt(
            intent,
            &LlmGenerateReceipt {
                output_ref,
                raw_output_ref: None,
                provider_response_id: Some(format!("resp-{}", self.llm_turn)),
                finish_reason: LlmFinishReason {
                    reason: if self.llm_turn == 1 {
                        "tool_calls".into()
                    } else {
                        "stop".into()
                    },
                    raw: None,
                },
                token_usage: TokenUsage {
                    prompt: 0,
                    completion: 0,
                    total: Some(0),
                },
                usage_details: None,
                warnings_ref: None,
                rate_limit_ref: None,
                cost_cents: Some(0),
                provider_id: params.provider,
            },
            "adapter.llm.fake",
        )
    }

    fn handle_host_session_open(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let _params: HostSessionOpenParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode host.session.open params")?;

        self.seen_tool_effect_kinds.insert(TOOL_SESSION_OPEN.into());
        self.opened_session_id = Some(SCRIPTED_SESSION_ID.into());

        ok_receipt(
            intent,
            &HostSessionOpenReceipt {
                session_id: SCRIPTED_SESSION_ID.into(),
                status: "ready".into(),
                started_at_ns: 1,
                expires_at_ns: None,
                error_code: None,
                error_message: None,
            },
            "adapter.host.fake",
        )
    }

    fn handle_host_fs_write(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: Value = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode host.fs.write_file params")?;
        let session_id = require_json_string(&params, "session_id")?;
        self.ensure_runtime_session(session_id.as_str())?;
        let path = require_json_string(&params, "path")?;
        ensure!(
            path == TEST_FILE_PATH,
            "expected write path {TEST_FILE_PATH}, got {}",
            path
        );
        ensure!(
            params.get("content").is_some(),
            "host.fs.write_file params missing content field"
        );

        self.seen_tool_effect_kinds.insert(TOOL_FS_WRITE.into());
        ok_receipt(
            intent,
            &HostFsWriteFileReceipt {
                status: "ok".into(),
                written_bytes: Some(TEST_FILE_TEXT.len() as u64),
                created: Some(true),
                new_mtime_ns: Some(11),
                error_code: None,
            },
            "adapter.host.fake",
        )
    }

    fn handle_host_fs_exists(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: Value = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode host.fs.exists params")?;
        let session_id = require_json_string(&params, "session_id")?;
        self.ensure_runtime_session(session_id.as_str())?;
        let path = require_json_string(&params, "path")?;
        ensure!(
            path == TEST_FILE_PATH,
            "expected exists path {TEST_FILE_PATH}, got {}",
            path
        );

        self.seen_tool_effect_kinds.insert(TOOL_FS_EXISTS.into());
        ok_receipt(
            intent,
            &HostFsExistsReceipt {
                status: "ok".into(),
                exists: Some(true),
                error_code: None,
            },
            "adapter.host.fake",
        )
    }

    fn handle_host_fs_read(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: Value = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode host.fs.read_file params")?;
        let session_id = require_json_string(&params, "session_id")?;
        self.ensure_runtime_session(session_id.as_str())?;
        let path = require_json_string(&params, "path")?;
        ensure!(
            path == TEST_FILE_PATH,
            "expected read path {TEST_FILE_PATH}, got {}",
            path
        );

        self.seen_tool_effect_kinds.insert(TOOL_FS_READ.into());
        ok_receipt(
            intent,
            &HostFsReadFileReceipt {
                status: "ok".into(),
                content: Some(HostOutput::InlineText {
                    inline_text: HostInlineText {
                        text: TEST_FILE_TEXT.into(),
                    },
                }),
                truncated: Some(false),
                size_bytes: Some(TEST_FILE_TEXT.len() as u64),
                error_code: None,
            },
            "adapter.host.fake",
        )
    }

    fn handle_host_fs_list(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: Value = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode host.fs.list_dir params")?;
        let session_id = require_json_string(&params, "session_id")?;
        self.ensure_runtime_session(session_id.as_str())?;
        let path = params.get("path").and_then(Value::as_str);
        ensure!(
            path == Some("notes"),
            "expected list path notes, got {:?}",
            path
        );

        self.seen_tool_effect_kinds.insert(TOOL_FS_LIST.into());
        ok_receipt(
            intent,
            &HostFsListDirReceipt {
                status: "ok".into(),
                entries: Some(HostTextOutput::InlineText {
                    inline_text: HostInlineText {
                        text: "hello.txt\n".into(),
                    },
                }),
                count: Some(1),
                truncated: Some(false),
                error_code: None,
                summary_text: None,
            },
            "adapter.host.fake",
        )
    }

    fn handle_host_exec(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: Value =
            serde_cbor::from_slice(&intent.params_cbor).context("decode host.exec params")?;
        let session_id = require_json_string(&params, "session_id")?;
        self.ensure_runtime_session(session_id.as_str())?;
        let argv = params
            .get("argv")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("host.exec params missing argv array"))?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        ensure!(
            argv == vec!["echo".to_string(), "agent-tools".to_string()],
            "unexpected argv in host.exec: {:?}",
            argv
        );

        self.seen_tool_effect_kinds.insert(TOOL_EXEC.into());
        ok_receipt(
            intent,
            &HostExecReceipt {
                exit_code: 0,
                status: "ok".into(),
                stdout: Some(HostOutput::InlineText {
                    inline_text: HostInlineText {
                        text: "agent-tools\n".into(),
                    },
                }),
                stderr: None,
                started_at_ns: 17,
                ended_at_ns: 21,
                error_code: None,
                error_message: None,
            },
            "adapter.host.fake",
        )
    }

    fn ensure_runtime_session(&self, actual: &str) -> Result<()> {
        let expected = self
            .opened_session_id
            .as_deref()
            .ok_or_else(|| anyhow!("expected host.session.open receipt before fs/exec calls"))?;
        ensure!(
            actual == expected,
            "expected mapped session_id={expected}, got {actual}"
        );
        Ok(())
    }
}

fn configure_tool_registry(host: &mut ExampleHost, clock: &mut u64) -> Result<()> {
    let mut registry = default_tool_registry();
    let ordered = vec![
        TOOL_SESSION_OPEN.to_string(),
        TOOL_FS_WRITE.to_string(),
        TOOL_FS_EXISTS.to_string(),
        TOOL_FS_READ.to_string(),
        TOOL_FS_LIST.to_string(),
        TOOL_EXEC.to_string(),
    ];
    let allowed: BTreeSet<String> = ordered.iter().cloned().collect();
    registry.retain(|name, _| allowed.contains(name));
    ensure!(
        registry.len() == ordered.len(),
        "tool registry subset missing expected built-ins"
    );

    let mut profiles = BTreeMap::new();
    profiles.insert(TOOL_PROFILE.into(), ordered);

    send_session_event(
        host,
        clock,
        SessionIngressKind::ToolRegistrySet {
            registry,
            profiles: Some(profiles),
            default_profile: Some(TOOL_PROFILE.into()),
        },
    )
}

fn send_session_event(
    host: &mut ExampleHost,
    clock: &mut u64,
    kind: SessionIngressKind,
) -> Result<()> {
    *clock = clock.saturating_add(1);
    host.send_event(&SessionIngress {
        session_id: SessionId(SESSION_ID.into()),
        observed_at_ns: *clock,
        ingress: kind,
    })
}

fn drive_scripted_effects(host: &mut ExampleHost, script: &mut AgentToolsScript) -> Result<()> {
    let store = host.store();
    for _ in 0..256 {
        let intents = host.kernel_mut().drain_effects()?;
        if intents.is_empty() {
            return Ok(());
        }

        for intent in intents {
            let receipt = script.handle_intent(store.as_ref(), intent)?;
            host.kernel_mut().handle_receipt(receipt)?;
            host.kernel_mut().tick_until_idle()?;
        }
    }
    bail!("agent-tools safety trip: effects did not drain")
}

fn build_first_llm_output_ref(store: &FsStore) -> Result<HashRef> {
    let tool_calls = build_scripted_tool_calls(store)?;
    let tool_calls_ref = store_json_blob(store, &serde_json::to_value(tool_calls)?)?;

    let envelope = LlmOutputEnvelope {
        assistant_text: None,
        tool_calls_ref: Some(tool_calls_ref),
        reasoning_ref: None,
    };
    store_json_blob(store, &serde_json::to_value(envelope)?)
}

fn build_second_llm_output_ref(store: &FsStore) -> Result<HashRef> {
    let envelope = LlmOutputEnvelope {
        assistant_text: Some(
            "Tools complete: wrote file, verified existence, read content, listed directory, and executed command."
                .into(),
        ),
        tool_calls_ref: None,
        reasoning_ref: None,
    };
    store_json_blob(store, &serde_json::to_value(envelope)?)
}

fn build_scripted_tool_calls(store: &FsStore) -> Result<LlmToolCallList> {
    let open_args = store_json_blob(
        store,
        &json!({
            "target": { "local": { "network_mode": "off" } }
        }),
    )?;
    let write_args = store_json_blob(
        store,
        &json!({
            "path": TEST_FILE_PATH,
            "text": TEST_FILE_TEXT,
            "create_parents": true
        }),
    )?;
    let exists_args = store_json_blob(
        store,
        &json!({
            "path": TEST_FILE_PATH
        }),
    )?;
    let read_args = store_json_blob(
        store,
        &json!({
            "path": TEST_FILE_PATH,
            "encoding": "utf-8"
        }),
    )?;
    let list_args = store_json_blob(
        store,
        &json!({
            "path": "notes"
        }),
    )?;
    let exec_args = store_json_blob(
        store,
        &json!({
            "argv": ["echo", "agent-tools"],
            "cwd": "."
        }),
    )?;

    Ok(vec![
        LlmToolCall {
            call_id: CALL_OPEN.into(),
            tool_name: TOOL_SESSION_OPEN.into(),
            arguments_ref: open_args,
            provider_call_id: Some("provider-call-open".into()),
        },
        LlmToolCall {
            call_id: CALL_WRITE.into(),
            tool_name: TOOL_FS_WRITE.into(),
            arguments_ref: write_args,
            provider_call_id: Some("provider-call-write".into()),
        },
        LlmToolCall {
            call_id: CALL_EXISTS.into(),
            tool_name: TOOL_FS_EXISTS.into(),
            arguments_ref: exists_args,
            provider_call_id: Some("provider-call-exists".into()),
        },
        LlmToolCall {
            call_id: CALL_READ.into(),
            tool_name: TOOL_FS_READ.into(),
            arguments_ref: read_args,
            provider_call_id: Some("provider-call-read".into()),
        },
        LlmToolCall {
            call_id: CALL_LIST.into(),
            tool_name: TOOL_FS_LIST.into(),
            arguments_ref: list_args,
            provider_call_id: Some("provider-call-list".into()),
        },
        LlmToolCall {
            call_id: CALL_EXEC.into(),
            tool_name: TOOL_EXEC.into(),
            arguments_ref: exec_args,
            provider_call_id: Some("provider-call-exec".into()),
        },
    ])
}

fn store_json_blob(store: &FsStore, value: &Value) -> Result<HashRef> {
    let bytes = serde_json::to_vec(value).context("encode json blob")?;
    let hash = store.put_blob(&bytes).context("store json blob")?;
    HashRef::new(hash.to_hex()).context("json blob hash_ref")
}

fn require_json_string(value: &Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing string field '{field}'"))
}

fn hash_from_ref(blob_ref: &str) -> Result<Hash> {
    Hash::from_hex_str(blob_ref).map_err(|err| anyhow!("invalid hash ref '{blob_ref}': {err}"))
}

fn ok_receipt<T: Serialize>(
    intent: EffectIntent,
    payload: &T,
    adapter_id: &str,
) -> Result<EffectReceipt> {
    Ok(EffectReceipt {
        intent_hash: intent.intent_hash,
        adapter_id: adapter_id.into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(payload).context("encode receipt payload")?,
        cost_cents: Some(0),
        signature: vec![0; 64],
    })
}
