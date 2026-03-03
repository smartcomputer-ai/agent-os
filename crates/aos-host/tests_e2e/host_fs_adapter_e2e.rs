//! End-to-end tests for host filesystem adapters under one host session.

#![cfg(feature = "e2e-tests")]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aos_cbor::Hash;
use aos_effects::builtins::{
    HostExecParams, HostExecReceipt, HostFileContentInput, HostFsApplyPatchParams,
    HostFsApplyPatchReceipt, HostFsEditFileParams, HostFsEditFileReceipt, HostFsGlobParams,
    HostFsGlobReceipt, HostFsGrepParams, HostFsGrepReceipt, HostFsReadFileParams,
    HostFsReadFileReceipt, HostFsWriteFileParams, HostFsWriteFileReceipt, HostInlineText,
    HostLocalTarget, HostOutput, HostPatchInput, HostSessionOpenParams, HostSessionOpenReceipt,
    HostTarget, HostTextOutput,
};
use aos_effects::{EffectIntent, EffectKind, ReceiptStatus};
use aos_host::adapters::host::{HostAdapterSet, make_host_adapter_set};
use aos_host::adapters::traits::AsyncEffectAdapter;
use aos_store::{MemStore, Store};
use tempfile::TempDir;

fn build_intent(kind: &str, params_cbor: Vec<u8>, seed: u8) -> EffectIntent {
    EffectIntent::from_raw_params(EffectKind::new(kind), "cap", params_cbor, [seed; 32]).unwrap()
}

fn shell_available() -> bool {
    Path::new("/bin/sh").exists()
}

async fn open_session(set: &HostAdapterSet<MemStore>, workdir: &Path, seed: u8) -> String {
    let params = HostSessionOpenParams {
        target: HostTarget {
            local: Some(HostLocalTarget {
                mounts: None,
                workdir: Some(workdir.to_string_lossy().to_string()),
                env: None,
                network_mode: "none".into(),
            }),
        },
        session_ttl_ns: None,
        labels: None,
    };
    let receipt = set
        .session_open
        .execute(&build_intent(
            EffectKind::HOST_SESSION_OPEN,
            serde_cbor::to_vec(&params).unwrap(),
            seed,
        ))
        .await
        .unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Ok);
    let payload: HostSessionOpenReceipt = serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    assert_eq!(payload.status, "ready");
    payload.session_id
}

fn decode_text_output(output: &HostTextOutput, store: &MemStore) -> String {
    match output {
        HostTextOutput::InlineText { inline_text } => inline_text.text.clone(),
        HostTextOutput::Blob { blob } => {
            let hash = Hash::from_hex_str(blob.blob_ref.as_str()).unwrap();
            let bytes = store.get_blob(hash).unwrap();
            String::from_utf8(bytes).unwrap()
        }
    }
}

#[tokio::test]
async fn host_fs_session_flow_roundtrip_and_large_output_posture_expected() {
    if !shell_available() {
        eprintln!(
            "skipping host_fs_session_flow_roundtrip_and_large_output_posture_expected: /bin/sh not available"
        );
        return;
    }

    let tmp = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let outside_file = outside.path().join("outside.txt");
    fs::write(&outside_file, "outside").unwrap();

    let store = Arc::new(MemStore::new());
    let set = make_host_adapter_set(store.clone());
    let session_id = open_session(&set, tmp.path(), 1).await;

    let write_params = HostFsWriteFileParams {
        session_id: session_id.clone(),
        path: "notes.txt".into(),
        content: HostFileContentInput::InlineText {
            inline_text: HostInlineText {
                text: "hello from session\n".into(),
            },
        },
        create_parents: Some(false),
        mode: Some("overwrite".into()),
    };
    let write_receipt = set
        .fs_write_file
        .execute(&build_intent(
            EffectKind::HOST_FS_WRITE_FILE,
            serde_cbor::to_vec(&write_params).unwrap(),
            2,
        ))
        .await
        .unwrap();
    let write_payload: HostFsWriteFileReceipt =
        serde_cbor::from_slice(&write_receipt.payload_cbor).unwrap();
    assert_eq!(write_payload.status, "ok");

    let read_params = HostFsReadFileParams {
        session_id: session_id.clone(),
        path: "notes.txt".into(),
        offset_bytes: None,
        max_bytes: None,
        encoding: Some("utf8".into()),
        output_mode: Some("require_inline".into()),
    };
    let read_receipt = set
        .fs_read_file
        .execute(&build_intent(
            EffectKind::HOST_FS_READ_FILE,
            serde_cbor::to_vec(&read_params).unwrap(),
            3,
        ))
        .await
        .unwrap();
    let read_payload: HostFsReadFileReceipt =
        serde_cbor::from_slice(&read_receipt.payload_cbor).unwrap();
    assert_eq!(read_payload.status, "ok");
    match read_payload.content {
        Some(HostOutput::InlineText { inline_text }) => {
            assert_eq!(inline_text.text, "hello from session\n");
        }
        other => panic!("expected inline text read, got {other:?}"),
    }

    let edit_params = HostFsEditFileParams {
        session_id: session_id.clone(),
        path: "notes.txt".into(),
        old_string: "hello from session".into(),
        new_string: "edited in session".into(),
        replace_all: Some(false),
    };
    let edit_receipt = set
        .fs_edit_file
        .execute(&build_intent(
            EffectKind::HOST_FS_EDIT_FILE,
            serde_cbor::to_vec(&edit_params).unwrap(),
            4,
        ))
        .await
        .unwrap();
    let edit_payload: HostFsEditFileReceipt =
        serde_cbor::from_slice(&edit_receipt.payload_cbor).unwrap();
    assert_eq!(edit_payload.status, "ok");

    let patch_text = "\
*** Begin Patch
*** Add File: patched.txt
+patched
*** End Patch";
    let patch_params = HostFsApplyPatchParams {
        session_id: session_id.clone(),
        patch: HostPatchInput::InlineText {
            inline_text: HostInlineText {
                text: patch_text.into(),
            },
        },
        patch_format: Some("v4a".into()),
        dry_run: Some(false),
    };
    let patch_receipt = set
        .fs_apply_patch
        .execute(&build_intent(
            EffectKind::HOST_FS_APPLY_PATCH,
            serde_cbor::to_vec(&patch_params).unwrap(),
            5,
        ))
        .await
        .unwrap();
    let patch_payload: HostFsApplyPatchReceipt =
        serde_cbor::from_slice(&patch_receipt.payload_cbor).unwrap();
    assert_eq!(patch_payload.status, "ok");

    let grep_params = HostFsGrepParams {
        session_id: session_id.clone(),
        pattern: "edited".into(),
        path: None,
        glob_filter: Some("*.txt".into()),
        case_insensitive: Some(false),
        max_results: Some(10),
        output_mode: Some("require_inline".into()),
    };
    let grep_receipt = set
        .fs_grep
        .execute(&build_intent(
            EffectKind::HOST_FS_GREP,
            serde_cbor::to_vec(&grep_params).unwrap(),
            6,
        ))
        .await
        .unwrap();
    let grep_payload: HostFsGrepReceipt =
        serde_cbor::from_slice(&grep_receipt.payload_cbor).unwrap();
    assert_eq!(grep_payload.status, "ok");
    assert_eq!(grep_payload.match_count, Some(1));

    let glob_params = HostFsGlobParams {
        session_id: session_id.clone(),
        pattern: "*.txt".into(),
        path: None,
        max_results: Some(10),
        output_mode: Some("require_inline".into()),
    };
    let glob_receipt = set
        .fs_glob
        .execute(&build_intent(
            EffectKind::HOST_FS_GLOB,
            serde_cbor::to_vec(&glob_params).unwrap(),
            7,
        ))
        .await
        .unwrap();
    let glob_payload: HostFsGlobReceipt =
        serde_cbor::from_slice(&glob_receipt.payload_cbor).unwrap();
    assert_eq!(glob_payload.status, "ok");
    let paths_text = decode_text_output(glob_payload.paths.as_ref().unwrap(), store.as_ref());
    assert!(paths_text.contains("notes.txt"));
    assert!(paths_text.contains("patched.txt"));

    let exec_params = HostExecParams {
        session_id: session_id.clone(),
        argv: vec!["/bin/sh".into(), "-lc".into(), "printf 'exec-ok'".into()],
        cwd: None,
        timeout_ns: None,
        env_patch: None,
        stdin_ref: None,
        output_mode: Some("require_inline".into()),
    };
    let exec_receipt = set
        .exec
        .execute(&build_intent(
            EffectKind::HOST_EXEC,
            serde_cbor::to_vec(&exec_params).unwrap(),
            8,
        ))
        .await
        .unwrap();
    let exec_payload: HostExecReceipt = serde_cbor::from_slice(&exec_receipt.payload_cbor).unwrap();
    assert_eq!(exec_payload.status, "ok");
    match exec_payload.stdout {
        Some(HostOutput::InlineText { inline_text }) => assert_eq!(inline_text.text, "exec-ok"),
        other => panic!("expected inline text exec output, got {other:?}"),
    }

    let rel_outside = relative_path(tmp.path(), &outside_file);
    let forbidden_params = HostFsReadFileParams {
        session_id: session_id.clone(),
        path: rel_outside,
        offset_bytes: None,
        max_bytes: None,
        encoding: Some("utf8".into()),
        output_mode: Some("require_inline".into()),
    };
    let forbidden_receipt = set
        .fs_read_file
        .execute(&build_intent(
            EffectKind::HOST_FS_READ_FILE,
            serde_cbor::to_vec(&forbidden_params).unwrap(),
            9,
        ))
        .await
        .unwrap();
    let forbidden_payload: HostFsReadFileReceipt =
        serde_cbor::from_slice(&forbidden_receipt.payload_cbor).unwrap();
    assert_eq!(forbidden_payload.status, "forbidden");
    assert_eq!(forbidden_payload.error_code.as_deref(), Some("forbidden"));

    let large = "z".repeat(20_000);
    fs::write(tmp.path().join("large.txt"), large).unwrap();

    let auto_large_params = HostFsReadFileParams {
        session_id: session_id.clone(),
        path: "large.txt".into(),
        offset_bytes: None,
        max_bytes: None,
        encoding: Some("utf8".into()),
        output_mode: Some("auto".into()),
    };
    let auto_large_receipt = set
        .fs_read_file
        .execute(&build_intent(
            EffectKind::HOST_FS_READ_FILE,
            serde_cbor::to_vec(&auto_large_params).unwrap(),
            10,
        ))
        .await
        .unwrap();
    let auto_large_payload: HostFsReadFileReceipt =
        serde_cbor::from_slice(&auto_large_receipt.payload_cbor).unwrap();
    assert_eq!(auto_large_payload.status, "ok");
    match auto_large_payload.content {
        Some(HostOutput::Blob { blob }) => {
            let hash = Hash::from_hex_str(blob.blob_ref.as_str()).unwrap();
            let bytes = store.get_blob(hash).unwrap();
            assert_eq!(bytes.len(), 20_000);
        }
        other => panic!("expected blob read payload, got {other:?}"),
    }

    let inline_large_params = HostFsReadFileParams {
        session_id,
        path: "large.txt".into(),
        offset_bytes: None,
        max_bytes: None,
        encoding: Some("utf8".into()),
        output_mode: Some("require_inline".into()),
    };
    let inline_large_receipt = set
        .fs_read_file
        .execute(&build_intent(
            EffectKind::HOST_FS_READ_FILE,
            serde_cbor::to_vec(&inline_large_params).unwrap(),
            11,
        ))
        .await
        .unwrap();
    assert_eq!(inline_large_receipt.status, ReceiptStatus::Error);
    let inline_large_payload: HostFsReadFileReceipt =
        serde_cbor::from_slice(&inline_large_receipt.payload_cbor).unwrap();
    assert_eq!(inline_large_payload.status, "error");
    assert_eq!(
        inline_large_payload.error_code.as_deref(),
        Some("inline_required_too_large")
    );
}

fn relative_path(base: &Path, target: &Path) -> String {
    let mut current = PathBuf::from(base);
    let mut prefix = String::new();
    while !target.starts_with(&current) {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str("..");
        if !current.pop() {
            return "../outside.txt".into();
        }
    }
    let suffix = target
        .strip_prefix(&current)
        .unwrap()
        .to_string_lossy()
        .to_string();
    if prefix.is_empty() {
        suffix
    } else if suffix.is_empty() {
        prefix
    } else {
        format!("{prefix}/{suffix}")
    }
}
