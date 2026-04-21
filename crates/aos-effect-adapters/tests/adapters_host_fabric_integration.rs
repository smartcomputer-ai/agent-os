use std::{
    collections::BTreeMap,
    convert::Infallible,
    net::{SocketAddr, TcpListener},
    sync::{Arc, Mutex},
    time::Duration,
};

use aos_cbor::Hash;
use aos_effect_adapters::{
    adapters::host::{make_fabric_host_adapter_set, make_host_adapter_set},
    config::{AdapterProviderSpec, EffectAdapterConfig, FabricAdapterConfig},
    default_registry,
    traits::{AdapterStartContext, AsyncEffectAdapter, EffectUpdate},
};
use aos_effects::{
    EffectIntent, EffectKind, ReceiptStatus,
    builtins::{
        HostExecParams, HostExecProgressFrame, HostExecReceipt, HostFileContentInput,
        HostFsApplyPatchParams, HostFsApplyPatchReceipt, HostFsEditFileParams,
        HostFsEditFileReceipt, HostFsExistsParams, HostFsExistsReceipt, HostFsGlobParams,
        HostFsGlobReceipt, HostFsGrepParams, HostFsGrepReceipt, HostFsListDirParams,
        HostFsListDirReceipt, HostFsReadFileParams, HostFsReadFileReceipt, HostFsStatParams,
        HostFsStatReceipt, HostFsWriteFileParams, HostFsWriteFileReceipt, HostInlineBytes,
        HostInlineText, HostOutput, HostPatchInput, HostSandboxTarget, HostSessionOpenParams,
        HostSessionOpenReceipt, HostSessionSignalParams, HostSessionSignalReceipt, HostTarget,
        HostTextOutput,
    },
};
use aos_kernel::{MemStore, Store};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use fabric_protocol::{
    ControllerExecRequest, ControllerSessionOpenRequest, ControllerSessionOpenResponse,
    ControllerSessionStatus, ControllerSessionSummary, ControllerSignalSessionRequest, ExecEvent,
    ExecEventKind, ExecId, FabricBytes, FabricSessionTargetKind, FsApplyPatchRequest,
    FsApplyPatchResponse, FsDirEntry, FsEditFileRequest, FsEditFileResponse, FsEntryKind,
    FsExistsResponse, FsFileReadResponse, FsFileWriteRequest, FsGlobRequest, FsGlobResponse,
    FsGrepMatch, FsGrepRequest, FsGrepResponse, FsListDirResponse, FsPatchOpsSummary, FsPathQuery,
    FsStatResponse, FsWriteResponse, HostId, SessionId,
};
use globset::Glob;
use tokio::task::JoinHandle;

fn build_intent(kind: &str, params_cbor: Vec<u8>, seed: u8) -> EffectIntent {
    EffectIntent::from_raw_params(EffectKind::new(kind), params_cbor, [seed; 32]).unwrap()
}

#[test]
fn default_registry_registers_fabric_providers_when_configured() {
    let store = Arc::new(MemStore::new());
    let mut config = EffectAdapterConfig {
        fabric: Some(test_fabric_config("http://127.0.0.1:1")),
        ..EffectAdapterConfig::default()
    };
    config.adapter_routes.insert(
        "host.exec.sandbox".to_string(),
        AdapterProviderSpec {
            adapter_kind: "host.exec.fabric".to_string(),
        },
    );
    let registry = default_registry(store, &config);

    assert!(registry.get("host.session.open.fabric").is_some());
    assert!(registry.get("host.exec.fabric").is_some());
    assert!(registry.get("host.session.signal.fabric").is_some());
    assert!(registry.get("host.fs.read_file.fabric").is_some());
    assert!(registry.get("host.fs.write_file.fabric").is_some());
    assert!(registry.get("host.fs.edit_file.fabric").is_some());
    assert!(registry.get("host.fs.apply_patch.fabric").is_some());
    assert!(registry.get("host.fs.grep.fabric").is_some());
    assert!(registry.get("host.fs.glob.fabric").is_some());
    assert!(registry.get("host.fs.stat.fabric").is_some());
    assert!(registry.get("host.fs.exists.fabric").is_some());
    assert!(registry.get("host.fs.list_dir.fabric").is_some());
    assert!(registry.has_route("host.exec.sandbox"));
}

#[tokio::test]
async fn fabric_host_adapters_open_exec_signal_and_binary_fs_roundtrip() {
    let fake = FakeFabricController::start().await;
    let store = Arc::new(MemStore::new());
    let adapters = make_fabric_host_adapter_set(store.clone(), test_fabric_config(&fake.base_url));

    let open_params = HostSessionOpenParams {
        target: HostTarget::sandbox(HostSandboxTarget {
            image: "docker.io/library/alpine:latest".to_string(),
            runtime_class: Some("smolvm".to_string()),
            workdir: Some("/workspace".to_string()),
            env: Some(BTreeMap::from([("A".to_string(), "B".to_string())])),
            network_mode: Some("egress".to_string()),
            mounts: None,
            cpu_limit_millis: Some(1_000),
            memory_limit_bytes: Some(268_435_456),
        }),
        session_ttl_ns: Some(60_000_000_000),
        labels: Some(BTreeMap::from([("world".to_string(), "test".to_string())])),
    };
    let open_intent = build_intent(
        EffectKind::HOST_SESSION_OPEN,
        serde_cbor::to_vec(&open_params).unwrap(),
        1,
    );
    let open_receipt = adapters.session_open.execute(&open_intent).await.unwrap();
    assert_eq!(open_receipt.status, ReceiptStatus::Ok);
    let open_payload: HostSessionOpenReceipt =
        serde_cbor::from_slice(&open_receipt.payload_cbor).unwrap();
    assert_eq!(open_payload.session_id, "fabric-session-1");
    assert_eq!(open_payload.status, "ready");
    let open_request = fake.state.open_requests.lock().unwrap()[0].clone();
    assert_eq!(
        open_request.request_id.as_ref().map(|id| id.0.as_str()),
        Some(format!("aos:{}", hex::encode(open_intent.intent_hash)).as_str())
    );

    let stdin_bytes = vec![0, 159, 255, b'\n'];
    let stdin_hash = store.put_blob(&stdin_bytes).unwrap();
    let exec_params = HostExecParams {
        session_id: open_payload.session_id.clone(),
        argv: vec!["cat".to_string()],
        cwd: None,
        timeout_ns: None,
        env_patch: None,
        stdin_ref: Some(aos_air_types::HashRef::new(stdin_hash.to_hex()).unwrap()),
        output_mode: Some("require_inline".to_string()),
    };
    let exec_intent = build_intent(
        EffectKind::HOST_EXEC,
        serde_cbor::to_vec(&exec_params).unwrap(),
        2,
    );
    let exec_receipt = adapters.exec.execute(&exec_intent).await.unwrap();
    assert_eq!(exec_receipt.status, ReceiptStatus::Ok);
    let exec_payload: HostExecReceipt = serde_cbor::from_slice(&exec_receipt.payload_cbor).unwrap();
    assert_eq!(exec_payload.status, "ok");
    assert_eq!(exec_payload.exit_code, 0);
    assert_eq!(
        output_bytes(exec_payload.stdout.as_ref().unwrap(), store.as_ref()),
        stdin_bytes
    );
    let exec_request = fake.state.exec_requests.lock().unwrap()[0].clone();
    assert_eq!(
        exec_request.request_id.as_ref().map(|id| id.0.as_str()),
        Some(format!("aos:{}", hex::encode(exec_intent.intent_hash)).as_str())
    );

    let file_bytes = vec![255, 0, b'f'];
    let write_params = HostFsWriteFileParams {
        session_id: open_payload.session_id.clone(),
        path: "blob.bin".to_string(),
        content: HostFileContentInput::InlineBytes {
            inline_bytes: HostInlineBytes {
                bytes: file_bytes.clone(),
            },
        },
        mode: Some("overwrite".to_string()),
        create_parents: Some(true),
    };
    let write_receipt = adapters
        .fs_write_file
        .execute(&build_intent(
            EffectKind::HOST_FS_WRITE_FILE,
            serde_cbor::to_vec(&write_params).unwrap(),
            3,
        ))
        .await
        .unwrap();
    assert_eq!(write_receipt.status, ReceiptStatus::Ok);
    let write_payload: HostFsWriteFileReceipt =
        serde_cbor::from_slice(&write_receipt.payload_cbor).unwrap();
    assert_eq!(write_payload.status, "ok");
    assert_eq!(write_payload.written_bytes, Some(file_bytes.len() as u64));

    let read_params = HostFsReadFileParams {
        session_id: open_payload.session_id.clone(),
        path: "blob.bin".to_string(),
        offset_bytes: None,
        max_bytes: None,
        encoding: Some("bytes".to_string()),
        output_mode: Some("require_inline".to_string()),
    };
    let read_receipt = adapters
        .fs_read_file
        .execute(&build_intent(
            EffectKind::HOST_FS_READ_FILE,
            serde_cbor::to_vec(&read_params).unwrap(),
            4,
        ))
        .await
        .unwrap();
    assert_eq!(read_receipt.status, ReceiptStatus::Ok);
    let read_payload: HostFsReadFileReceipt =
        serde_cbor::from_slice(&read_receipt.payload_cbor).unwrap();
    assert_eq!(read_payload.status, "ok");
    assert_eq!(
        output_bytes(read_payload.content.as_ref().unwrap(), store.as_ref()),
        file_bytes
    );
    assert_eq!(read_payload.mtime_ns, Some(123));

    let source_text = b"pub fn main() {}\nfn helper() {}\n".to_vec();
    let write_source_params = HostFsWriteFileParams {
        session_id: open_payload.session_id.clone(),
        path: "src/main.rs".to_string(),
        content: HostFileContentInput::InlineBytes {
            inline_bytes: HostInlineBytes {
                bytes: source_text.clone(),
            },
        },
        mode: Some("overwrite".to_string()),
        create_parents: Some(true),
    };
    let _ = adapters
        .fs_write_file
        .execute(&build_intent(
            EffectKind::HOST_FS_WRITE_FILE,
            serde_cbor::to_vec(&write_source_params).unwrap(),
            10,
        ))
        .await
        .unwrap();

    let exists_receipt = adapters
        .fs_exists
        .execute(&build_intent(
            EffectKind::HOST_FS_EXISTS,
            serde_cbor::to_vec(&HostFsExistsParams {
                session_id: open_payload.session_id.clone(),
                path: "src/main.rs".to_string(),
            })
            .unwrap(),
            11,
        ))
        .await
        .unwrap();
    assert_eq!(exists_receipt.status, ReceiptStatus::Ok);
    let exists_payload: HostFsExistsReceipt =
        serde_cbor::from_slice(&exists_receipt.payload_cbor).unwrap();
    assert_eq!(exists_payload.status, "ok");
    assert_eq!(exists_payload.exists, Some(true));

    let stat_receipt = adapters
        .fs_stat
        .execute(&build_intent(
            EffectKind::HOST_FS_STAT,
            serde_cbor::to_vec(&HostFsStatParams {
                session_id: open_payload.session_id.clone(),
                path: "src".to_string(),
            })
            .unwrap(),
            12,
        ))
        .await
        .unwrap();
    assert_eq!(stat_receipt.status, ReceiptStatus::Ok);
    let stat_payload: HostFsStatReceipt =
        serde_cbor::from_slice(&stat_receipt.payload_cbor).unwrap();
    assert_eq!(stat_payload.status, "ok");
    assert_eq!(stat_payload.exists, Some(true));
    assert_eq!(stat_payload.is_dir, Some(true));

    let list_receipt = adapters
        .fs_list_dir
        .execute(&build_intent(
            EffectKind::HOST_FS_LIST_DIR,
            serde_cbor::to_vec(&HostFsListDirParams {
                session_id: open_payload.session_id.clone(),
                path: Some("src".to_string()),
                max_results: None,
                output_mode: Some("require_inline".to_string()),
            })
            .unwrap(),
            13,
        ))
        .await
        .unwrap();
    assert_eq!(list_receipt.status, ReceiptStatus::Ok);
    let list_payload: HostFsListDirReceipt =
        serde_cbor::from_slice(&list_receipt.payload_cbor).unwrap();
    assert_eq!(list_payload.status, "ok");
    assert_eq!(list_payload.count, Some(1));
    assert_eq!(
        output_text(list_payload.entries.as_ref().unwrap(), store.as_ref()),
        "main.rs"
    );

    let edit_receipt = adapters
        .fs_edit_file
        .execute(&build_intent(
            EffectKind::HOST_FS_EDIT_FILE,
            serde_cbor::to_vec(&HostFsEditFileParams {
                session_id: open_payload.session_id.clone(),
                path: "src/main.rs".to_string(),
                old_string: "helper".to_string(),
                new_string: "helper2".to_string(),
                replace_all: Some(false),
            })
            .unwrap(),
            14,
        ))
        .await
        .unwrap();
    assert_eq!(edit_receipt.status, ReceiptStatus::Ok);
    let edit_payload: HostFsEditFileReceipt =
        serde_cbor::from_slice(&edit_receipt.payload_cbor).unwrap();
    assert_eq!(edit_payload.status, "ok");
    assert_eq!(edit_payload.replacements, Some(1));
    assert_eq!(edit_payload.applied, Some(true));

    let grep_receipt = adapters
        .fs_grep
        .execute(&build_intent(
            EffectKind::HOST_FS_GREP,
            serde_cbor::to_vec(&HostFsGrepParams {
                session_id: open_payload.session_id.clone(),
                pattern: "fn".to_string(),
                path: Some("src".to_string()),
                glob_filter: Some("*.rs".to_string()),
                max_results: Some(10),
                case_insensitive: None,
                output_mode: Some("require_inline".to_string()),
            })
            .unwrap(),
            15,
        ))
        .await
        .unwrap();
    assert_eq!(grep_receipt.status, ReceiptStatus::Ok);
    let grep_payload: HostFsGrepReceipt =
        serde_cbor::from_slice(&grep_receipt.payload_cbor).unwrap();
    assert_eq!(grep_payload.status, "ok");
    assert_eq!(grep_payload.match_count, Some(2));
    assert_eq!(
        output_text(grep_payload.matches.as_ref().unwrap(), store.as_ref()),
        "/workspace/src/main.rs:1:pub fn main() {}\n/workspace/src/main.rs:2:fn helper2() {}"
    );

    let glob_receipt = adapters
        .fs_glob
        .execute(&build_intent(
            EffectKind::HOST_FS_GLOB,
            serde_cbor::to_vec(&HostFsGlobParams {
                session_id: open_payload.session_id.clone(),
                pattern: "*.rs".to_string(),
                path: Some("src".to_string()),
                max_results: None,
                output_mode: Some("require_inline".to_string()),
            })
            .unwrap(),
            16,
        ))
        .await
        .unwrap();
    assert_eq!(glob_receipt.status, ReceiptStatus::Ok);
    let glob_payload: HostFsGlobReceipt =
        serde_cbor::from_slice(&glob_receipt.payload_cbor).unwrap();
    assert_eq!(glob_payload.status, "ok");
    assert_eq!(glob_payload.count, Some(1));
    assert_eq!(
        output_text(glob_payload.paths.as_ref().unwrap(), store.as_ref()),
        "/workspace/src/main.rs"
    );

    let patch_text = "\
*** Begin Patch
*** Add File: src/lib.rs
+pub fn lib() {}
*** End Patch";
    let apply_patch_receipt = adapters
        .fs_apply_patch
        .execute(&build_intent(
            EffectKind::HOST_FS_APPLY_PATCH,
            serde_cbor::to_vec(&HostFsApplyPatchParams {
                session_id: open_payload.session_id.clone(),
                patch: HostPatchInput::InlineText {
                    inline_text: HostInlineText {
                        text: patch_text.to_string(),
                    },
                },
                patch_format: Some("v4a".to_string()),
                dry_run: Some(false),
            })
            .unwrap(),
            17,
        ))
        .await
        .unwrap();
    assert_eq!(apply_patch_receipt.status, ReceiptStatus::Ok);
    let apply_patch_payload: HostFsApplyPatchReceipt =
        serde_cbor::from_slice(&apply_patch_receipt.payload_cbor).unwrap();
    assert_eq!(apply_patch_payload.status, "ok");
    assert_eq!(apply_patch_payload.files_changed, Some(1));
    assert_eq!(
        apply_patch_payload.changed_paths,
        Some(vec!["/workspace/src/lib.rs".to_string()])
    );
    assert_eq!(apply_patch_payload.ops.as_ref().map(|ops| ops.add), Some(1));

    let signal_params = HostSessionSignalParams {
        session_id: open_payload.session_id,
        signal: "close".to_string(),
        grace_timeout_ns: None,
    };
    let signal_receipt = adapters
        .session_signal
        .execute(&build_intent(
            EffectKind::HOST_SESSION_SIGNAL,
            serde_cbor::to_vec(&signal_params).unwrap(),
            5,
        ))
        .await
        .unwrap();
    assert_eq!(signal_receipt.status, ReceiptStatus::Ok);
    let signal_payload: HostSessionSignalReceipt =
        serde_cbor::from_slice(&signal_receipt.payload_cbor).unwrap();
    assert_eq!(signal_payload.status, "closed");
}

#[tokio::test]
async fn fabric_exec_async_start_emits_progress_frames_before_terminal_receipt() {
    let fake = FakeFabricController::start().await;
    let store = Arc::new(MemStore::new());
    let mut config = test_fabric_config(&fake.base_url);
    config.exec_progress_interval = Duration::from_millis(50);
    let adapters = make_fabric_host_adapter_set(store.clone(), config);

    let exec_params = HostExecParams {
        session_id: "fabric-session-1".to_string(),
        argv: vec!["slow".to_string()],
        cwd: None,
        timeout_ns: None,
        env_patch: None,
        stdin_ref: None,
        output_mode: Some("require_inline".to_string()),
    };
    let exec_intent = build_intent(
        EffectKind::HOST_EXEC,
        serde_cbor::to_vec(&exec_params).unwrap(),
        6,
    );
    let context = AdapterStartContext {
        origin_module_id: "com.acme/Workflow@1".to_string(),
        origin_instance_key: Some(vec![1, 2, 3]),
        effect_kind: EffectKind::HOST_EXEC.to_string(),
        emitted_at_seq: 42,
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);

    adapters
        .exec
        .ensure_started_with_context(exec_intent.clone(), Some(context.clone()), tx)
        .await
        .unwrap();

    let mut updates = Vec::new();
    while let Some(update) = rx.recv().await {
        updates.push(update);
    }

    let first_frame_index = updates
        .iter()
        .position(|update| matches!(update, EffectUpdate::StreamFrame(_)))
        .expect("progress frame");
    let receipt_index = updates
        .iter()
        .position(|update| matches!(update, EffectUpdate::Receipt(_)))
        .expect("terminal receipt");
    assert!(first_frame_index < receipt_index);
    let EffectUpdate::StreamFrame(first_frame) = &updates[first_frame_index] else {
        unreachable!();
    };
    assert_eq!(first_frame.intent_hash, exec_intent.intent_hash);
    assert_eq!(first_frame.adapter_id, "host.exec.fabric");
    assert_eq!(first_frame.origin_module_id, context.origin_module_id);
    assert_eq!(first_frame.origin_instance_key, context.origin_instance_key);
    assert_eq!(first_frame.effect_kind, context.effect_kind);
    assert_eq!(first_frame.emitted_at_seq, context.emitted_at_seq);
    assert_eq!(first_frame.seq, 1);
    assert_eq!(first_frame.kind, "host.exec.progress");
    let progress_payload: HostExecProgressFrame =
        serde_cbor::from_slice(&first_frame.payload_cbor).unwrap();
    assert_eq!(progress_payload.exec_id.as_deref(), Some("fake-exec"));
    assert_eq!(progress_payload.stdout_delta, b"slow-stdout\n");
    assert_eq!(progress_payload.stderr_delta, b"");
    assert_eq!(progress_payload.stdout_bytes, "slow-stdout\n".len() as u64);

    let EffectUpdate::Receipt(receipt) = &updates[receipt_index] else {
        unreachable!();
    };
    assert_eq!(receipt.status, ReceiptStatus::Ok);
    let payload: HostExecReceipt = serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    assert_eq!(payload.exit_code, 0);
    assert_eq!(
        output_bytes(payload.stdout.as_ref().unwrap(), store.as_ref()),
        b"slow-stdout\n"
    );
}

#[tokio::test]
async fn fabric_exec_async_start_fast_exec_emits_only_terminal_receipt() {
    let fake = FakeFabricController::start().await;
    let store = Arc::new(MemStore::new());
    let adapters = make_fabric_host_adapter_set(store.clone(), test_fabric_config(&fake.base_url));

    let exec_params = HostExecParams {
        session_id: "fabric-session-1".to_string(),
        argv: vec!["quick".to_string()],
        cwd: None,
        timeout_ns: None,
        env_patch: None,
        stdin_ref: None,
        output_mode: Some("require_inline".to_string()),
    };
    let exec_intent = build_intent(
        EffectKind::HOST_EXEC,
        serde_cbor::to_vec(&exec_params).unwrap(),
        7,
    );
    let context = AdapterStartContext {
        origin_module_id: "com.acme/Workflow@1".to_string(),
        origin_instance_key: None,
        effect_kind: EffectKind::HOST_EXEC.to_string(),
        emitted_at_seq: 43,
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);

    adapters
        .exec
        .ensure_started_with_context(exec_intent, Some(context), tx)
        .await
        .unwrap();

    let mut frame_count = 0;
    let mut receipt = None;
    while let Some(update) = rx.recv().await {
        match update {
            EffectUpdate::StreamFrame(_) => frame_count += 1,
            EffectUpdate::Receipt(next) => receipt = Some(next),
        }
    }

    assert_eq!(frame_count, 0);
    let receipt = receipt.unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Ok);
    let payload: HostExecReceipt = serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    assert_eq!(
        output_bytes(payload.stdout.as_ref().unwrap(), store.as_ref()),
        b"fake-stdout\n"
    );
}

#[tokio::test]
async fn local_host_adapter_rejects_sandbox_target() {
    let store = Arc::new(MemStore::new());
    let adapters = make_host_adapter_set(store);
    let open_params = HostSessionOpenParams {
        target: HostTarget::sandbox(HostSandboxTarget {
            image: "docker.io/library/alpine:latest".to_string(),
            runtime_class: None,
            workdir: None,
            env: None,
            network_mode: Some("egress".to_string()),
            mounts: None,
            cpu_limit_millis: None,
            memory_limit_bytes: None,
        }),
        session_ttl_ns: None,
        labels: None,
    };
    let receipt = adapters
        .session_open
        .execute(&build_intent(
            EffectKind::HOST_SESSION_OPEN,
            serde_cbor::to_vec(&open_params).unwrap(),
            9,
        ))
        .await
        .unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Error);
    let payload: HostSessionOpenReceipt = serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    assert_eq!(payload.error_code.as_deref(), Some("unsupported_target"));
}

fn test_fabric_config(controller_url: &str) -> FabricAdapterConfig {
    FabricAdapterConfig {
        controller_url: controller_url.to_string(),
        bearer_token: None,
        request_timeout: std::time::Duration::from_secs(30),
        exec_progress_interval: std::time::Duration::from_secs(10),
        default_image: None,
        default_runtime_class: None,
        default_network_mode: None,
    }
}

fn output_bytes(output: &HostOutput, store: &MemStore) -> Vec<u8> {
    match output {
        HostOutput::InlineText { inline_text } => inline_text.text.as_bytes().to_vec(),
        HostOutput::InlineBytes { inline_bytes } => inline_bytes.bytes.clone(),
        HostOutput::Blob { blob } => {
            let hash = Hash::from_hex_str(blob.blob_ref.as_str()).unwrap();
            store.get_blob(hash).unwrap()
        }
    }
}

fn output_text(output: &HostTextOutput, store: &MemStore) -> String {
    match output {
        HostTextOutput::InlineText { inline_text } => inline_text.text.clone(),
        HostTextOutput::Blob { blob } => {
            let hash = Hash::from_hex_str(blob.blob_ref.as_str()).unwrap();
            String::from_utf8(store.get_blob(hash).unwrap()).unwrap()
        }
    }
}

#[derive(Default)]
struct FakeFabricControllerState {
    open_requests: Mutex<Vec<ControllerSessionOpenRequest>>,
    exec_requests: Mutex<Vec<ControllerExecRequest>>,
    files: Mutex<BTreeMap<String, Vec<u8>>>,
}

struct FakeFabricController {
    base_url: String,
    state: Arc<FakeFabricControllerState>,
    _task: JoinHandle<()>,
}

impl FakeFabricController {
    async fn start() -> Self {
        let state = Arc::new(FakeFabricControllerState::default());
        let router = Router::new()
            .route("/v1/sessions", post(open_session))
            .route("/v1/sessions/{session_id}/exec", post(exec_session))
            .route("/v1/sessions/{session_id}/signal", post(signal_session))
            .route(
                "/v1/sessions/{session_id}/fs/file",
                get(read_file).put(write_file),
            )
            .route("/v1/sessions/{session_id}/fs/exists", get(exists))
            .route("/v1/sessions/{session_id}/fs/stat", get(stat))
            .route("/v1/sessions/{session_id}/fs/edit", post(edit_file))
            .route(
                "/v1/sessions/{session_id}/fs/apply_patch",
                post(apply_patch),
            )
            .route("/v1/sessions/{session_id}/fs/list_dir", get(list_dir))
            .route("/v1/sessions/{session_id}/fs/grep", post(grep))
            .route("/v1/sessions/{session_id}/fs/glob", post(glob))
            .with_state(state.clone());
        let addr = free_loopback_addr();
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            base_url: format!("http://{addr}"),
            state,
            _task: task,
        }
    }
}

async fn open_session(
    State(state): State<Arc<FakeFabricControllerState>>,
    Json(request): Json<ControllerSessionOpenRequest>,
) -> Json<ControllerSessionOpenResponse> {
    state.open_requests.lock().unwrap().push(request);
    Json(ControllerSessionOpenResponse {
        session_id: SessionId("fabric-session-1".to_string()),
        status: ControllerSessionStatus::Ready,
        target_kind: FabricSessionTargetKind::Sandbox,
        host_id: HostId("fake-host".to_string()),
        host_session_id: SessionId("host-session-1".to_string()),
        workdir: "/workspace".to_string(),
        supported_signals: Vec::new(),
        created_at_ns: 1,
        expires_at_ns: Some(61_000_000_000),
    })
}

async fn exec_session(
    State(state): State<Arc<FakeFabricControllerState>>,
    Path(_session_id): Path<String>,
    Json(request): Json<ControllerExecRequest>,
) -> impl IntoResponse {
    let slow = request.argv.first().map(String::as_str) == Some("slow");
    let stdout = request
        .stdin
        .as_ref()
        .map(|stdin| FabricBytes::from(stdin.clone()).decode_bytes().unwrap())
        .unwrap_or_else(|| {
            if slow {
                b"slow-stdout\n".to_vec()
            } else {
                b"fake-stdout\n".to_vec()
            }
        });
    state.exec_requests.lock().unwrap().push(request);
    let exec_id = ExecId("fake-exec".to_string());
    let events = vec![
        ExecEvent {
            exec_id: exec_id.clone(),
            seq: 0,
            kind: ExecEventKind::Started,
            data: None,
            exit_code: None,
            message: None,
        },
        ExecEvent {
            exec_id: exec_id.clone(),
            seq: 1,
            kind: ExecEventKind::Stdout,
            data: Some(FabricBytes::from_bytes_auto(stdout.clone())),
            exit_code: None,
            message: None,
        },
        ExecEvent {
            exec_id,
            seq: 2,
            kind: ExecEventKind::Exit,
            data: None,
            exit_code: Some(0),
            message: None,
        },
    ];
    let body = if slow {
        let stdout = stdout.clone();
        Body::from_stream(futures_util::stream::unfold(0, move |step| {
            let events = events.clone();
            let stdout = stdout.clone();
            async move {
                match step {
                    0 => Some((Ok::<Bytes, Infallible>(event_line(&events[0])), 1)),
                    1 => Some((
                        Ok::<Bytes, Infallible>(event_line(&ExecEvent {
                            data: Some(FabricBytes::from_bytes_auto(stdout)),
                            ..events[1].clone()
                        })),
                        2,
                    )),
                    2 => {
                        tokio::time::sleep(Duration::from_millis(120)).await;
                        Some((Ok::<Bytes, Infallible>(event_line(&events[2])), 3))
                    }
                    _ => None,
                }
            }
        }))
    } else {
        let mut body = String::new();
        for event in events {
            body.push_str(&serde_json::to_string(&event).unwrap());
            body.push('\n');
        }
        Body::from(body)
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        body,
    )
}

fn event_line(event: &ExecEvent) -> Bytes {
    Bytes::from(format!("{}\n", serde_json::to_string(event).unwrap()))
}

async fn signal_session(
    Path(session_id): Path<String>,
    Json(_request): Json<ControllerSignalSessionRequest>,
) -> Json<ControllerSessionSummary> {
    Json(ControllerSessionSummary {
        session_id: SessionId(session_id),
        status: ControllerSessionStatus::Closed,
        target_kind: FabricSessionTargetKind::Sandbox,
        host_id: HostId("fake-host".to_string()),
        host_session_id: SessionId("host-session-1".to_string()),
        workdir: Some("/workspace".to_string()),
        supported_signals: Vec::new(),
        labels: BTreeMap::new(),
        created_at_ns: 1,
        updated_at_ns: 2,
        expires_at_ns: None,
        closed_at_ns: Some(2),
    })
}

async fn write_file(
    State(state): State<Arc<FakeFabricControllerState>>,
    Path(_session_id): Path<String>,
    Json(request): Json<FsFileWriteRequest>,
) -> Json<FsWriteResponse> {
    let bytes = request.content.decode_bytes().unwrap();
    let bytes_written = bytes.len() as u64;
    state
        .files
        .lock()
        .unwrap()
        .insert(request.path.clone(), bytes);
    Json(FsWriteResponse {
        path: request.path,
        bytes_written,
    })
}

async fn read_file(
    State(state): State<Arc<FakeFabricControllerState>>,
    Path(_session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Json<FsFileReadResponse> {
    let bytes = state
        .files
        .lock()
        .unwrap()
        .get(&query.path)
        .cloned()
        .unwrap_or_default();
    Json(FsFileReadResponse {
        path: query.path,
        content: FabricBytes::from_bytes_auto(bytes.clone()),
        offset_bytes: 0,
        bytes_read: bytes.len() as u64,
        size_bytes: bytes.len() as u64,
        truncated: false,
        mtime_ns: Some(123),
    })
}

async fn exists(
    State(state): State<Arc<FakeFabricControllerState>>,
    Path(_session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Json<FsExistsResponse> {
    let path = normalize_rel_path(&query.path);
    let files = state.files.lock().unwrap();
    Json(FsExistsResponse {
        path: query.path.clone(),
        exists: files.contains_key(&path) || is_dir_path(&files, &path),
    })
}

async fn stat(
    State(state): State<Arc<FakeFabricControllerState>>,
    Path(_session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Json<FsStatResponse> {
    let path = normalize_rel_path(&query.path);
    let files = state.files.lock().unwrap();
    let (kind, size_bytes) = match files.get(&path) {
        Some(bytes) => (FsEntryKind::File, bytes.len() as u64),
        None if is_dir_path(&files, &path) => (FsEntryKind::Directory, 0),
        None => (FsEntryKind::File, 0),
    };
    Json(FsStatResponse {
        path: query.path,
        kind,
        size_bytes,
        readonly: false,
        mtime_ns: Some(123),
    })
}

async fn edit_file(
    State(state): State<Arc<FakeFabricControllerState>>,
    Path(_session_id): Path<String>,
    Json(request): Json<FsEditFileRequest>,
) -> Json<FsEditFileResponse> {
    let path = normalize_rel_path(&request.path);
    let mut files = state.files.lock().unwrap();
    let bytes = files.get(&path).cloned().unwrap_or_default();
    let content = String::from_utf8(bytes).unwrap();
    let replacements = if request.replace_all {
        content.matches(&request.old_string).count()
    } else {
        usize::from(content.contains(&request.old_string))
    };
    let updated = if request.replace_all {
        content.replace(&request.old_string, &request.new_string)
    } else {
        content.replacen(&request.old_string, &request.new_string, 1)
    };
    files.insert(path.clone(), updated.into_bytes());
    Json(FsEditFileResponse {
        path: workspace_path(&path),
        replacements: replacements as u64,
        applied: true,
    })
}

async fn apply_patch(
    State(state): State<Arc<FakeFabricControllerState>>,
    Path(_session_id): Path<String>,
    Json(request): Json<FsApplyPatchRequest>,
) -> Json<FsApplyPatchResponse> {
    let path = request
        .patch
        .lines()
        .find_map(|line| line.strip_prefix("*** Add File: "))
        .unwrap_or("src/lib.rs")
        .to_string();
    let body = request
        .patch
        .lines()
        .filter_map(|line| line.strip_prefix('+'))
        .collect::<Vec<_>>()
        .join("\n");
    if !request.dry_run {
        state
            .files
            .lock()
            .unwrap()
            .insert(normalize_rel_path(&path), body.into_bytes());
    }
    Json(FsApplyPatchResponse {
        files_changed: 1,
        changed_paths: vec![workspace_path(&normalize_rel_path(&path))],
        ops: FsPatchOpsSummary {
            add: 1,
            update: 0,
            delete: 0,
            move_count: 0,
        },
        applied: !request.dry_run,
    })
}

async fn list_dir(
    State(state): State<Arc<FakeFabricControllerState>>,
    Path(_session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Json<FsListDirResponse> {
    let dir = normalize_rel_path(&query.path);
    let files = state.files.lock().unwrap();
    let mut entries = BTreeMap::new();
    for (file_path, bytes) in files.iter() {
        let Some(rest) = path_under_dir(file_path, &dir) else {
            continue;
        };
        let Some(name) = rest.split('/').next().filter(|name| !name.is_empty()) else {
            continue;
        };
        let child_path = join_rel_path(&dir, name);
        let is_dir = rest.contains('/');
        entries
            .entry(name.to_string())
            .or_insert_with(|| FsDirEntry {
                name: name.to_string(),
                path: workspace_path(&child_path),
                kind: if is_dir {
                    FsEntryKind::Directory
                } else {
                    FsEntryKind::File
                },
                size_bytes: if is_dir { 0 } else { bytes.len() as u64 },
                readonly: false,
            });
    }
    Json(FsListDirResponse {
        path: workspace_path(&dir),
        entries: entries.into_values().collect(),
    })
}

async fn grep(
    State(state): State<Arc<FakeFabricControllerState>>,
    Path(_session_id): Path<String>,
    Json(request): Json<FsGrepRequest>,
) -> Json<FsGrepResponse> {
    let base = normalize_rel_path(request.path.as_deref().unwrap_or("."));
    let max_results = request.max_results.unwrap_or(u64::MAX) as usize;
    let glob = request
        .glob_filter
        .as_deref()
        .map(|filter| Glob::new(filter).unwrap().compile_matcher());
    let needle = if request.case_insensitive {
        request.pattern.to_lowercase()
    } else {
        request.pattern
    };
    let files = state.files.lock().unwrap();
    let mut matches = Vec::new();
    let mut truncated = false;
    for (file_path, bytes) in files.iter() {
        let Some(rest) = path_under_base(file_path, &base) else {
            continue;
        };
        if let Some(glob) = &glob {
            if !glob.is_match(rest) {
                continue;
            }
        }
        let content = String::from_utf8_lossy(bytes);
        for (line_index, line) in content.lines().enumerate() {
            let haystack = if request.case_insensitive {
                line.to_lowercase()
            } else {
                line.to_string()
            };
            if !haystack.contains(&needle) {
                continue;
            }
            if matches.len() >= max_results {
                truncated = true;
                break;
            }
            matches.push(FsGrepMatch {
                path: workspace_path(file_path),
                line_number: line_index as u64 + 1,
                line: line.to_string(),
            });
        }
        if truncated {
            break;
        }
    }
    let match_count = matches.len() as u64;
    Json(FsGrepResponse {
        matches,
        match_count,
        truncated,
    })
}

async fn glob(
    State(state): State<Arc<FakeFabricControllerState>>,
    Path(_session_id): Path<String>,
    Json(request): Json<FsGlobRequest>,
) -> Json<FsGlobResponse> {
    let base = normalize_rel_path(request.path.as_deref().unwrap_or("."));
    let max_results = request.max_results.unwrap_or(u64::MAX) as usize;
    let matcher = Glob::new(&request.pattern).unwrap().compile_matcher();
    let files = state.files.lock().unwrap();
    let mut paths = Vec::new();
    let mut truncated = false;
    for file_path in files.keys() {
        let Some(rest) = path_under_base(file_path, &base) else {
            continue;
        };
        if !matcher.is_match(rest) {
            continue;
        }
        if paths.len() >= max_results {
            truncated = true;
            break;
        }
        paths.push(workspace_path(file_path));
    }
    paths.sort();
    let count = paths.len() as u64;
    Json(FsGlobResponse {
        paths,
        count,
        truncated,
    })
}

fn normalize_rel_path(path: &str) -> String {
    let path = path.strip_prefix("/workspace/").unwrap_or(path);
    let path = path
        .trim_start_matches('/')
        .trim_end_matches('/')
        .trim_start_matches("./");
    if path == "." {
        String::new()
    } else {
        path.to_string()
    }
}

fn join_rel_path(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else {
        format!("{base}/{name}")
    }
}

fn workspace_path(path: &str) -> String {
    if path.is_empty() {
        "/workspace".to_string()
    } else {
        format!("/workspace/{path}")
    }
}

fn path_under_dir<'a>(file_path: &'a str, dir: &str) -> Option<&'a str> {
    if dir.is_empty() {
        Some(file_path)
    } else {
        file_path.strip_prefix(&format!("{dir}/"))
    }
}

fn path_under_base<'a>(file_path: &'a str, base: &str) -> Option<&'a str> {
    if base.is_empty() {
        Some(file_path)
    } else if file_path == base {
        file_path.rsplit('/').next()
    } else {
        file_path.strip_prefix(&format!("{base}/"))
    }
}

fn is_dir_path(files: &BTreeMap<String, Vec<u8>>, path: &str) -> bool {
    path.is_empty()
        || files
            .keys()
            .any(|file_path| file_path.starts_with(&format!("{path}/")))
}

fn free_loopback_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}
