use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effect_adapters::{
    adapters::host::make_fabric_host_adapter_set,
    config::FabricAdapterConfig,
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
use serde::{Serialize, de::DeserializeOwned};
use serde_cbor::Value as CborValue;

#[tokio::test(flavor = "current_thread")]
async fn fabric_controller_live_host_adapter_e2e() -> anyhow::Result<()> {
    let Some(mut config) = live_fabric_config() else {
        eprintln!("skipping Fabric live e2e: set AOS_FABRIC_E2E=1 to enable");
        return Ok(());
    };
    config.exec_progress_interval = Duration::from_secs(1);

    let store = Arc::new(MemStore::new());
    let adapters = make_fabric_host_adapter_set(store.clone(), config);
    let image = env_or(
        "AOS_FABRIC_IMAGE",
        "AOS_FABRIC_DEFAULT_IMAGE",
        "alpine:latest",
    );
    let runtime_class = optional_env("AOS_FABRIC_RUNTIME_CLASS")
        .or_else(|| optional_env("AOS_FABRIC_DEFAULT_RUNTIME_CLASS"));
    let network_mode = optional_env("AOS_FABRIC_NETWORK_MODE")
        .or_else(|| optional_env("AOS_FABRIC_DEFAULT_NETWORK_MODE"))
        .unwrap_or_else(|| "egress".to_string());
    let prefix = format!("aos-fabric-e2e-{}", now_ns());

    let open_payload: HostSessionOpenReceipt = execute_ok(
        &adapters.session_open,
        EffectKind::HOST_SESSION_OPEN,
        &HostSessionOpenParams {
            target: HostTarget::sandbox(HostSandboxTarget {
                image,
                runtime_class,
                workdir: Some("/workspace".to_string()),
                env: None,
                network_mode: Some(network_mode),
                mounts: None,
                cpu_limit_millis: Some(1_000),
                memory_limit_bytes: Some(536_870_912),
            }),
            session_ttl_ns: Some(120_000_000_000),
            labels: Some(BTreeMap::from([(
                "aos-fabric-e2e".to_string(),
                prefix.clone(),
            )])),
        },
        1,
    )
    .await;
    assert_eq!(open_payload.status, "ready");
    let session_id = open_payload.session_id;

    let text_path = format!("{prefix}/hello.txt");
    let binary_path = format!("{prefix}/blob.bin");
    let patch_path = format!("{prefix}/patched.txt");

    let write_text: HostFsWriteFileReceipt = execute_ok(
        &adapters.fs_write_file,
        EffectKind::HOST_FS_WRITE_FILE,
        &HostFsWriteFileParams {
            session_id: session_id.clone(),
            path: text_path.clone(),
            content: HostFileContentInput::InlineText {
                inline_text: HostInlineText {
                    text: "alpha\nneedle\n".to_string(),
                },
            },
            mode: Some("overwrite".to_string()),
            create_parents: Some(true),
        },
        2,
    )
    .await;
    assert_eq!(write_text.status, "ok");
    assert_eq!(
        write_text.written_bytes,
        Some("alpha\nneedle\n".len() as u64)
    );

    let read_text: HostFsReadFileReceipt = execute_ok(
        &adapters.fs_read_file,
        EffectKind::HOST_FS_READ_FILE,
        &HostFsReadFileParams {
            session_id: session_id.clone(),
            path: text_path.clone(),
            offset_bytes: None,
            max_bytes: None,
            encoding: Some("utf8".to_string()),
            output_mode: Some("require_inline".to_string()),
        },
        3,
    )
    .await;
    assert_eq!(read_text.status, "ok");
    assert_eq!(
        host_output_text(read_text.content.as_ref().unwrap(), store.as_ref()),
        "alpha\nneedle\n"
    );

    let binary = vec![0, 159, 255, b'\n'];
    let write_binary: HostFsWriteFileReceipt = execute_ok(
        &adapters.fs_write_file,
        EffectKind::HOST_FS_WRITE_FILE,
        &HostFsWriteFileParams {
            session_id: session_id.clone(),
            path: binary_path.clone(),
            content: HostFileContentInput::InlineBytes {
                inline_bytes: HostInlineBytes {
                    bytes: binary.clone(),
                },
            },
            mode: Some("overwrite".to_string()),
            create_parents: Some(true),
        },
        4,
    )
    .await;
    assert_eq!(write_binary.status, "ok");

    let read_binary: HostFsReadFileReceipt = execute_ok(
        &adapters.fs_read_file,
        EffectKind::HOST_FS_READ_FILE,
        &HostFsReadFileParams {
            session_id: session_id.clone(),
            path: binary_path.clone(),
            offset_bytes: None,
            max_bytes: None,
            encoding: Some("bytes".to_string()),
            output_mode: Some("require_inline".to_string()),
        },
        5,
    )
    .await;
    assert_eq!(read_binary.status, "ok");
    assert_eq!(
        host_output_bytes(read_binary.content.as_ref().unwrap(), store.as_ref()),
        binary
    );

    let exists: HostFsExistsReceipt = execute_ok(
        &adapters.fs_exists,
        EffectKind::HOST_FS_EXISTS,
        &HostFsExistsParams {
            session_id: session_id.clone(),
            path: text_path.clone(),
        },
        6,
    )
    .await;
    assert_eq!(exists.exists, Some(true));

    let stat: HostFsStatReceipt = execute_ok(
        &adapters.fs_stat,
        EffectKind::HOST_FS_STAT,
        &HostFsStatParams {
            session_id: session_id.clone(),
            path: text_path.clone(),
        },
        7,
    )
    .await;
    assert_eq!(stat.exists, Some(true));
    assert_eq!(stat.is_dir, Some(false));

    let grep: HostFsGrepReceipt = execute_ok(
        &adapters.fs_grep,
        EffectKind::HOST_FS_GREP,
        &HostFsGrepParams {
            session_id: session_id.clone(),
            pattern: "needle".to_string(),
            path: Some(prefix.clone()),
            glob_filter: Some("*.txt".to_string()),
            max_results: Some(10),
            case_insensitive: None,
            output_mode: Some("require_inline".to_string()),
        },
        8,
    )
    .await;
    assert_eq!(grep.status, "ok");
    assert_eq!(grep.match_count, Some(1));
    assert!(text_output(grep.matches.as_ref().unwrap(), store.as_ref()).contains("needle"));

    let glob: HostFsGlobReceipt = execute_ok(
        &adapters.fs_glob,
        EffectKind::HOST_FS_GLOB,
        &HostFsGlobParams {
            session_id: session_id.clone(),
            pattern: "*.txt".to_string(),
            path: Some(prefix.clone()),
            max_results: Some(10),
            output_mode: Some("require_inline".to_string()),
        },
        9,
    )
    .await;
    assert_eq!(glob.status, "ok");
    assert!(text_output(glob.paths.as_ref().unwrap(), store.as_ref()).contains("hello.txt"));

    let list_dir: HostFsListDirReceipt = execute_ok(
        &adapters.fs_list_dir,
        EffectKind::HOST_FS_LIST_DIR,
        &HostFsListDirParams {
            session_id: session_id.clone(),
            path: Some(prefix.clone()),
            max_results: Some(20),
            output_mode: Some("require_inline".to_string()),
        },
        10,
    )
    .await;
    assert_eq!(list_dir.status, "ok");
    let listed = text_output(list_dir.entries.as_ref().unwrap(), store.as_ref());
    assert!(listed.contains("hello.txt"));
    assert!(listed.contains("blob.bin"));

    let edit: HostFsEditFileReceipt = execute_ok(
        &adapters.fs_edit_file,
        EffectKind::HOST_FS_EDIT_FILE,
        &HostFsEditFileParams {
            session_id: session_id.clone(),
            path: text_path.clone(),
            old_string: "needle".to_string(),
            new_string: "needle-edited".to_string(),
            replace_all: Some(false),
        },
        11,
    )
    .await;
    assert_eq!(edit.status, "ok");
    assert_eq!(edit.replacements, Some(1));

    let patch = format!("*** Begin Patch\n*** Add File: {patch_path}\n+patched ok\n*** End Patch");
    let patch_receipt: HostFsApplyPatchReceipt = execute_ok(
        &adapters.fs_apply_patch,
        EffectKind::HOST_FS_APPLY_PATCH,
        &HostFsApplyPatchParams {
            session_id: session_id.clone(),
            patch: HostPatchInput::InlineText {
                inline_text: HostInlineText { text: patch },
            },
            patch_format: Some("v4a".to_string()),
            dry_run: Some(false),
        },
        12,
    )
    .await;
    assert_eq!(patch_receipt.status, "ok");
    assert_eq!(patch_receipt.files_changed, Some(1));

    let stdin_hash = store.put_blob(&binary)?;
    let cat: HostExecReceipt = execute_ok(
        &adapters.exec,
        EffectKind::HOST_EXEC,
        &HostExecParams {
            session_id: session_id.clone(),
            argv: vec!["cat".to_string()],
            cwd: Some("/workspace".to_string()),
            timeout_ns: Some(20_000_000_000),
            env_patch: None,
            stdin_ref: Some(HashRef::new(stdin_hash.to_hex())?),
            output_mode: Some("require_inline".to_string()),
        },
        13,
    )
    .await;
    assert_eq!(cat.status, "ok");
    assert_eq!(cat.exit_code, 0);
    assert_eq!(
        host_output_bytes(cat.stdout.as_ref().unwrap(), store.as_ref()),
        binary
    );

    let (progress_frames, long_receipt) =
        run_long_exec_with_progress(&adapters.exec, &session_id, store.as_ref()).await?;
    assert!(
        !progress_frames.is_empty(),
        "expected at least one host.exec.progress frame"
    );
    assert_eq!(long_receipt.status, "ok");
    assert_eq!(long_receipt.exit_code, 0);
    assert!(
        host_output_text(long_receipt.stdout.as_ref().unwrap(), store.as_ref())
            .contains("e2e-done")
    );

    let signal: HostSessionSignalReceipt = execute_ok(
        &adapters.session_signal,
        EffectKind::HOST_SESSION_SIGNAL,
        &HostSessionSignalParams {
            session_id,
            signal: "close".to_string(),
            grace_timeout_ns: None,
        },
        14,
    )
    .await;
    assert_eq!(signal.status, "closed");

    Ok(())
}

async fn run_long_exec_with_progress<A: AsyncEffectAdapter + ?Sized>(
    adapter: &A,
    session_id: &str,
    store: &MemStore,
) -> anyhow::Result<(Vec<HostExecProgressFrame>, HostExecReceipt)> {
    let params = HostExecParams {
        session_id: session_id.to_string(),
        argv: vec![
            "sh".to_string(),
            "-lc".to_string(),
            "printf 'e2e-progress\\n'; sleep 2; printf 'e2e-done\\n'".to_string(),
        ],
        cwd: Some("/workspace".to_string()),
        timeout_ns: Some(20_000_000_000),
        env_patch: None,
        stdin_ref: None,
        output_mode: Some("require_inline".to_string()),
    };
    let intent = intent_for(EffectKind::HOST_EXEC, &params, 64);
    let context = AdapterStartContext {
        origin_module_id: "live/FabricHostE2E@1".to_string(),
        origin_workflow_op_hash: None,
        origin_instance_key: None,
        effect_op: "sys/host.exec@1".to_string(),
        effect_op_hash: None,
        executor_module: Some("sys/Host@1".to_string()),
        executor_module_hash: None,
        executor_entrypoint: Some(EffectKind::HOST_EXEC.to_string()),
        emitted_at_seq: 1,
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    adapter
        .ensure_started_with_context(intent, Some(context), tx)
        .await?;

    let mut frames = Vec::new();
    let mut receipt = None;
    while let Some(update) = rx.recv().await {
        match update {
            EffectUpdate::StreamFrame(frame) => {
                assert_eq!(frame.adapter_id, "host.exec.fabric");
                assert_eq!(frame.kind, "host.exec.progress");
                frames.push(serde_cbor::from_slice::<HostExecProgressFrame>(
                    &frame.payload_cbor,
                )?);
            }
            EffectUpdate::Receipt(next) => {
                assert_eq!(next.status, ReceiptStatus::Ok);
                receipt = Some(serde_cbor::from_slice::<HostExecReceipt>(
                    &next.payload_cbor,
                )?);
            }
        }
    }

    assert!(
        frames
            .iter()
            .any(|frame| frame.stdout_bytes > 0 || !frame.stdout_delta.is_empty()),
        "expected at least one progress frame with stdout progress"
    );
    let receipt = receipt.expect("terminal receipt");
    assert_eq!(
        host_output_text(receipt.stdout.as_ref().unwrap(), store),
        "e2e-progress\ne2e-done\n"
    );
    Ok((frames, receipt))
}

async fn execute_ok<A, P, R>(adapter: &A, kind: &str, params: &P, seed: u8) -> R
where
    A: AsyncEffectAdapter + ?Sized,
    P: Serialize,
    R: DeserializeOwned,
{
    let intent = intent_for(kind, params, seed);
    let receipt = adapter.execute(&intent).await.expect("execute adapter");
    assert_eq!(
        receipt.status,
        ReceiptStatus::Ok,
        "{kind} returned {:?}: {:?}",
        receipt.status,
        serde_cbor::from_slice::<CborValue>(&receipt.payload_cbor)
            .unwrap_or(CborValue::Text("<invalid cbor>".to_string()))
    );
    serde_cbor::from_slice(&receipt.payload_cbor).expect("decode receipt payload")
}

fn intent_for<P: Serialize>(kind: &str, params: &P, seed: u8) -> EffectIntent {
    EffectIntent::from_raw_params(
        EffectKind::new(kind),
        serde_cbor::to_vec(params).expect("encode params"),
        [seed; 32],
    )
    .expect("intent")
}

fn live_fabric_config() -> Option<FabricAdapterConfig> {
    if std::env::var("AOS_FABRIC_E2E").as_deref() != Ok("1") {
        return None;
    }
    Some(
        FabricAdapterConfig::from_env()
            .expect("AOS_FABRIC_E2E=1 requires AOS_FABRIC_CONTROLLER_URL"),
    )
}

fn optional_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_or(key: &str, fallback_key: &str, default: &str) -> String {
    optional_env(key)
        .or_else(|| optional_env(fallback_key))
        .unwrap_or_else(|| default.to_string())
}

fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn host_output_text(output: &HostOutput, store: &MemStore) -> String {
    String::from_utf8(host_output_bytes(output, store)).expect("utf8 host output")
}

fn host_output_bytes(output: &HostOutput, store: &MemStore) -> Vec<u8> {
    match output {
        HostOutput::InlineText { inline_text } => inline_text.text.as_bytes().to_vec(),
        HostOutput::InlineBytes { inline_bytes } => inline_bytes.bytes.clone(),
        HostOutput::Blob { blob } => {
            let hash = Hash::from_hex_str(blob.blob_ref.as_str()).expect("blob hash");
            store.get_blob(hash).expect("blob payload")
        }
    }
}

fn text_output(output: &HostTextOutput, store: &MemStore) -> String {
    match output {
        HostTextOutput::InlineText { inline_text } => inline_text.text.clone(),
        HostTextOutput::Blob { blob } => {
            let hash = Hash::from_hex_str(blob.blob_ref.as_str()).expect("blob hash");
            String::from_utf8(store.get_blob(hash).expect("blob payload")).expect("utf8 blob")
        }
    }
}
