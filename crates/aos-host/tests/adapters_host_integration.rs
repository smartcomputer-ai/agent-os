use std::path::Path;
use std::sync::Arc;

use aos_cbor::Hash;
use aos_effects::builtins::{
    HostExecParams, HostExecReceipt, HostLocalTarget, HostOutput, HostSessionOpenParams,
    HostSessionOpenReceipt, HostSessionSignalParams, HostSessionSignalReceipt, HostTarget,
};
use aos_effects::{EffectIntent, EffectKind, ReceiptStatus};
use aos_host::adapters::host::make_host_adapters;
use aos_host::adapters::traits::AsyncEffectAdapter;
use aos_store::{MemStore, Store};

fn build_intent(kind: EffectKind, params_cbor: Vec<u8>, seed: u8) -> EffectIntent {
    EffectIntent::from_raw_params(kind, "cap", params_cbor, [seed; 32]).unwrap()
}

fn shell_available() -> bool {
    Path::new("/bin/sh").exists()
}

#[tokio::test]
async fn process_open_exec_signal_roundtrip() {
    if !shell_available() {
        eprintln!("skipping process_open_exec_signal_roundtrip: /bin/sh not available");
        return;
    }

    let store = Arc::new(MemStore::new());
    let (open_adapter, exec_adapter, signal_adapter) = make_host_adapters(store);

    let open_params = HostSessionOpenParams {
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
    };

    let open_receipt = open_adapter
        .execute(&build_intent(
            EffectKind::host_session_open(),
            serde_cbor::to_vec(&open_params).unwrap(),
            1,
        ))
        .await
        .unwrap();
    assert_eq!(open_receipt.status, ReceiptStatus::Ok);
    let open_payload: HostSessionOpenReceipt =
        serde_cbor::from_slice(&open_receipt.payload_cbor).unwrap();
    assert_eq!(open_payload.status, "ready");
    assert!(!open_payload.session_id.is_empty());

    let exec_params = HostExecParams {
        session_id: open_payload.session_id.clone(),
        argv: vec![
            "/bin/sh".into(),
            "-lc".into(),
            "printf 'integration-ok'".into(),
        ],
        cwd: None,
        timeout_ns: None,
        env_patch: None,
        stdin_ref: None,
        output_mode: Some("require_inline".into()),
    };

    let exec_receipt = exec_adapter
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
            assert_eq!(inline_text.text, "integration-ok")
        }
        other => panic!("expected inline text stdout, got {other:?}"),
    }

    let signal_params = HostSessionSignalParams {
        session_id: open_payload.session_id,
        signal: "term".into(),
        grace_timeout_ns: None,
    };

    let signal_receipt = signal_adapter
        .execute(&build_intent(
            EffectKind::host_session_signal(),
            serde_cbor::to_vec(&signal_params).unwrap(),
            3,
        ))
        .await
        .unwrap();
    assert_eq!(signal_receipt.status, ReceiptStatus::Ok);
    let signal_payload: HostSessionSignalReceipt =
        serde_cbor::from_slice(&signal_receipt.payload_cbor).unwrap();
    assert_eq!(signal_payload.status, "signaled");
}

#[tokio::test]
async fn process_exec_auto_large_output_uses_blob() {
    if !shell_available() {
        eprintln!("skipping process_exec_auto_large_output_uses_blob: /bin/sh not available");
        return;
    }

    let store = Arc::new(MemStore::new());
    let (open_adapter, exec_adapter, _) = make_host_adapters(store.clone());

    let open_params = HostSessionOpenParams {
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
    };

    let open_receipt = open_adapter
        .execute(&build_intent(
            EffectKind::host_session_open(),
            serde_cbor::to_vec(&open_params).unwrap(),
            10,
        ))
        .await
        .unwrap();
    let open_payload: HostSessionOpenReceipt =
        serde_cbor::from_slice(&open_receipt.payload_cbor).unwrap();

    // Emit >16KiB stdout to force blob arm under output_mode=auto.
    let exec_params = HostExecParams {
        session_id: open_payload.session_id,
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

    let exec_receipt = exec_adapter
        .execute(&build_intent(
            EffectKind::host_exec(),
            serde_cbor::to_vec(&exec_params).unwrap(),
            11,
        ))
        .await
        .unwrap();

    assert_eq!(exec_receipt.status, ReceiptStatus::Ok);
    let exec_payload: HostExecReceipt = serde_cbor::from_slice(&exec_receipt.payload_cbor).unwrap();
    assert_eq!(exec_payload.status, "ok");
    assert_eq!(exec_payload.exit_code, 0);

    let stdout = exec_payload.stdout.expect("stdout expected");
    match stdout {
        HostOutput::Blob { blob } => {
            let hash = Hash::from_hex_str(blob.blob_ref.as_str()).unwrap();
            let bytes = store.get_blob(hash).unwrap();
            assert_eq!(bytes.len() as u64, blob.size_bytes);
            assert!(blob.preview_bytes.is_some());
            assert!(!bytes.is_empty());
        }
        other => panic!("expected blob output, got {other:?}"),
    }
}
