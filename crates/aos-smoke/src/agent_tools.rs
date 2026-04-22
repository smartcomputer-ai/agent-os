use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail, ensure};
use aos_agent::{
    HostSessionStatus, SessionConfig, SessionId, SessionIngress, SessionIngressKind,
    SessionLifecycle, SessionState, ToolCallStatus, default_tool_registry,
};
use aos_air_types::{HashRef, Manifest};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_effect_types::HashRef as EffectHashRef;
use aos_effect_types::introspect::{
    IntrospectCellInfo, IntrospectListCellsParams, IntrospectListCellsReceipt,
    IntrospectManifestParams, IntrospectManifestReceipt, IntrospectWorkflowStateParams,
    IntrospectWorkflowStateReceipt, ReadMeta,
};
use aos_effect_types::workspace::{
    WorkspaceDiffParams, WorkspaceDiffReceipt, WorkspaceEmptyRootParams, WorkspaceEmptyRootReceipt,
    WorkspaceListParams, WorkspaceListReceipt, WorkspaceReadBytesParams, WorkspaceReadRefParams,
    WorkspaceResolveParams, WorkspaceResolveReceipt, WorkspaceWriteBytesParams,
    WorkspaceWriteBytesReceipt, WorkspaceWriteRefParams, WorkspaceWriteRefReceipt,
};
use aos_effects::builtins::{
    BlobEdge, BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt, HostExecReceipt,
    HostFsExistsReceipt, HostFsListDirReceipt, HostFsReadFileReceipt, HostFsWriteFileReceipt,
    HostInlineText, HostOutput, HostSessionOpenParams, HostSessionOpenReceipt, HostTextOutput,
    LlmFinishReason, LlmGenerateParams, LlmGenerateReceipt, LlmOutputEnvelope, LlmToolCall,
    LlmToolCallList, TokenUsage,
};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus, effect_ops};
use aos_kernel::Store;
use aos_node::WorldConfig;
use serde::Serialize;
use serde_json::{Value, json};

use crate::example_host::{ExampleHost, ExampleHostConfig, HarnessConfig};

const WORKFLOW_NAME: &str = "aos.agent/SessionWorkflow@1";
const EVENT_SCHEMA: &str = "aos.agent/SessionIngress@1";
const SDK_AIR_ROOT: &str = "crates/aos-agent/air";
const SDK_WASM_PACKAGE: &str = "aos-agent";
const SDK_WASM_BIN: &str = "session_workflow";
const SESSION_ID: &str = "33333333-3333-3333-3333-333333333333";
const TOOL_PROFILE: &str = "smoke-host";
const SCRIPTED_SESSION_ID: &str = "hs_tools_opened";
const SEEDED_SESSION_ID: &str = "hs_seed";

const CALL_WRITE: &str = "call-write";
const CALL_EXISTS: &str = "call-exists";
const CALL_READ: &str = "call-read";
const CALL_LIST: &str = "call-list";
const CALL_EXEC: &str = "call-exec";
const CALL_INSPECT_WORLD: &str = "call-inspect-world";
const CALL_INSPECT_WORKFLOW_STATE: &str = "call-inspect-workflow-state";
const CALL_INSPECT_WORKFLOW_CELLS: &str = "call-inspect-workflow-cells";
const CALL_WORKSPACE_INSPECT: &str = "call-workspace-inspect";
const CALL_WORKSPACE_LIST_WORKSPACES: &str = "call-workspace-list-workspaces";
const CALL_WORKSPACE_LIST_TREE: &str = "call-workspace-list-tree";
const CALL_WORKSPACE_READ: &str = "call-workspace-read";
const CALL_WORKSPACE_APPLY: &str = "call-workspace-apply";
const CALL_WORKSPACE_COMMIT: &str = "call-workspace-commit";
const CALL_WORKSPACE_DIFF: &str = "call-workspace-diff";

const TOOL_SESSION_OPEN: &str = "host.session.open";
const TOOL_FS_WRITE: &str = "host.fs.write_file";
const TOOL_FS_EXISTS: &str = "host.fs.exists";
const TOOL_FS_READ: &str = "host.fs.read_file";
const TOOL_FS_LIST: &str = "host.fs.list_dir";
const TOOL_EXEC: &str = "host.exec";
const TOOL_INTROSPECT_MANIFEST: &str = "introspect.manifest";
const TOOL_INTROSPECT_WORKFLOW_STATE: &str = "introspect.workflow_state";
const TOOL_INTROSPECT_LIST_CELLS: &str = "introspect.list_cells";
const TOOL_WORKSPACE_RESOLVE: &str = "workspace.resolve";
const TOOL_WORKSPACE_EMPTY_ROOT: &str = "workspace.empty_root";
const TOOL_WORKSPACE_LIST: &str = "workspace.list";
const TOOL_WORKSPACE_READ_REF: &str = "workspace.read_ref";
const TOOL_WORKSPACE_READ_BYTES: &str = "workspace.read_bytes";
const TOOL_WORKSPACE_WRITE_BYTES: &str = "workspace.write_bytes";
const TOOL_WORKSPACE_WRITE_REF: &str = "workspace.write_ref";
const TOOL_WORKSPACE_DIFF: &str = "workspace.diff";

const LLM_TOOL_FS_WRITE: &str = "write_file";
const LLM_TOOL_FS_EXISTS: &str = "exists";
const LLM_TOOL_FS_READ: &str = "read_file";
const LLM_TOOL_FS_LIST: &str = "list_dir";
const LLM_TOOL_EXEC: &str = "shell";
const LLM_TOOL_INSPECT_WORLD: &str = "inspect_world";
const LLM_TOOL_INSPECT_WORKFLOW: &str = "inspect_workflow";
const LLM_TOOL_WORKSPACE_INSPECT: &str = "workspace_inspect";
const LLM_TOOL_WORKSPACE_LIST: &str = "workspace_list";
const LLM_TOOL_WORKSPACE_READ: &str = "workspace_read";
const LLM_TOOL_WORKSPACE_APPLY: &str = "workspace_apply";
const LLM_TOOL_WORKSPACE_COMMIT: &str = "workspace_commit";
const LLM_TOOL_WORKSPACE_DIFF: &str = "workspace_diff";

const TEST_FILE_PATH: &str = "notes/hello.txt";
const TEST_FILE_TEXT: &str = "hello from agent-tools";
const WORKSPACE_ALPHA: &str = "alpha";
const WORKSPACE_BETA: &str = "beta";
const WORKSPACE_DRAFT: &str = "draft";
const WORKSPACE_FILE_PATH: &str = "docs/readme.txt";
const WORKSPACE_TEXT: &str = "workspace hello";
const ALPHA_ROOT_HASH: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BETA_ROOT_HASH: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DRAFT_EMPTY_ROOT_HASH: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const DRAFT_TEXT_ROOT_HASH: &str =
    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const DRAFT_FINAL_ROOT_HASH: &str =
    "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const APPLY_BLOB_HASH: &str =
    "sha256:9999999999999999999999999999999999999999999999999999999999999999";

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
        Some(ExampleHostConfig {
            world: WorldConfig::default(),
            adapters: EffectAdapterConfig {
                llm: None,
                ..EffectAdapterConfig::default()
            },
            ..ExampleHostConfig::default()
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
            host_session_id: Some(SEEDED_SESSION_ID.into()),
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
                default_prompt_refs: None,
                default_tool_profile: Some(TOOL_PROFILE.into()),
                default_tool_enable: None,
                default_tool_disable: None,
                default_tool_force: None,
            }),
        },
    )?;

    let mut script = AgentToolsScript {
        opened_session_id: Some(SEEDED_SESSION_ID.into()),
        ..AgentToolsScript::default()
    };
    drive_scripted_effects(&mut host, &mut script)?;

    let state_after_tools: SessionState = host.read_state()?;
    ensure!(
        state_after_tools.lifecycle == SessionLifecycle::WaitingInput,
        "expected WaitingInput after scripted tool + follow-up llm flow, got {:?}; active_tool_batch={:?}; pending_effects={:?}; llm_results={:?}; pending_follow_up={:?}",
        state_after_tools.lifecycle,
        state_after_tools
            .active_tool_batch
            .as_ref()
            .map(|batch| &batch.call_status),
        state_after_tools
            .active_tool_batch
            .as_ref()
            .map(|batch| batch.pending_effects.len()),
        state_after_tools
            .active_tool_batch
            .as_ref()
            .map(|batch| &batch.llm_results),
        state_after_tools.pending_follow_up_turn
    );
    let batch = state_after_tools
        .active_tool_batch
        .as_ref()
        .ok_or_else(|| anyhow!("expected active tool batch"))?;
    ensure!(batch.is_settled(), "expected tool batch settled");
    for call_id in [
        CALL_WRITE,
        CALL_EXISTS,
        CALL_READ,
        CALL_LIST,
        CALL_INSPECT_WORLD,
        CALL_INSPECT_WORKFLOW_STATE,
        CALL_INSPECT_WORKFLOW_CELLS,
        CALL_WORKSPACE_INSPECT,
        CALL_WORKSPACE_LIST_WORKSPACES,
        CALL_WORKSPACE_LIST_TREE,
        CALL_WORKSPACE_READ,
        CALL_WORKSPACE_APPLY,
        CALL_WORKSPACE_COMMIT,
        CALL_WORKSPACE_DIFF,
        CALL_EXEC,
    ] {
        ensure!(
            matches!(
                batch.call_status.get(call_id),
                Some(ToolCallStatus::Succeeded)
            ),
            "expected call {call_id} status Succeeded, got {:?}; llm_result={:?}",
            batch.call_status.get(call_id),
            batch.llm_results.get(call_id)
        );
    }
    let expected_groups = vec![
        vec![
            CALL_WRITE.to_string(),
            CALL_EXISTS.to_string(),
            CALL_READ.to_string(),
            CALL_LIST.to_string(),
            CALL_INSPECT_WORLD.to_string(),
            CALL_INSPECT_WORKFLOW_STATE.to_string(),
            CALL_INSPECT_WORKFLOW_CELLS.to_string(),
            CALL_WORKSPACE_INSPECT.to_string(),
            CALL_WORKSPACE_LIST_WORKSPACES.to_string(),
            CALL_WORKSPACE_LIST_TREE.to_string(),
            CALL_WORKSPACE_READ.to_string(),
        ],
        vec![CALL_WORKSPACE_APPLY.to_string()],
        vec![CALL_WORKSPACE_COMMIT.to_string()],
        vec![CALL_WORKSPACE_DIFF.to_string()],
        vec![CALL_EXEC.to_string()],
    ];
    ensure!(
        batch.plan.execution_plan.groups == expected_groups,
        "unexpected execution grouping: {:?}",
        batch.plan.execution_plan.groups
    );
    let expected_ops = BTreeSet::from([
        TOOL_FS_WRITE.to_string(),
        TOOL_FS_EXISTS.to_string(),
        TOOL_FS_READ.to_string(),
        TOOL_FS_LIST.to_string(),
        TOOL_EXEC.to_string(),
        TOOL_INTROSPECT_MANIFEST.to_string(),
        TOOL_INTROSPECT_WORKFLOW_STATE.to_string(),
        TOOL_INTROSPECT_LIST_CELLS.to_string(),
        TOOL_WORKSPACE_RESOLVE.to_string(),
        TOOL_WORKSPACE_EMPTY_ROOT.to_string(),
        TOOL_WORKSPACE_LIST.to_string(),
        TOOL_WORKSPACE_READ_REF.to_string(),
        TOOL_WORKSPACE_READ_BYTES.to_string(),
        TOOL_WORKSPACE_WRITE_REF.to_string(),
        TOOL_WORKSPACE_DIFF.to_string(),
    ]);
    ensure!(
        script.seen_tool_effect_ops == expected_ops,
        "unexpected seen tool effects: {:?}",
        script.seen_tool_effect_ops
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
        "   tool flow validated: llm_turns={} effects={}",
        script.llm_turn,
        script.seen_tool_effect_ops.len()
    );

    let key = host.single_keyed_cell_key()?;
    host.finish_with_keyed_samples(Some(WORKFLOW_NAME), &[key])?
        .verify_replay()?;
    Ok(())
}

#[derive(Debug, Default)]
struct AgentToolsScript {
    llm_turn: u64,
    seen_tool_effect_ops: BTreeSet<String>,
    opened_session_id: Option<String>,
    workspace_write_ref_calls: u64,
}

impl AgentToolsScript {
    fn handle_intent<S: Store>(
        &mut self,
        store: &S,
        intent: EffectIntent,
    ) -> Result<EffectReceipt> {
        match intent.effect.as_str() {
            effect_ops::BLOB_PUT => self.handle_blob_put(store, intent),
            effect_ops::BLOB_GET => self.handle_blob_get(store, intent),
            effect_ops::LLM_GENERATE => self.handle_llm_generate(store, intent),
            effect_ops::HOST_SESSION_OPEN | TOOL_SESSION_OPEN => {
                self.handle_host_session_open(intent)
            }
            effect_ops::HOST_FS_WRITE_FILE | TOOL_FS_WRITE => self.handle_host_fs_write(intent),
            effect_ops::HOST_FS_EXISTS | TOOL_FS_EXISTS => self.handle_host_fs_exists(intent),
            effect_ops::HOST_FS_READ_FILE | TOOL_FS_READ => self.handle_host_fs_read(intent),
            effect_ops::HOST_FS_LIST_DIR | TOOL_FS_LIST => self.handle_host_fs_list(intent),
            effect_ops::HOST_EXEC | TOOL_EXEC => self.handle_host_exec(intent),
            effect_ops::INTROSPECT_MANIFEST | TOOL_INTROSPECT_MANIFEST => {
                self.handle_introspect_manifest(intent)
            }
            effect_ops::INTROSPECT_WORKFLOW_STATE | TOOL_INTROSPECT_WORKFLOW_STATE => {
                self.handle_introspect_workflow_state(intent)
            }
            effect_ops::INTROSPECT_LIST_CELLS | TOOL_INTROSPECT_LIST_CELLS => {
                self.handle_introspect_list_cells(intent)
            }
            effect_ops::WORKSPACE_RESOLVE | TOOL_WORKSPACE_RESOLVE => {
                self.handle_workspace_resolve(intent)
            }
            effect_ops::WORKSPACE_EMPTY_ROOT | TOOL_WORKSPACE_EMPTY_ROOT => {
                self.handle_workspace_empty_root(intent)
            }
            effect_ops::WORKSPACE_LIST | TOOL_WORKSPACE_LIST => self.handle_workspace_list(intent),
            effect_ops::WORKSPACE_READ_REF | TOOL_WORKSPACE_READ_REF => {
                self.handle_workspace_read_ref(intent)
            }
            effect_ops::WORKSPACE_READ_BYTES | TOOL_WORKSPACE_READ_BYTES => {
                self.handle_workspace_read_bytes(intent)
            }
            effect_ops::WORKSPACE_WRITE_BYTES | TOOL_WORKSPACE_WRITE_BYTES => {
                self.handle_workspace_write_bytes(intent)
            }
            effect_ops::WORKSPACE_WRITE_REF | TOOL_WORKSPACE_WRITE_REF => {
                self.handle_workspace_write_ref(intent)
            }
            effect_ops::WORKSPACE_DIFF | TOOL_WORKSPACE_DIFF => self.handle_workspace_diff(intent),
            other => bail!("unexpected effect in agent-tools smoke: {other}"),
        }
    }

    fn handle_blob_put<S: Store>(
        &mut self,
        store: &S,
        intent: EffectIntent,
    ) -> Result<EffectReceipt> {
        let params: BlobPutParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode blob.put params")?;
        let blob_hash = store
            .put_blob(&params.bytes)
            .context("store blob.put bytes")?;
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
        )
    }

    fn handle_blob_get<S: Store>(
        &mut self,
        store: &S,
        intent: EffectIntent,
    ) -> Result<EffectReceipt> {
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
        )
    }

    fn handle_llm_generate(
        &mut self,
        store: &impl Store,
        intent: EffectIntent,
    ) -> Result<EffectReceipt> {
        let params: LlmGenerateParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode llm.generate params")?;
        ensure!(
            !params.provider.trim().is_empty(),
            "llm provider must be set"
        );
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
        )
    }

    fn handle_host_session_open(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let _params: HostSessionOpenParams = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode host.session.open params")?;

        self.seen_tool_effect_ops.insert(TOOL_SESSION_OPEN.into());
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

        self.seen_tool_effect_ops.insert(TOOL_FS_WRITE.into());
        ok_receipt(
            intent,
            &HostFsWriteFileReceipt {
                status: "ok".into(),
                written_bytes: Some(TEST_FILE_TEXT.len() as u64),
                created: Some(true),
                new_mtime_ns: Some(11),
                error_code: None,
            },
        )
    }

    fn handle_host_fs_exists(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: Value =
            serde_cbor::from_slice(&intent.params_cbor).context("decode host.fs.exists params")?;
        let session_id = require_json_string(&params, "session_id")?;
        self.ensure_runtime_session(session_id.as_str())?;
        let path = require_json_string(&params, "path")?;
        ensure!(
            path == TEST_FILE_PATH,
            "expected exists path {TEST_FILE_PATH}, got {}",
            path
        );

        self.seen_tool_effect_ops.insert(TOOL_FS_EXISTS.into());
        ok_receipt(
            intent,
            &HostFsExistsReceipt {
                status: "ok".into(),
                exists: Some(true),
                error_code: None,
            },
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

        self.seen_tool_effect_ops.insert(TOOL_FS_READ.into());
        ok_receipt(
            intent,
            &json!({
                "status": "ok",
                "content": {
                    "inline_text": {
                        "text": TEST_FILE_TEXT,
                    }
                },
                "truncated": false,
                "size_bytes": TEST_FILE_TEXT.len() as u64,
            }),
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

        self.seen_tool_effect_ops.insert(TOOL_FS_LIST.into());
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

        self.seen_tool_effect_ops.insert(TOOL_EXEC.into());
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
        )
    }

    fn handle_introspect_manifest(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: IntrospectManifestParams = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode introspect.manifest params")?;
        ensure!(
            params.consistency == "head",
            "expected manifest consistency=head, got {}",
            params.consistency
        );

        self.seen_tool_effect_ops
            .insert(TOOL_INTROSPECT_MANIFEST.into());
        ok_receipt(
            intent,
            &IntrospectManifestReceipt {
                manifest: serde_cbor::to_vec(&smoke_manifest())?,
                meta: smoke_read_meta()?,
            },
        )
    }

    fn handle_introspect_workflow_state(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: IntrospectWorkflowStateParams = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode introspect.workflow_state params")?;
        ensure!(
            params.workflow == WORKFLOW_NAME,
            "expected workflow {}, got {}",
            WORKFLOW_NAME,
            params.workflow
        );
        ensure!(
            params.consistency == "head",
            "expected workflow_state consistency=head, got {}",
            params.consistency
        );
        ensure!(
            params.key.is_none(),
            "expected state inspection without key, got {:?}",
            params.key
        );

        self.seen_tool_effect_ops
            .insert(TOOL_INTROSPECT_WORKFLOW_STATE.into());
        ok_receipt(
            intent,
            &IntrospectWorkflowStateReceipt {
                state: Some(serde_cbor::to_vec(&json!({
                    "lifecycle": "WaitingInput",
                    "tool_profile": TOOL_PROFILE,
                }))?),
                meta: smoke_read_meta()?,
            },
        )
    }

    fn handle_introspect_list_cells(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: IntrospectListCellsParams = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode introspect.list_cells params")?;
        if params.workflow == WORKFLOW_NAME {
            self.seen_tool_effect_ops
                .insert(TOOL_INTROSPECT_LIST_CELLS.into());
            return ok_receipt(
                intent,
                &IntrospectListCellsReceipt {
                    cells: vec![IntrospectCellInfo {
                        key: Vec::new(),
                        state_hash: EffectHashRef::new(
                            "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
                        )?,
                        size: 128,
                        last_active_ns: 99,
                    }],
                    meta: smoke_read_meta()?,
                },
            );
        }
        ensure!(
            params.workflow == "sys/Workspace@1",
            "expected workflow {} or sys/Workspace@1, got {}",
            WORKFLOW_NAME,
            params.workflow
        );

        self.seen_tool_effect_ops
            .insert(TOOL_INTROSPECT_LIST_CELLS.into());
        ok_receipt(
            intent,
            &IntrospectListCellsReceipt {
                cells: vec![
                    IntrospectCellInfo {
                        key: serde_cbor::to_vec(&WORKSPACE_ALPHA)?,
                        state_hash: EffectHashRef::new(ALPHA_ROOT_HASH)?,
                        size: 256,
                        last_active_ns: 100,
                    },
                    IntrospectCellInfo {
                        key: serde_cbor::to_vec(&WORKSPACE_BETA)?,
                        state_hash: EffectHashRef::new(BETA_ROOT_HASH)?,
                        size: 192,
                        last_active_ns: 101,
                    },
                ],
                meta: smoke_read_meta()?,
            },
        )
    }

    fn handle_workspace_resolve(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: WorkspaceResolveParams = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode workspace.resolve params")?;
        self.seen_tool_effect_ops
            .insert(TOOL_WORKSPACE_RESOLVE.into());
        let receipt = match params.workspace.as_str() {
            WORKSPACE_ALPHA => WorkspaceResolveReceipt {
                exists: true,
                resolved_version: Some(params.version.unwrap_or(2)),
                head: Some(3),
                root_hash: Some(EffectHashRef::new(ALPHA_ROOT_HASH)?),
            },
            WORKSPACE_BETA => WorkspaceResolveReceipt {
                exists: true,
                resolved_version: Some(params.version.unwrap_or(5)),
                head: Some(5),
                root_hash: Some(EffectHashRef::new(BETA_ROOT_HASH)?),
            },
            WORKSPACE_DRAFT => WorkspaceResolveReceipt {
                exists: false,
                resolved_version: None,
                head: None,
                root_hash: None,
            },
            other => bail!("unexpected workspace.resolve workspace {other}"),
        };
        ok_receipt(intent, &receipt)
    }

    fn handle_workspace_empty_root(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: WorkspaceEmptyRootParams = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode workspace.empty_root params")?;
        ensure!(
            params.workspace == WORKSPACE_DRAFT,
            "expected empty_root for draft, got {}",
            params.workspace
        );
        self.seen_tool_effect_ops
            .insert(TOOL_WORKSPACE_EMPTY_ROOT.into());
        ok_receipt(
            intent,
            &WorkspaceEmptyRootReceipt {
                root_hash: EffectHashRef::new(DRAFT_EMPTY_ROOT_HASH)?,
            },
        )
    }

    fn handle_workspace_list(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: WorkspaceListParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode workspace.list params")?;
        ensure!(
            params.root_hash.as_str() == ALPHA_ROOT_HASH,
            "expected workspace.list root {}, got {}",
            ALPHA_ROOT_HASH,
            params.root_hash
        );
        ensure!(
            params.path.as_deref() == Some("docs"),
            "expected workspace.list path docs, got {:?}",
            params.path
        );
        self.seen_tool_effect_ops.insert(TOOL_WORKSPACE_LIST.into());
        ok_receipt(
            intent,
            &WorkspaceListReceipt {
                entries: vec![aos_effect_types::workspace::WorkspaceListEntry {
                    path: WORKSPACE_FILE_PATH.into(),
                    kind: "file".into(),
                    hash: Some(EffectHashRef::new(
                        "sha256:1212121212121212121212121212121212121212121212121212121212121212",
                    )?),
                    size: Some(WORKSPACE_TEXT.len() as u64),
                    mode: Some(0o644),
                }],
                next_cursor: None,
            },
        )
    }

    fn handle_workspace_read_ref(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: WorkspaceReadRefParams = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode workspace.read_ref params")?;
        ensure!(
            params.root_hash.as_str() == ALPHA_ROOT_HASH,
            "expected workspace.read_ref root {}, got {}",
            ALPHA_ROOT_HASH,
            params.root_hash
        );
        ensure!(
            params.path == WORKSPACE_FILE_PATH,
            "expected workspace.read_ref path {}, got {}",
            WORKSPACE_FILE_PATH,
            params.path
        );
        self.seen_tool_effect_ops
            .insert(TOOL_WORKSPACE_READ_REF.into());
        ok_receipt(
            intent,
            &Some(aos_effect_types::workspace::WorkspaceRefEntry {
                kind: "file".into(),
                hash: EffectHashRef::new(
                    "sha256:1313131313131313131313131313131313131313131313131313131313131313",
                )?,
                size: WORKSPACE_TEXT.len() as u64,
                mode: 0o644,
            }),
        )
    }

    fn handle_workspace_read_bytes(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: WorkspaceReadBytesParams = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode workspace.read_bytes params")?;
        ensure!(
            params.root_hash.as_str() == ALPHA_ROOT_HASH,
            "expected workspace.read_bytes root {}, got {}",
            ALPHA_ROOT_HASH,
            params.root_hash
        );
        ensure!(
            params.path == WORKSPACE_FILE_PATH,
            "expected workspace.read_bytes path {}, got {}",
            WORKSPACE_FILE_PATH,
            params.path
        );
        self.seen_tool_effect_ops
            .insert(TOOL_WORKSPACE_READ_BYTES.into());
        ok_receipt(
            intent,
            &serde_cbor::Value::Bytes(WORKSPACE_TEXT.as_bytes().to_vec()),
        )
    }

    fn handle_workspace_write_bytes(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: WorkspaceWriteBytesParams = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode workspace.write_bytes params")?;
        ensure!(
            params.root_hash.as_str() == DRAFT_EMPTY_ROOT_HASH,
            "expected workspace.write_bytes root {}, got {}",
            DRAFT_EMPTY_ROOT_HASH,
            params.root_hash
        );
        ensure!(
            params.path == "draft.txt",
            "expected workspace.write_bytes path draft.txt, got {}",
            params.path
        );
        self.seen_tool_effect_ops
            .insert(TOOL_WORKSPACE_WRITE_BYTES.into());
        ok_receipt(
            intent,
            &WorkspaceWriteBytesReceipt {
                new_root_hash: EffectHashRef::new(DRAFT_TEXT_ROOT_HASH)?,
                blob_hash: EffectHashRef::new(
                    "sha256:1414141414141414141414141414141414141414141414141414141414141414",
                )?,
            },
        )
    }

    fn handle_workspace_write_ref(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: WorkspaceWriteRefParams = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode workspace.write_ref params")?;
        self.seen_tool_effect_ops
            .insert(TOOL_WORKSPACE_WRITE_REF.into());
        self.workspace_write_ref_calls = self.workspace_write_ref_calls.saturating_add(1);

        match self.workspace_write_ref_calls {
            1 => {
                ensure!(
                    params.root_hash.as_str() == DRAFT_EMPTY_ROOT_HASH,
                    "expected first workspace.write_ref root {}, got {}",
                    DRAFT_EMPTY_ROOT_HASH,
                    params.root_hash
                );
                ensure!(
                    params.path == "draft.txt",
                    "expected first workspace.write_ref path draft.txt, got {}",
                    params.path
                );
                ok_receipt(
                    intent,
                    &WorkspaceWriteRefReceipt {
                        new_root_hash: EffectHashRef::new(DRAFT_TEXT_ROOT_HASH)?,
                        blob_hash: params.blob_hash,
                    },
                )
            }
            2 => {
                ensure!(
                    params.root_hash.as_str() == DRAFT_TEXT_ROOT_HASH,
                    "expected second workspace.write_ref root {}, got {}",
                    DRAFT_TEXT_ROOT_HASH,
                    params.root_hash
                );
                ensure!(
                    params.path == "linked.bin",
                    "expected second workspace.write_ref path linked.bin, got {}",
                    params.path
                );
                ensure!(
                    params.blob_hash.as_str() == APPLY_BLOB_HASH,
                    "expected second workspace.write_ref blob {}, got {}",
                    APPLY_BLOB_HASH,
                    params.blob_hash
                );
                ok_receipt(
                    intent,
                    &WorkspaceWriteRefReceipt {
                        new_root_hash: EffectHashRef::new(DRAFT_FINAL_ROOT_HASH)?,
                        blob_hash: EffectHashRef::new(APPLY_BLOB_HASH)?,
                    },
                )
            }
            other => bail!("unexpected workspace.write_ref call count {other}"),
        }
    }

    fn handle_workspace_diff(&mut self, intent: EffectIntent) -> Result<EffectReceipt> {
        let params: WorkspaceDiffParams =
            serde_cbor::from_slice(&intent.params_cbor).context("decode workspace.diff params")?;
        ensure!(
            params.root_a.as_str() == ALPHA_ROOT_HASH,
            "expected workspace.diff left {}, got {}",
            ALPHA_ROOT_HASH,
            params.root_a
        );
        ensure!(
            params.root_b.as_str() == BETA_ROOT_HASH,
            "expected workspace.diff right {}, got {}",
            BETA_ROOT_HASH,
            params.root_b
        );
        self.seen_tool_effect_ops.insert(TOOL_WORKSPACE_DIFF.into());
        ok_receipt(
            intent,
            &WorkspaceDiffReceipt {
                changes: vec![aos_effect_types::workspace::WorkspaceDiffChange {
                    path: WORKSPACE_FILE_PATH.into(),
                    kind: "modified".into(),
                    old_hash: Some(EffectHashRef::new(
                        "sha256:1515151515151515151515151515151515151515151515151515151515151515",
                    )?),
                    new_hash: Some(EffectHashRef::new(
                        "sha256:1616161616161616161616161616161616161616161616161616161616161616",
                    )?),
                }],
            },
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
        TOOL_INTROSPECT_MANIFEST.to_string(),
        TOOL_INTROSPECT_WORKFLOW_STATE.to_string(),
        "workspace.inspect".to_string(),
        TOOL_WORKSPACE_LIST.to_string(),
        "workspace.read".to_string(),
        "workspace.apply".to_string(),
        "workspace.commit".to_string(),
        TOOL_WORKSPACE_DIFF.to_string(),
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
        host.kernel_mut().tick_until_idle()?;
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

fn build_first_llm_output_ref(store: &impl Store) -> Result<HashRef> {
    let tool_calls = build_scripted_tool_calls(store)?;
    let tool_calls_ref = store_json_blob(store, &serde_json::to_value(tool_calls)?)?;

    let envelope = LlmOutputEnvelope {
        assistant_text: None,
        tool_calls_ref: Some(tool_calls_ref),
        reasoning_ref: None,
    };
    store_json_blob(store, &serde_json::to_value(envelope)?)
}

fn build_second_llm_output_ref(store: &impl Store) -> Result<HashRef> {
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

fn build_scripted_tool_calls(store: &impl Store) -> Result<LlmToolCallList> {
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
    let inspect_world_args = store_json_blob(store, &json!({}))?;
    let inspect_workflow_state_args = store_json_blob(
        store,
        &json!({
            "workflow": WORKFLOW_NAME,
            "view": "state"
        }),
    )?;
    let inspect_workflow_cells_args = store_json_blob(
        store,
        &json!({
            "workflow": WORKFLOW_NAME,
            "view": "cells"
        }),
    )?;
    let workspace_inspect_args = store_json_blob(
        store,
        &json!({
            "workspace": WORKSPACE_ALPHA
        }),
    )?;
    let workspace_list_workspaces_args = store_json_blob(store, &json!({}))?;
    let workspace_list_tree_args = store_json_blob(
        store,
        &json!({
            "workspace": WORKSPACE_ALPHA,
            "path": "docs"
        }),
    )?;
    let workspace_read_args = store_json_blob(
        store,
        &json!({
            "workspace": WORKSPACE_ALPHA,
            "path": WORKSPACE_FILE_PATH
        }),
    )?;
    let workspace_apply_args = store_json_blob(
        store,
        &json!({
            "workspace": WORKSPACE_DRAFT,
            "operations": [
                {
                    "op": "write",
                    "path": "draft.txt",
                    "text": "draft body"
                },
                {
                    "op": "write",
                    "path": "linked.bin",
                    "blob_hash": APPLY_BLOB_HASH
                }
            ]
        }),
    )?;
    let workspace_diff_args = store_json_blob(
        store,
        &json!({
            "left": { "root_hash": ALPHA_ROOT_HASH },
            "right": { "workspace": WORKSPACE_BETA }
        }),
    )?;
    let workspace_commit_args = store_json_blob(
        store,
        &json!({
            "workspace": WORKSPACE_DRAFT,
            "root_hash": DRAFT_FINAL_ROOT_HASH,
            "owner": "agent-tools"
        }),
    )?;

    Ok(vec![
        LlmToolCall {
            call_id: CALL_WRITE.into(),
            tool_name: LLM_TOOL_FS_WRITE.into(),
            arguments_ref: write_args,
            provider_call_id: Some("provider-call-write".into()),
        },
        LlmToolCall {
            call_id: CALL_EXISTS.into(),
            tool_name: LLM_TOOL_FS_EXISTS.into(),
            arguments_ref: exists_args,
            provider_call_id: Some("provider-call-exists".into()),
        },
        LlmToolCall {
            call_id: CALL_READ.into(),
            tool_name: LLM_TOOL_FS_READ.into(),
            arguments_ref: read_args,
            provider_call_id: Some("provider-call-read".into()),
        },
        LlmToolCall {
            call_id: CALL_LIST.into(),
            tool_name: LLM_TOOL_FS_LIST.into(),
            arguments_ref: list_args,
            provider_call_id: Some("provider-call-list".into()),
        },
        LlmToolCall {
            call_id: CALL_INSPECT_WORLD.into(),
            tool_name: LLM_TOOL_INSPECT_WORLD.into(),
            arguments_ref: inspect_world_args,
            provider_call_id: Some("provider-call-inspect-world".into()),
        },
        LlmToolCall {
            call_id: CALL_INSPECT_WORKFLOW_STATE.into(),
            tool_name: LLM_TOOL_INSPECT_WORKFLOW.into(),
            arguments_ref: inspect_workflow_state_args,
            provider_call_id: Some("provider-call-inspect-workflow-state".into()),
        },
        LlmToolCall {
            call_id: CALL_INSPECT_WORKFLOW_CELLS.into(),
            tool_name: LLM_TOOL_INSPECT_WORKFLOW.into(),
            arguments_ref: inspect_workflow_cells_args,
            provider_call_id: Some("provider-call-inspect-workflow-cells".into()),
        },
        LlmToolCall {
            call_id: CALL_WORKSPACE_INSPECT.into(),
            tool_name: LLM_TOOL_WORKSPACE_INSPECT.into(),
            arguments_ref: workspace_inspect_args,
            provider_call_id: Some("provider-call-workspace-inspect".into()),
        },
        LlmToolCall {
            call_id: CALL_WORKSPACE_LIST_WORKSPACES.into(),
            tool_name: LLM_TOOL_WORKSPACE_LIST.into(),
            arguments_ref: workspace_list_workspaces_args,
            provider_call_id: Some("provider-call-workspace-list-workspaces".into()),
        },
        LlmToolCall {
            call_id: CALL_WORKSPACE_LIST_TREE.into(),
            tool_name: LLM_TOOL_WORKSPACE_LIST.into(),
            arguments_ref: workspace_list_tree_args,
            provider_call_id: Some("provider-call-workspace-list-tree".into()),
        },
        LlmToolCall {
            call_id: CALL_WORKSPACE_READ.into(),
            tool_name: LLM_TOOL_WORKSPACE_READ.into(),
            arguments_ref: workspace_read_args,
            provider_call_id: Some("provider-call-workspace-read".into()),
        },
        LlmToolCall {
            call_id: CALL_WORKSPACE_APPLY.into(),
            tool_name: LLM_TOOL_WORKSPACE_APPLY.into(),
            arguments_ref: workspace_apply_args,
            provider_call_id: Some("provider-call-workspace-apply".into()),
        },
        LlmToolCall {
            call_id: CALL_WORKSPACE_COMMIT.into(),
            tool_name: LLM_TOOL_WORKSPACE_COMMIT.into(),
            arguments_ref: workspace_commit_args,
            provider_call_id: Some("provider-call-workspace-commit".into()),
        },
        LlmToolCall {
            call_id: CALL_WORKSPACE_DIFF.into(),
            tool_name: LLM_TOOL_WORKSPACE_DIFF.into(),
            arguments_ref: workspace_diff_args,
            provider_call_id: Some("provider-call-workspace-diff".into()),
        },
        LlmToolCall {
            call_id: CALL_EXEC.into(),
            tool_name: LLM_TOOL_EXEC.into(),
            arguments_ref: exec_args,
            provider_call_id: Some("provider-call-exec".into()),
        },
    ])
}

fn store_json_blob(store: &impl Store, value: &Value) -> Result<HashRef> {
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

fn ok_receipt<T: Serialize>(intent: EffectIntent, payload: &T) -> Result<EffectReceipt> {
    Ok(EffectReceipt {
        intent_hash: intent.intent_hash,
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(payload).context("encode receipt payload")?,
        cost_cents: Some(0),
        signature: vec![0; 64],
    })
}

fn smoke_read_meta() -> Result<ReadMeta> {
    Ok(ReadMeta {
        journal_height: 12,
        snapshot_hash: None,
        manifest_hash: EffectHashRef::new(
            "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        )?,
    })
}

fn smoke_manifest() -> Manifest {
    Manifest {
        air_version: aos_air_types::CURRENT_AIR_VERSION.into(),
        schemas: vec![],
        modules: vec![],
        ops: vec![],
        workflows: vec![],
        effects: vec![],
        secrets: vec![],
        routing: None,
    }
}
