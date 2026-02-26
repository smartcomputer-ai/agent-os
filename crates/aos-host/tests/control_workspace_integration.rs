use std::sync::Arc;

use aos_air_types::{ReducerAbi, RoutingEvent};
use aos_host::control::{ControlClient, ControlServer, RequestEnvelope};
use aos_host::{WorldHost, config::HostConfig};
use aos_kernel::Kernel;
use aos_kernel::journal::mem::MemJournal;
use aos_wasm_abi::ReducerOutput;
use base64::prelude::*;
use serde_json::json;
use std::os::unix::net::UnixListener;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};

#[path = "helpers.rs"]
mod helpers;
use helpers::fixtures;
use helpers::fixtures::TestStore;

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

#[tokio::test]
async fn control_workspace_internal_effects() {
    if !control_socket_allowed() {
        eprintln!("skipping control_workspace_internal_effects: control socket not permitted");
        return;
    }

    let store: Arc<TestStore> = fixtures::new_mem_store();
    let reducer_output = ReducerOutput {
        state: None,
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };
    let mut reducer = fixtures::stub_reducer_module(&store, "sys/Workspace@1", &reducer_output);
    reducer.key_schema = Some(fixtures::schema("sys/WorkspaceName@1"));
    reducer.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("sys/WorkspaceHistory@1"),
        event: fixtures::schema("sys/WorkspaceCommit@1"),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let routing = vec![RoutingEvent {
        event: fixtures::schema("sys/WorkspaceCommit@1"),
        module: "sys/Workspace@1".to_string(),
        key_field: Some("workspace".into()),
    }];
    let manifest = fixtures::build_loaded_manifest(vec![reducer], routing);

    let kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();
    let host = WorldHost::from_kernel(kernel, store.clone(), HostConfig::default());

    let (control_tx, control_rx) = mpsc::channel(8);
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    let mut daemon = aos_host::WorldDaemon::new(host, control_rx, shutdown_rx, None, None);
    let daemon_handle = tokio::spawn(async move { daemon.run().await });

    let sock_dir = TempDir::new().unwrap();
    let sock_path = sock_dir.path().join("control.sock");
    let server = ControlServer::new(
        sock_path.clone(),
        control_tx.clone(),
        shutdown_tx.clone(),
        aos_host::control::ControlMode::Ndjson,
    );
    tokio::spawn(async move {
        let _ = server.run().await;
    });

    for _ in 0..20 {
        if sock_path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let mut client = ControlClient::connect(&sock_path).await.unwrap();

    let resolve = RequestEnvelope {
        v: 1,
        id: "resolve".into(),
        cmd: "workspace-resolve".into(),
        payload: json!({ "workspace": "alpha" }),
    };
    let resp = client.request(&resolve).await.unwrap();
    assert!(resp.ok, "workspace-resolve failed: {:?}", resp.error);
    let exists = resp
        .result
        .as_ref()
        .and_then(|v| v.get("exists"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    assert!(!exists, "expected workspace to be missing");

    let empty_root = RequestEnvelope {
        v: 1,
        id: "empty".into(),
        cmd: "workspace-empty-root".into(),
        payload: json!({ "workspace": "alpha" }),
    };
    let resp = client.request(&empty_root).await.unwrap();
    assert!(resp.ok, "workspace-empty-root failed: {:?}", resp.error);
    let root_hash = resp
        .result
        .and_then(|v| v.get("root_hash").cloned())
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .expect("root_hash missing");

    let write = RequestEnvelope {
        v: 1,
        id: "write".into(),
        cmd: "workspace-write-bytes".into(),
        payload: json!({
            "root_hash": root_hash.clone(),
            "path": "README.md",
            "bytes_b64": BASE64_STANDARD.encode(b"hello"),
            "mode": 0o644u64,
        }),
    };
    let resp = client.request(&write).await.unwrap();
    assert!(resp.ok, "workspace-write-bytes failed: {:?}", resp.error);
    let new_root = resp
        .result
        .as_ref()
        .and_then(|v| v.get("new_root_hash"))
        .and_then(|v| v.as_str())
        .expect("new_root_hash missing")
        .to_string();

    let read = RequestEnvelope {
        v: 1,
        id: "read".into(),
        cmd: "workspace-read-bytes".into(),
        payload: json!({
            "root_hash": new_root.clone(),
            "path": "README.md",
        }),
    };
    let resp = client.request(&read).await.unwrap();
    assert!(resp.ok, "workspace-read-bytes failed: {:?}", resp.error);
    let data_b64 = resp
        .result
        .as_ref()
        .and_then(|v| v.get("data_b64"))
        .and_then(|v| v.as_str())
        .expect("data_b64 missing");
    let bytes = BASE64_STANDARD.decode(data_b64).unwrap();
    assert_eq!(bytes, b"hello");

    let list = RequestEnvelope {
        v: 1,
        id: "list".into(),
        cmd: "workspace-list".into(),
        payload: json!({
            "root_hash": new_root.clone(),
            "path": null,
            "scope": "dir",
            "limit": 0u64,
        }),
    };
    let resp = client.request(&list).await.unwrap();
    assert!(resp.ok, "workspace-list failed: {:?}", resp.error);
    let entries = resp
        .result
        .as_ref()
        .and_then(|v| v.get("entries"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        entries
            .iter()
            .any(|e| e.get("path").and_then(|p| p.as_str()) == Some("README.md")),
        "expected README.md entry"
    );

    let read_ref = RequestEnvelope {
        v: 1,
        id: "ref".into(),
        cmd: "workspace-read-ref".into(),
        payload: json!({
            "root_hash": new_root.clone(),
            "path": "README.md",
        }),
    };
    let resp = client.request(&read_ref).await.unwrap();
    assert!(resp.ok, "workspace-read-ref failed: {:?}", resp.error);
    let kind = resp
        .result
        .as_ref()
        .and_then(|v| v.get("kind"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert_eq!(kind, "file");

    let diff = RequestEnvelope {
        v: 1,
        id: "diff".into(),
        cmd: "workspace-diff".into(),
        payload: json!({
            "root_a": root_hash,
            "root_b": new_root,
        }),
    };
    let resp = client.request(&diff).await.unwrap();
    assert!(resp.ok, "workspace-diff failed: {:?}", resp.error);
    let changes = resp
        .result
        .as_ref()
        .and_then(|v| v.get("changes"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        changes
            .iter()
            .any(|c| c.get("path").and_then(|p| p.as_str()) == Some("README.md")),
        "expected diff for README.md"
    );

    // shutdown daemon
    let shutdown_cmd = RequestEnvelope {
        v: 1,
        id: "shutdown".into(),
        cmd: "shutdown".into(),
        payload: json!({}),
    };
    let resp = client.request(&shutdown_cmd).await.unwrap();
    assert!(resp.ok);
    let _ = shutdown_tx.send(());
    daemon_handle.await.unwrap().unwrap();
}
