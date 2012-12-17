use std::sync::Arc;

use aos_air_types::ReducerAbi;
use aos_host::control::{ControlClient, ControlServer, RequestEnvelope};
use aos_host::{WorldHost, config::HostConfig};
use aos_kernel::Kernel;
use aos_kernel::journal::mem::MemJournal;
use aos_wasm_abi::ReducerOutput;
use base64::prelude::*;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};

// Reuse helper utilities
#[path = "helpers.rs"]
mod helpers;
use helpers::fixtures;
use helpers::fixtures::{START_SCHEMA, TestStore};

/// End-to-end control channel over Unix socket: send-event -> step -> query-state -> shutdown.
#[tokio::test]
async fn control_channel_round_trip() {
    let store: Arc<TestStore> = fixtures::new_mem_store();

    // Build simple manifest: reducer sets fixed state when invoked.
    let reducer_output = ReducerOutput {
        state: Some(vec![0xAA]),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };
    let mut reducer = fixtures::stub_reducer_module(&store, "com.acme/Echo@1", &reducer_output);
    reducer.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema(START_SCHEMA),
        event: fixtures::schema(START_SCHEMA),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let mut manifest = fixtures::build_loaded_manifest(
        vec![],
        vec![],
        vec![reducer],
        vec![fixtures::routing_event(START_SCHEMA, "com.acme/Echo@1")],
    );
    helpers::insert_test_schemas(
        &mut manifest,
        vec![helpers::def_text_record_schema(
            START_SCHEMA,
            vec![("id", helpers::text_type())],
        )],
    );

    let kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();
    let host = WorldHost::from_kernel(kernel, store.clone(), HostConfig::default());

    let (control_tx, control_rx) = mpsc::channel(16);
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    // Start daemon
    let mut daemon = aos_host::WorldDaemon::new(host, control_rx, shutdown_rx, None);
    let daemon_handle = tokio::spawn(async move { daemon.run().await });

    // Start control server
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

    // Wait for socket to appear
    for _ in 0..20 {
        if sock_path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // Client
    let mut client = ControlClient::connect(&sock_path).await.unwrap();

    // send-event
    let evt = RequestEnvelope {
        v: 1,
        id: "1".into(),
        cmd: "send-event".into(),
        payload: json!({
            "schema": START_SCHEMA,
            "value_b64": BASE64_STANDARD.encode(serde_cbor::to_vec(&serde_json::json!({"id": "x"})).unwrap())
        }),
    };
    let resp = client.request(&evt).await.unwrap();
    assert!(resp.ok, "send-event failed: {:?}", resp.error);

    // query-state
    let query = RequestEnvelope {
        v: 1,
        id: "2".into(),
        cmd: "query-state".into(),
        payload: json!({ "reducer": "com.acme/Echo@1" }),
    };
    let resp = client.request(&query).await.unwrap();
    assert!(resp.ok);
    let state_b64 = resp
        .result
        .and_then(|v| v.get("state_b64").cloned())
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .expect("missing state_b64");
    let state = BASE64_STANDARD.decode(state_b64).unwrap();
    assert_eq!(state, vec![0xAA]);

    // defs-get
    let defs_get = RequestEnvelope {
        v: 1,
        id: "defs".into(),
        cmd: "defs-get".into(),
        payload: json!({ "name": START_SCHEMA }),
    };
    let resp = client.request(&defs_get).await.unwrap();
    assert!(resp.ok);
    let def_kind = resp
        .result
        .as_ref()
        .and_then(|v| v.get("def"))
        .and_then(|d| d.get("$kind"))
        .and_then(|k| k.as_str())
        .unwrap_or_default();
    assert_eq!(def_kind, "defschema");

    // defs-ls
    let defs_ls = RequestEnvelope {
        v: 1,
        id: "defs-ls".into(),
        cmd: "defs-ls".into(),
        payload: json!({ "kinds": ["schema"], "prefix": "demo/" }),
    };
    let resp = client.request(&defs_ls).await.unwrap();
    assert!(resp.ok);
    let defs = resp
        .result
        .as_ref()
        .and_then(|v| v.get("defs"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        defs.iter()
            .any(|d| d.get("name").and_then(|n| n.as_str()) == Some(START_SCHEMA)),
        "defs-ls should include the schema"
    );

    // query-state with key_b64 on a non-keyed reducer should return null (state keyed lookup unsupported)
    let query_key = RequestEnvelope {
        v: 1,
        id: "2b".into(),
        cmd: "query-state".into(),
        payload: json!({ "reducer": "com.acme/Echo@1", "key_b64": BASE64_STANDARD.encode(b"k1") }),
    };
    let resp = client.request(&query_key).await.unwrap();
    assert!(resp.ok);
    let state_b64 = resp.result.and_then(|v| v.get("state_b64").cloned());
    assert!(state_b64.is_none() || state_b64 == Some(serde_json::Value::Null));

    // shutdown daemon via control
    let shutdown_cmd = RequestEnvelope {
        v: 1,
        id: "4".into(),
        cmd: "shutdown".into(),
        payload: json!({}),
    };
    let resp = client.request(&shutdown_cmd).await.unwrap();
    assert!(resp.ok);

    // stop server loop
    let _ = shutdown_tx.send(());

    // Await daemon exit
    daemon_handle.await.unwrap().unwrap();
}

/// Control errors: unknown method and invalid request.
#[tokio::test]
async fn control_channel_errors() {
    let store: Arc<TestStore> = fixtures::new_mem_store();
    let manifest = fixtures::build_loaded_manifest(vec![], vec![], vec![], vec![]);
    let kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();
    let host = WorldHost::from_kernel(kernel, store.clone(), HostConfig::default());

    let (control_tx, control_rx) = mpsc::channel(4);
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let mut daemon = aos_host::WorldDaemon::new(host, control_rx, shutdown_rx, None);
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

    // Unknown method
    let resp = client
        .request(&RequestEnvelope {
            v: 1,
            id: "u".into(),
            cmd: "does-not-exist".into(),
            payload: json!({}),
        })
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("unknown_method")
    );

    // Invalid request: missing schema
    let resp = client
        .request(&RequestEnvelope {
            v: 1,
            id: "inv".into(),
            cmd: "send-event".into(),
            payload: json!({ "value_b64": "" }),
        })
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("invalid_request")
    );

    let _ = shutdown_tx.send(());
    daemon_handle.await.unwrap().unwrap();
}

/// put-blob control verb: store data and get hash back.
#[tokio::test]
async fn control_channel_put_blob() {
    let store: Arc<TestStore> = fixtures::new_mem_store();
    let manifest = fixtures::build_loaded_manifest(vec![], vec![], vec![], vec![]);
    let kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();
    let host = WorldHost::from_kernel(kernel, store.clone(), HostConfig::default());

    let (control_tx, control_rx) = mpsc::channel(4);
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let mut daemon = aos_host::WorldDaemon::new(host, control_rx, shutdown_rx, None);
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

    let data = b"hello control blob";
    let resp = client.put_blob("put", data).await.unwrap();
    assert!(resp.ok);
    let hash = resp
        .result
        .and_then(|v| v.get("hash").cloned())
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .expect("hash");
    assert!(!hash.is_empty());

    let _ = shutdown_tx.send(());
    daemon_handle.await.unwrap().unwrap();
}
