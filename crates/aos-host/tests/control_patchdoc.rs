//! Control channel governance: patch-doc validation and compilation

use std::time::Duration;

use aos_cbor::Hash;
use aos_host::WorldHost;
use aos_host::config::HostConfig;
use aos_host::control::{ControlClient, ControlMode, ControlServer, RequestEnvelope};
use aos_host::fixtures::{self, TestStore};
use aos_host::modes::daemon::WorldDaemon;
use aos_kernel::Kernel;
use aos_kernel::journal::mem::MemJournal;
use aos_store::Store;
use base64::prelude::*;
use serde_json::json;
use std::os::unix::net::UnixListener;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};

#[path = "helpers.rs"]
mod helpers;

fn control_socket_allowed() -> bool {
    let dir = tempfile::tempdir();
    if dir.is_err() {
        return false;
    }
    let dir = dir.unwrap();
    let path = dir.path().join("probe.sock");
    match UnixListener::bind(&path) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("control socket not permitted: {e}");
            false
        }
    }
}
use helpers::simple_state_manifest;

async fn setup_daemon_with_control() -> (
    ControlClient,
    TempDir,
    std::sync::Arc<TestStore>,
    String,
    tokio::sync::broadcast::Sender<()>,
    tokio::task::JoinHandle<Result<(), aos_host::error::HostError>>,
) {
    let store: std::sync::Arc<TestStore> = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    // Store defs referenced by manifest so patch compilation can load them if needed.
    for schema in manifest.schemas.values() {
        let _ = store.put_node(&aos_air_types::AirNode::Defschema(schema.clone()));
    }
    for module in manifest.modules.values() {
        let _ = store.put_node(&aos_air_types::AirNode::Defmodule(module.clone()));
    }
    for plan in manifest.plans.values() {
        let _ = store.put_node(&aos_air_types::AirNode::Defplan(plan.clone()));
    }
    for cap in manifest.caps.values() {
        let _ = store.put_node(&aos_air_types::AirNode::Defcap(cap.clone()));
    }
    for policy in manifest.policies.values() {
        let _ = store.put_node(&aos_air_types::AirNode::Defpolicy(policy.clone()));
    }
    for eff in manifest.effects.values() {
        let _ = store.put_node(&aos_air_types::AirNode::Defeffect(eff.clone()));
    }
    // Store manifest node so patch-doc base hash resolves.
    let manifest_hash = store
        .put_node(&aos_air_types::AirNode::Manifest(manifest.manifest.clone()))
        .unwrap()
        .to_hex();
    // sanity: manifest node retrievable
    let _: aos_air_types::AirNode = store
        .get_node(aos_cbor::Hash::from_hex_str(&manifest_hash).expect("hash parse"))
        .expect("manifest present in store");

    let kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();
    let host = WorldHost::from_kernel(kernel, store.clone(), HostConfig::default());

    let (control_tx, control_rx) = mpsc::channel(16);
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    // Start daemon
    let mut daemon = WorldDaemon::new(host, control_rx, shutdown_rx, None);
    let daemon_handle = tokio::spawn(async move { daemon.run().await });

    // Start control server
    let sock_dir = TempDir::new().expect("tmpdir");
    let sock_path = sock_dir.path().join("control.sock");
    let server = ControlServer::new(
        sock_path.clone(),
        control_tx.clone(),
        shutdown_tx.clone(),
        ControlMode::Ndjson,
    );
    tokio::spawn(async move {
        let _ = server.run().await;
    });

    for _ in 0..50 {
        if sock_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let client = ControlClient::connect(&sock_path).await.unwrap();
    (
        client,
        sock_dir,
        store,
        manifest_hash,
        shutdown_tx,
        daemon_handle,
    )
}

#[tokio::test]
async fn propose_rejects_invalid_patch_doc() {
    if !control_socket_allowed() {
        eprintln!(
            "skipping propose_rejects_invalid_patch_doc: control socket bind/connect not permitted"
        );
        return;
    }

    let (mut client, _tmp, _store, _hash, shutdown_tx, daemon_handle) =
        setup_daemon_with_control().await;
    // Missing base_manifest_hash -> schema validation should fail.
    let doc = json!({ "patches": [] });
    let patch_b64 = BASE64_STANDARD.encode(doc.to_string());
    let env = RequestEnvelope {
        v: 1,
        id: "invalid".into(),
        cmd: "gov-propose".into(),
        payload: json!({ "patch_b64": patch_b64 }),
    };
    let resp = client.request(&env).await.unwrap();
    assert!(!resp.ok);
    let msg = resp
        .error
        .as_ref()
        .map(|e| e.message.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("patch schema validation failed"),
        "unexpected error: {msg}"
    );
    let _ = shutdown_tx.send(());
    let _ = daemon_handle.await;
}

#[tokio::test]
async fn propose_accepts_patch_doc_and_compiles() {
    if !control_socket_allowed() {
        eprintln!(
            "skipping propose_accepts_patch_doc_and_compiles: control socket bind/connect not permitted"
        );
        return;
    }

    let (mut client, _tmp, store, base_manifest_hash, shutdown_tx, daemon_handle) =
        setup_daemon_with_control().await;

    // Build a minimal patch doc that adds a schema and sets manifest refs.
    // Obtain the base manifest hash from the running world via journal-head -> manifest.air.cbor not exposed;
    // use the stored manifest node hash from setup helper.
    let doc = json!({
        "base_manifest_hash": base_manifest_hash,
        "patches": [
            { "add_def": { "kind": "defschema", "node": { "$kind":"defschema", "name":"demo/Added@1", "type": { "bool": {} } } } },
            { "set_manifest_refs": { "add": [ { "kind":"defschema", "name":"demo/Added@1", "hash":"sha256:0000000000000000000000000000000000000000000000000000000000000000" } ] } }
        ]
    });
    let base_hash = Hash::from_hex_str(&base_manifest_hash).expect("base hash parse");
    assert!(
        store.has_node(base_hash).expect("store lookup"),
        "base manifest missing from store"
    );
    let patch_b64 = BASE64_STANDARD.encode(doc.to_string());
    let env = RequestEnvelope {
        v: 1,
        id: "valid".into(),
        cmd: "gov-propose".into(),
        payload: json!({ "patch_b64": patch_b64 }),
    };
    let resp = client.request(&env).await.unwrap();
    if !resp.ok {
        let _ = shutdown_tx.send(());
        let join = daemon_handle.await;
        let msg = resp
            .error
            .as_ref()
            .map(|e| format!("{}: {}", e.code, e.message))
            .unwrap_or_else(|| "unknown".into());
        panic!("propose failed: {msg}; daemon: {:?}", join);
    }
    let proposal_id = resp
        .result
        .as_ref()
        .and_then(|v| v.get("proposal_id"))
        .and_then(|v| v.as_u64())
        .expect("proposal id");
    assert_eq!(proposal_id, 0);
    let _ = shutdown_tx.send(());
    let _ = daemon_handle.await.expect("daemon run");
}
