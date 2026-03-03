use std::fs;
use std::path::Path;
use std::sync::Arc;

use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::{
    HostBlobRefInput, HostFileContentInput, HostFsApplyPatchParams, HostFsApplyPatchReceipt,
    HostFsEditFileParams, HostFsEditFileReceipt, HostFsExistsParams, HostFsExistsReceipt,
    HostFsGlobParams, HostFsGlobReceipt, HostFsGrepParams, HostFsGrepReceipt, HostFsListDirParams,
    HostFsListDirReceipt, HostFsReadFileParams, HostFsReadFileReceipt, HostFsStatParams,
    HostFsStatReceipt, HostFsWriteFileParams, HostFsWriteFileReceipt, HostInlineBytes,
    HostInlineText, HostLocalTarget, HostOutput, HostPatchInput, HostSessionOpenParams,
    HostSessionOpenReceipt, HostTarget, HostTextOutput,
};
use aos_effects::{EffectIntent, EffectKind, ReceiptStatus};
use aos_host::adapters::host::{HostAdapterSet, make_host_adapter_set};
use aos_host::adapters::traits::AsyncEffectAdapter;
use aos_store::{MemStore, Store};
use tempfile::TempDir;

fn build_intent(kind: &str, params_cbor: Vec<u8>, seed: u8) -> EffectIntent {
    EffectIntent::from_raw_params(EffectKind::new(kind), "cap", params_cbor, [seed; 32]).unwrap()
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
async fn host_fs_read_file_modes_and_statuses_expected() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(MemStore::new());
    let set = make_host_adapter_set(store.clone());
    let session_id = open_session(&set, tmp.path(), 1).await;

    let dir_path = tmp.path().join("nested");
    fs::create_dir_all(&dir_path).unwrap();
    let text_path = tmp.path().join("large.txt");
    fs::write(&text_path, "x".repeat(20_000)).unwrap();
    let bytes_path = tmp.path().join("bytes.bin");
    fs::write(&bytes_path, [0_u8, 159, 255, 10]).unwrap();

    let auto_params = HostFsReadFileParams {
        session_id: session_id.clone(),
        path: "large.txt".into(),
        offset_bytes: None,
        max_bytes: None,
        encoding: Some("utf8".into()),
        output_mode: Some("auto".into()),
    };
    let auto_receipt = set
        .fs_read_file
        .execute(&build_intent(
            EffectKind::HOST_FS_READ_FILE,
            serde_cbor::to_vec(&auto_params).unwrap(),
            2,
        ))
        .await
        .unwrap();
    assert_eq!(auto_receipt.status, ReceiptStatus::Ok);
    let auto_payload: HostFsReadFileReceipt =
        serde_cbor::from_slice(&auto_receipt.payload_cbor).unwrap();
    assert_eq!(auto_payload.status, "ok");
    match auto_payload.content {
        Some(HostOutput::Blob { blob }) => {
            let hash = Hash::from_hex_str(blob.blob_ref.as_str()).unwrap();
            let bytes = store.get_blob(hash).unwrap();
            assert_eq!(bytes.len(), 20_000);
        }
        other => panic!("expected blob read payload, got {other:?}"),
    }

    let inline_params = HostFsReadFileParams {
        session_id: session_id.clone(),
        path: "large.txt".into(),
        offset_bytes: None,
        max_bytes: None,
        encoding: Some("utf8".into()),
        output_mode: Some("require_inline".into()),
    };
    let inline_receipt = set
        .fs_read_file
        .execute(&build_intent(
            EffectKind::HOST_FS_READ_FILE,
            serde_cbor::to_vec(&inline_params).unwrap(),
            3,
        ))
        .await
        .unwrap();
    assert_eq!(inline_receipt.status, ReceiptStatus::Error);
    let inline_payload: HostFsReadFileReceipt =
        serde_cbor::from_slice(&inline_receipt.payload_cbor).unwrap();
    assert_eq!(inline_payload.status, "error");
    assert_eq!(
        inline_payload.error_code.as_deref(),
        Some("inline_required_too_large")
    );

    let bytes_params = HostFsReadFileParams {
        session_id: session_id.clone(),
        path: "bytes.bin".into(),
        offset_bytes: None,
        max_bytes: None,
        encoding: Some("bytes".into()),
        output_mode: Some("require_inline".into()),
    };
    let bytes_receipt = set
        .fs_read_file
        .execute(&build_intent(
            EffectKind::HOST_FS_READ_FILE,
            serde_cbor::to_vec(&bytes_params).unwrap(),
            4,
        ))
        .await
        .unwrap();
    let bytes_payload: HostFsReadFileReceipt =
        serde_cbor::from_slice(&bytes_receipt.payload_cbor).unwrap();
    assert_eq!(bytes_payload.status, "ok");
    match bytes_payload.content {
        Some(HostOutput::InlineBytes { inline_bytes }) => {
            assert_eq!(inline_bytes.bytes, vec![0_u8, 159, 255, 10]);
        }
        other => panic!("expected inline bytes payload, got {other:?}"),
    }

    let missing_params = HostFsReadFileParams {
        session_id: session_id.clone(),
        path: "missing.txt".into(),
        offset_bytes: None,
        max_bytes: None,
        encoding: None,
        output_mode: None,
    };
    let missing_receipt = set
        .fs_read_file
        .execute(&build_intent(
            EffectKind::HOST_FS_READ_FILE,
            serde_cbor::to_vec(&missing_params).unwrap(),
            5,
        ))
        .await
        .unwrap();
    let missing_payload: HostFsReadFileReceipt =
        serde_cbor::from_slice(&missing_receipt.payload_cbor).unwrap();
    assert_eq!(missing_payload.status, "not_found");

    let dir_params = HostFsReadFileParams {
        session_id,
        path: "nested".into(),
        offset_bytes: None,
        max_bytes: None,
        encoding: None,
        output_mode: None,
    };
    let dir_receipt = set
        .fs_read_file
        .execute(&build_intent(
            EffectKind::HOST_FS_READ_FILE,
            serde_cbor::to_vec(&dir_params).unwrap(),
            6,
        ))
        .await
        .unwrap();
    let dir_payload: HostFsReadFileReceipt =
        serde_cbor::from_slice(&dir_receipt.payload_cbor).unwrap();
    assert_eq!(dir_payload.status, "is_directory");
}

#[tokio::test]
async fn host_fs_write_file_modes_and_content_sources_expected() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(MemStore::new());
    let set = make_host_adapter_set(store.clone());
    let session_id = open_session(&set, tmp.path(), 10).await;

    let write_text = HostFsWriteFileParams {
        session_id: session_id.clone(),
        path: "src/lib.rs".into(),
        content: HostFileContentInput::InlineText {
            inline_text: HostInlineText {
                text: "pub fn run() {}\n".into(),
            },
        },
        create_parents: Some(true),
        mode: Some("overwrite".into()),
    };
    let write_receipt = set
        .fs_write_file
        .execute(&build_intent(
            EffectKind::HOST_FS_WRITE_FILE,
            serde_cbor::to_vec(&write_text).unwrap(),
            11,
        ))
        .await
        .unwrap();
    assert_eq!(write_receipt.status, ReceiptStatus::Ok);
    let write_payload: HostFsWriteFileReceipt =
        serde_cbor::from_slice(&write_receipt.payload_cbor).unwrap();
    assert_eq!(write_payload.status, "ok");
    assert_eq!(write_payload.created, Some(true));

    let create_new_conflict = HostFsWriteFileParams {
        session_id: session_id.clone(),
        path: "src/lib.rs".into(),
        content: HostFileContentInput::InlineBytes {
            inline_bytes: HostInlineBytes {
                bytes: b"ignored".to_vec(),
            },
        },
        create_parents: Some(true),
        mode: Some("create_new".into()),
    };
    let conflict_receipt = set
        .fs_write_file
        .execute(&build_intent(
            EffectKind::HOST_FS_WRITE_FILE,
            serde_cbor::to_vec(&create_new_conflict).unwrap(),
            12,
        ))
        .await
        .unwrap();
    let conflict_payload: HostFsWriteFileReceipt =
        serde_cbor::from_slice(&conflict_receipt.payload_cbor).unwrap();
    assert_eq!(conflict_payload.status, "conflict");
    assert_eq!(conflict_payload.error_code.as_deref(), Some("file_exists"));

    let blob_hash = store.put_blob(b"from-blob").unwrap();
    let blob_ref = HashRef::new(blob_hash.to_hex()).unwrap();
    let write_blob = HostFsWriteFileParams {
        session_id,
        path: "blob.txt".into(),
        content: HostFileContentInput::BlobRef {
            blob_ref: HostBlobRefInput { blob_ref },
        },
        create_parents: Some(false),
        mode: Some("overwrite".into()),
    };
    let blob_receipt = set
        .fs_write_file
        .execute(&build_intent(
            EffectKind::HOST_FS_WRITE_FILE,
            serde_cbor::to_vec(&write_blob).unwrap(),
            13,
        ))
        .await
        .unwrap();
    let blob_payload: HostFsWriteFileReceipt =
        serde_cbor::from_slice(&blob_receipt.payload_cbor).unwrap();
    assert_eq!(blob_payload.status, "ok");
    assert_eq!(
        fs::read_to_string(tmp.path().join("blob.txt")).unwrap(),
        "from-blob"
    );
}

#[tokio::test]
async fn host_fs_edit_file_exact_fuzzy_ambiguous_and_invalid_inputs_expected() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(MemStore::new());
    let set = make_host_adapter_set(store);
    let session_id = open_session(&set, tmp.path(), 20).await;

    fs::write(tmp.path().join("main.txt"), "alpha\nalpha\n").unwrap();
    fs::write(tmp.path().join("fuzzy.txt"), "let s = \"hello   world\";\n").unwrap();

    let ambiguous_params = HostFsEditFileParams {
        session_id: session_id.clone(),
        path: "main.txt".into(),
        old_string: "alpha".into(),
        new_string: "beta".into(),
        replace_all: Some(false),
    };
    let ambiguous_receipt = set
        .fs_edit_file
        .execute(&build_intent(
            EffectKind::HOST_FS_EDIT_FILE,
            serde_cbor::to_vec(&ambiguous_params).unwrap(),
            21,
        ))
        .await
        .unwrap();
    let ambiguous_payload: HostFsEditFileReceipt =
        serde_cbor::from_slice(&ambiguous_receipt.payload_cbor).unwrap();
    assert_eq!(ambiguous_payload.status, "ambiguous");
    assert_eq!(
        ambiguous_payload.error_code.as_deref(),
        Some("ambiguous_matches")
    );

    let replace_all_params = HostFsEditFileParams {
        session_id: session_id.clone(),
        path: "main.txt".into(),
        old_string: "alpha".into(),
        new_string: "beta".into(),
        replace_all: Some(true),
    };
    let replace_all_receipt = set
        .fs_edit_file
        .execute(&build_intent(
            EffectKind::HOST_FS_EDIT_FILE,
            serde_cbor::to_vec(&replace_all_params).unwrap(),
            22,
        ))
        .await
        .unwrap();
    let replace_all_payload: HostFsEditFileReceipt =
        serde_cbor::from_slice(&replace_all_receipt.payload_cbor).unwrap();
    assert_eq!(replace_all_payload.status, "ok");
    assert_eq!(replace_all_payload.replacements, Some(2));
    assert_eq!(
        fs::read_to_string(tmp.path().join("main.txt")).unwrap(),
        "beta\nbeta\n"
    );

    let fuzzy_params = HostFsEditFileParams {
        session_id: session_id.clone(),
        path: "fuzzy.txt".into(),
        old_string: "let s = “hello world”;".into(),
        new_string: "let s = \"ok\";".into(),
        replace_all: Some(false),
    };
    let fuzzy_receipt = set
        .fs_edit_file
        .execute(&build_intent(
            EffectKind::HOST_FS_EDIT_FILE,
            serde_cbor::to_vec(&fuzzy_params).unwrap(),
            23,
        ))
        .await
        .unwrap();
    let fuzzy_payload: HostFsEditFileReceipt =
        serde_cbor::from_slice(&fuzzy_receipt.payload_cbor).unwrap();
    assert_eq!(fuzzy_payload.status, "ok");
    assert_eq!(fuzzy_payload.replacements, Some(1));
    assert_eq!(
        fs::read_to_string(tmp.path().join("fuzzy.txt")).unwrap(),
        "let s = \"ok\";\n"
    );

    let not_found_params = HostFsEditFileParams {
        session_id: session_id.clone(),
        path: "main.txt".into(),
        old_string: "does-not-exist".into(),
        new_string: "x".into(),
        replace_all: Some(false),
    };
    let not_found_receipt = set
        .fs_edit_file
        .execute(&build_intent(
            EffectKind::HOST_FS_EDIT_FILE,
            serde_cbor::to_vec(&not_found_params).unwrap(),
            24,
        ))
        .await
        .unwrap();
    let not_found_payload: HostFsEditFileReceipt =
        serde_cbor::from_slice(&not_found_receipt.payload_cbor).unwrap();
    assert_eq!(not_found_payload.status, "not_found");

    let invalid_old_string_params = HostFsEditFileParams {
        session_id,
        path: "main.txt".into(),
        old_string: String::new(),
        new_string: "x".into(),
        replace_all: Some(false),
    };
    let invalid_receipt = set
        .fs_edit_file
        .execute(&build_intent(
            EffectKind::HOST_FS_EDIT_FILE,
            serde_cbor::to_vec(&invalid_old_string_params).unwrap(),
            25,
        ))
        .await
        .unwrap();
    assert_eq!(invalid_receipt.status, ReceiptStatus::Error);
    let invalid_payload: HostFsEditFileReceipt =
        serde_cbor::from_slice(&invalid_receipt.payload_cbor).unwrap();
    assert_eq!(invalid_payload.status, "error");
    assert_eq!(
        invalid_payload.error_code.as_deref(),
        Some("invalid_input_empty_old_string")
    );
}

#[tokio::test]
async fn host_fs_apply_patch_success_dry_run_parse_error_and_atomic_failure_expected() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(MemStore::new());
    let set = make_host_adapter_set(store);
    let session_id = open_session(&set, tmp.path(), 30).await;
    fs::write(tmp.path().join("a.txt"), "one\n").unwrap();

    let update_patch = "\
*** Begin Patch
*** Update File: a.txt
@@ replace
-one
+two
*** End Patch";
    let dry_run_params = HostFsApplyPatchParams {
        session_id: session_id.clone(),
        patch: HostPatchInput::InlineText {
            inline_text: HostInlineText {
                text: update_patch.into(),
            },
        },
        patch_format: Some("v4a".into()),
        dry_run: Some(true),
    };
    let dry_run_receipt = set
        .fs_apply_patch
        .execute(&build_intent(
            EffectKind::HOST_FS_APPLY_PATCH,
            serde_cbor::to_vec(&dry_run_params).unwrap(),
            31,
        ))
        .await
        .unwrap();
    let dry_run_payload: HostFsApplyPatchReceipt =
        serde_cbor::from_slice(&dry_run_receipt.payload_cbor).unwrap();
    assert_eq!(dry_run_payload.status, "ok");
    assert_eq!(dry_run_payload.files_changed, Some(1));
    assert_eq!(
        fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "one\n"
    );

    let apply_params = HostFsApplyPatchParams {
        session_id: session_id.clone(),
        patch: HostPatchInput::InlineText {
            inline_text: HostInlineText {
                text: update_patch.into(),
            },
        },
        patch_format: Some("v4a".into()),
        dry_run: Some(false),
    };
    let apply_receipt = set
        .fs_apply_patch
        .execute(&build_intent(
            EffectKind::HOST_FS_APPLY_PATCH,
            serde_cbor::to_vec(&apply_params).unwrap(),
            32,
        ))
        .await
        .unwrap();
    let apply_payload: HostFsApplyPatchReceipt =
        serde_cbor::from_slice(&apply_receipt.payload_cbor).unwrap();
    assert_eq!(apply_payload.status, "ok");
    assert_eq!(apply_payload.files_changed, Some(1));
    assert_eq!(
        fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "two\n"
    );

    let parse_error_params = HostFsApplyPatchParams {
        session_id: session_id.clone(),
        patch: HostPatchInput::InlineText {
            inline_text: HostInlineText {
                text: "not-a-v4a-patch".into(),
            },
        },
        patch_format: Some("v4a".into()),
        dry_run: Some(false),
    };
    let parse_error_receipt = set
        .fs_apply_patch
        .execute(&build_intent(
            EffectKind::HOST_FS_APPLY_PATCH,
            serde_cbor::to_vec(&parse_error_params).unwrap(),
            33,
        ))
        .await
        .unwrap();
    let parse_error_payload: HostFsApplyPatchReceipt =
        serde_cbor::from_slice(&parse_error_receipt.payload_cbor).unwrap();
    assert_eq!(parse_error_payload.status, "parse_error");
    assert_eq!(
        parse_error_payload.error_code.as_deref(),
        Some("patch_parse_error")
    );

    let partial_failure_patch = "\
*** Begin Patch
*** Add File: created.txt
+hello
*** Update File: missing.txt
@@ missing
-old
+new
*** End Patch";
    let partial_failure_params = HostFsApplyPatchParams {
        session_id,
        patch: HostPatchInput::InlineText {
            inline_text: HostInlineText {
                text: partial_failure_patch.into(),
            },
        },
        patch_format: Some("v4a".into()),
        dry_run: Some(false),
    };
    let partial_failure_receipt = set
        .fs_apply_patch
        .execute(&build_intent(
            EffectKind::HOST_FS_APPLY_PATCH,
            serde_cbor::to_vec(&partial_failure_params).unwrap(),
            34,
        ))
        .await
        .unwrap();
    let partial_failure_payload: HostFsApplyPatchReceipt =
        serde_cbor::from_slice(&partial_failure_receipt.payload_cbor).unwrap();
    assert_eq!(partial_failure_payload.status, "not_found");
    assert_eq!(
        partial_failure_payload.error_code.as_deref(),
        Some("update_target_not_found")
    );
    assert!(!tmp.path().join("created.txt").exists());
}

#[tokio::test]
async fn host_fs_grep_glob_stat_exists_and_list_dir_expected() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(MemStore::new());
    let set = make_host_adapter_set(store.clone());
    let session_id = open_session(&set, tmp.path(), 40).await;

    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/a.rs"), "hit\nmiss\nhit\n").unwrap();
    fs::write(tmp.path().join("src/b.rs"), "hit\n").unwrap();
    fs::write(tmp.path().join("new.order"), "new").unwrap();
    fs::write(tmp.path().join("old.order"), "old").unwrap();

    let grep_params = HostFsGrepParams {
        session_id: session_id.clone(),
        pattern: "hit".into(),
        path: Some("src".into()),
        glob_filter: Some("*.rs".into()),
        case_insensitive: Some(false),
        max_results: Some(2),
        output_mode: Some("require_inline".into()),
    };
    let grep_receipt = set
        .fs_grep
        .execute(&build_intent(
            EffectKind::HOST_FS_GREP,
            serde_cbor::to_vec(&grep_params).unwrap(),
            41,
        ))
        .await
        .unwrap();
    let grep_payload: HostFsGrepReceipt =
        serde_cbor::from_slice(&grep_receipt.payload_cbor).unwrap();
    assert_eq!(grep_payload.status, "ok");
    assert_eq!(grep_payload.match_count, Some(3));
    assert_eq!(grep_payload.truncated, Some(true));

    let invalid_regex_params = HostFsGrepParams {
        session_id: session_id.clone(),
        pattern: "(".into(),
        path: Some("src".into()),
        glob_filter: None,
        case_insensitive: None,
        max_results: None,
        output_mode: Some("auto".into()),
    };
    let invalid_regex_receipt = set
        .fs_grep
        .execute(&build_intent(
            EffectKind::HOST_FS_GREP,
            serde_cbor::to_vec(&invalid_regex_params).unwrap(),
            42,
        ))
        .await
        .unwrap();
    let invalid_regex_payload: HostFsGrepReceipt =
        serde_cbor::from_slice(&invalid_regex_receipt.payload_cbor).unwrap();
    assert_eq!(invalid_regex_payload.status, "invalid_regex");

    let no_match_params = HostFsGrepParams {
        session_id: session_id.clone(),
        pattern: "does-not-exist".into(),
        path: Some("src".into()),
        glob_filter: None,
        case_insensitive: None,
        max_results: None,
        output_mode: Some("require_inline".into()),
    };
    let no_match_receipt = set
        .fs_grep
        .execute(&build_intent(
            EffectKind::HOST_FS_GREP,
            serde_cbor::to_vec(&no_match_params).unwrap(),
            43,
        ))
        .await
        .unwrap();
    let no_match_payload: HostFsGrepReceipt =
        serde_cbor::from_slice(&no_match_receipt.payload_cbor).unwrap();
    assert_eq!(no_match_payload.status, "ok");
    assert_eq!(no_match_payload.match_count, Some(0));

    let glob_params = HostFsGlobParams {
        session_id: session_id.clone(),
        pattern: "*.order".into(),
        path: None,
        max_results: Some(10),
        output_mode: Some("require_inline".into()),
    };
    let glob_receipt_first = set
        .fs_glob
        .execute(&build_intent(
            EffectKind::HOST_FS_GLOB,
            serde_cbor::to_vec(&glob_params).unwrap(),
            44,
        ))
        .await
        .unwrap();
    let glob_payload_first: HostFsGlobReceipt =
        serde_cbor::from_slice(&glob_receipt_first.payload_cbor).unwrap();
    assert_eq!(glob_payload_first.status, "ok");
    let first_text = decode_text_output(glob_payload_first.paths.as_ref().unwrap(), store.as_ref());

    let glob_receipt_second = set
        .fs_glob
        .execute(&build_intent(
            EffectKind::HOST_FS_GLOB,
            serde_cbor::to_vec(&glob_params).unwrap(),
            45,
        ))
        .await
        .unwrap();
    let glob_payload_second: HostFsGlobReceipt =
        serde_cbor::from_slice(&glob_receipt_second.payload_cbor).unwrap();
    let second_text =
        decode_text_output(glob_payload_second.paths.as_ref().unwrap(), store.as_ref());
    assert_eq!(first_text, second_text);

    let invalid_glob_params = HostFsGlobParams {
        session_id: session_id.clone(),
        pattern: "[".into(),
        path: None,
        max_results: None,
        output_mode: None,
    };
    let invalid_glob_receipt = set
        .fs_glob
        .execute(&build_intent(
            EffectKind::HOST_FS_GLOB,
            serde_cbor::to_vec(&invalid_glob_params).unwrap(),
            46,
        ))
        .await
        .unwrap();
    let invalid_glob_payload: HostFsGlobReceipt =
        serde_cbor::from_slice(&invalid_glob_receipt.payload_cbor).unwrap();
    assert_eq!(invalid_glob_payload.status, "invalid_pattern");

    let stat_ok_params = HostFsStatParams {
        session_id: session_id.clone(),
        path: "src/a.rs".into(),
    };
    let stat_ok_receipt = set
        .fs_stat
        .execute(&build_intent(
            EffectKind::HOST_FS_STAT,
            serde_cbor::to_vec(&stat_ok_params).unwrap(),
            47,
        ))
        .await
        .unwrap();
    let stat_ok_payload: HostFsStatReceipt =
        serde_cbor::from_slice(&stat_ok_receipt.payload_cbor).unwrap();
    assert_eq!(stat_ok_payload.status, "ok");
    assert_eq!(stat_ok_payload.exists, Some(true));

    let stat_missing_params = HostFsStatParams {
        session_id: session_id.clone(),
        path: "missing.rs".into(),
    };
    let stat_missing_receipt = set
        .fs_stat
        .execute(&build_intent(
            EffectKind::HOST_FS_STAT,
            serde_cbor::to_vec(&stat_missing_params).unwrap(),
            48,
        ))
        .await
        .unwrap();
    let stat_missing_payload: HostFsStatReceipt =
        serde_cbor::from_slice(&stat_missing_receipt.payload_cbor).unwrap();
    assert_eq!(stat_missing_payload.status, "not_found");

    let exists_params = HostFsExistsParams {
        session_id: session_id.clone(),
        path: "src/a.rs".into(),
    };
    let exists_receipt = set
        .fs_exists
        .execute(&build_intent(
            EffectKind::HOST_FS_EXISTS,
            serde_cbor::to_vec(&exists_params).unwrap(),
            49,
        ))
        .await
        .unwrap();
    let exists_payload: HostFsExistsReceipt =
        serde_cbor::from_slice(&exists_receipt.payload_cbor).unwrap();
    assert_eq!(exists_payload.status, "ok");
    assert_eq!(exists_payload.exists, Some(true));

    let list_dir_params = HostFsListDirParams {
        session_id,
        path: Some("src".into()),
        max_results: Some(1),
        output_mode: Some("require_inline".into()),
    };
    let list_dir_receipt = set
        .fs_list_dir
        .execute(&build_intent(
            EffectKind::HOST_FS_LIST_DIR,
            serde_cbor::to_vec(&list_dir_params).unwrap(),
            50,
        ))
        .await
        .unwrap();
    let list_dir_payload: HostFsListDirReceipt =
        serde_cbor::from_slice(&list_dir_receipt.payload_cbor).unwrap();
    assert_eq!(list_dir_payload.status, "ok");
    assert_eq!(list_dir_payload.count, Some(2));
    assert_eq!(list_dir_payload.truncated, Some(true));
}
