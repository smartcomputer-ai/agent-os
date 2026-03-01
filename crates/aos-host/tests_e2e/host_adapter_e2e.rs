//! End-to-end tests for real process adapters (`host.session.open`,
//! `host.exec`, `host.session.signal`).

#![cfg(feature = "e2e-tests")]

use std::path::Path;
use std::sync::Arc;

use aos_cbor::Hash;
use aos_effects::builtins::{
    HostExecParams, HostExecReceipt, HostLocalTarget, HostOutput, HostSessionOpenParams,
    HostSessionOpenReceipt, HostSessionSignalParams, HostSessionSignalReceipt, HostTarget,
};
use aos_effects::{EffectIntent, EffectKind, ReceiptStatus};
use aos_host::adapters::host::{
    HostExecAdapter, HostSessionOpenAdapter, HostSessionSignalAdapter, make_host_adapters,
};
use aos_host::adapters::traits::AsyncEffectAdapter;
use aos_store::{MemStore, Store};

fn build_intent(kind: EffectKind, params_cbor: Vec<u8>, seed: u8) -> EffectIntent {
    EffectIntent::from_raw_params(kind, "cap", params_cbor, [seed; 32]).unwrap()
}

fn shell_available() -> bool {
    Path::new("/bin/sh").exists()
}

fn open_params() -> HostSessionOpenParams {
    HostSessionOpenParams {
        target: HostTarget {
            local: Some(HostLocalTarget {
                mounts: None,
                workdir: None,
                env: None,
                network_mode: "none".into(),
            }),
        },
        session_ttl_ns: None,
        labels: None,
    }
}

async fn open_session(open: &HostSessionOpenAdapter) -> String {
    let receipt = open
        .execute(&build_intent(
            EffectKind::host_session_open(),
            serde_cbor::to_vec(&open_params()).unwrap(),
            1,
        ))
        .await
        .unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Ok);
    let payload: HostSessionOpenReceipt = serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    assert_eq!(payload.status, "ready");
    payload.session_id
}

async fn signal_session(
    signal: &HostSessionSignalAdapter,
    session_id: &str,
    seed: u8,
) -> HostSessionSignalReceipt {
    let params = HostSessionSignalParams {
        session_id: session_id.into(),
        signal: "term".into(),
        grace_timeout_ns: None,
    };
    let receipt = signal
        .execute(&build_intent(
            EffectKind::host_session_signal(),
            serde_cbor::to_vec(&params).unwrap(),
            seed,
        ))
        .await
        .unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Ok);
    serde_cbor::from_slice(&receipt.payload_cbor).unwrap()
}

fn adapters(
    store: Arc<MemStore>,
) -> (
    HostSessionOpenAdapter,
    HostExecAdapter<MemStore>,
    HostSessionSignalAdapter,
) {
    make_host_adapters(store)
}

#[tokio::test]
async fn process_exec_reads_stdin_and_signal_transitions() {
    if !shell_available() {
        eprintln!(
            "skipping process_exec_reads_stdin_and_signal_transitions: /bin/sh not available"
        );
        return;
    }

    let store = Arc::new(MemStore::new());
    let (open, exec, signal) = adapters(store.clone());
    let session_id = open_session(&open).await;

    let stdin_hash = store.put_blob(b"from-stdin").unwrap();
    let stdin_ref = aos_air_types::HashRef::new(stdin_hash.to_hex()).unwrap();

    let exec_params = HostExecParams {
        session_id: session_id.clone(),
        argv: vec!["/bin/sh".into(), "-lc".into(), "cat".into()],
        cwd: None,
        timeout_ns: None,
        env_patch: None,
        stdin_ref: Some(stdin_ref),
        output_mode: Some("require_inline".into()),
    };
    let exec_receipt = exec
        .execute(&build_intent(
            EffectKind::host_exec(),
            serde_cbor::to_vec(&exec_params).unwrap(),
            2,
        ))
        .await
        .unwrap();
    assert_eq!(exec_receipt.status, ReceiptStatus::Ok);
    let exec_payload: HostExecReceipt = serde_cbor::from_slice(&exec_receipt.payload_cbor).unwrap();
    assert_eq!(exec_payload.status, "ok");
    assert_eq!(exec_payload.exit_code, 0);
    match exec_payload.stdout {
        Some(HostOutput::InlineText { inline_text }) => {
            assert_eq!(inline_text.text, "from-stdin")
        }
        other => panic!("expected inline text stdout, got {other:?}"),
    }

    let first = signal_session(&signal, &session_id, 3).await;
    assert_eq!(first.status, "signaled");
    let second = signal_session(&signal, &session_id, 4).await;
    assert_eq!(second.status, "already_exited");

    let exec_after_close = exec
        .execute(&build_intent(
            EffectKind::host_exec(),
            serde_cbor::to_vec(&exec_params).unwrap(),
            5,
        ))
        .await
        .unwrap();
    assert_eq!(exec_after_close.status, ReceiptStatus::Error);
    let payload: HostExecReceipt = serde_cbor::from_slice(&exec_after_close.payload_cbor).unwrap();
    assert_eq!(payload.status, "error");
    assert_eq!(payload.error_code.as_deref(), Some("session_closed"));
}

#[tokio::test]
async fn process_exec_timeout_maps_timeout_status() {
    if !shell_available() {
        eprintln!("skipping process_exec_timeout_maps_timeout_status: /bin/sh not available");
        return;
    }

    let store = Arc::new(MemStore::new());
    let (open, exec, _) = adapters(store);
    let session_id = open_session(&open).await;

    let exec_params = HostExecParams {
        session_id,
        argv: vec!["/bin/sh".into(), "-lc".into(), "sleep 1".into()],
        cwd: None,
        timeout_ns: Some(20_000_000),
        env_patch: None,
        stdin_ref: None,
        output_mode: Some("auto".into()),
    };

    let receipt = exec
        .execute(&build_intent(
            EffectKind::host_exec(),
            serde_cbor::to_vec(&exec_params).unwrap(),
            6,
        ))
        .await
        .unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Timeout);
    let payload: HostExecReceipt = serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    assert_eq!(payload.status, "timeout");
}

#[tokio::test]
async fn process_exec_auto_large_output_writes_blob() {
    if !shell_available() {
        eprintln!("skipping process_exec_auto_large_output_writes_blob: /bin/sh not available");
        return;
    }

    let store = Arc::new(MemStore::new());
    let (open, exec, _) = adapters(store.clone());
    let session_id = open_session(&open).await;

    let exec_params = HostExecParams {
        session_id,
        argv: vec![
            "/bin/sh".into(),
            "-lc".into(),
            "i=0; while [ $i -lt 20000 ]; do printf x; i=$((i+1)); done".into(),
        ],
        cwd: None,
        timeout_ns: None,
        env_patch: None,
        stdin_ref: None,
        output_mode: Some("auto".into()),
    };

    let receipt = exec
        .execute(&build_intent(
            EffectKind::host_exec(),
            serde_cbor::to_vec(&exec_params).unwrap(),
            7,
        ))
        .await
        .unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Ok);

    let payload: HostExecReceipt = serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    let stdout = payload.stdout.expect("stdout expected");
    match stdout {
        HostOutput::Blob { blob } => {
            let hash = Hash::from_hex_str(blob.blob_ref.as_str()).unwrap();
            let bytes = store.get_blob(hash).unwrap();
            assert_eq!(bytes.len() as u64, blob.size_bytes);
            assert!(!bytes.is_empty());
            assert!(blob.preview_bytes.is_some());
        }
        other => panic!("expected blob output, got {other:?}"),
    }
}

#[tokio::test]
async fn process_exec_require_inline_rejects_large_output() {
    if !shell_available() {
        eprintln!(
            "skipping process_exec_require_inline_rejects_large_output: /bin/sh not available"
        );
        return;
    }

    let store = Arc::new(MemStore::new());
    let (open, exec, signal) = adapters(store);
    let session_id = open_session(&open).await;

    let exec_params = HostExecParams {
        session_id: session_id.clone(),
        argv: vec![
            "/bin/sh".into(),
            "-lc".into(),
            "i=0; while [ $i -lt 20000 ]; do printf x; i=$((i+1)); done".into(),
        ],
        cwd: None,
        timeout_ns: None,
        env_patch: None,
        stdin_ref: None,
        output_mode: Some("require_inline".into()),
    };

    let receipt = exec
        .execute(&build_intent(
            EffectKind::host_exec(),
            serde_cbor::to_vec(&exec_params).unwrap(),
            8,
        ))
        .await
        .unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Error);

    let payload: HostExecReceipt = serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    assert_eq!(payload.status, "error");
    assert_eq!(
        payload.error_code.as_deref(),
        Some("inline_required_too_large")
    );

    // Cleanup should still work after inline-size rejection.
    let close = signal_session(&signal, &session_id, 9).await;
    assert!(matches!(
        close.status.as_str(),
        "signaled" | "already_exited"
    ));
}

#[tokio::test]
async fn process_signal_not_found_is_reported() {
    let store = Arc::new(MemStore::new());
    let (_, _, signal) = adapters(store);

    let payload = signal_session(&signal, "sess-missing", 42).await;
    assert_eq!(payload.status, "not_found");
}
