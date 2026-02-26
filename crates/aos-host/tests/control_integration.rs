use std::sync::Arc;

use aos_air_types::ReducerAbi;
use aos_host::control::{ControlClient, ControlServer, RequestEnvelope};
use aos_host::{WorldHost, config::HostConfig};
use aos_kernel::Kernel;
use aos_kernel::journal::JournalRecord;
use aos_kernel::journal::mem::MemJournal;
use aos_wasm_abi::ReducerOutput;
use base64::prelude::*;
use serde_json::json;
use std::os::unix::net::UnixListener;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};

// Reuse helper utilities
#[path = "helpers.rs"]
mod helpers;
use helpers::fixtures;
use helpers::fixtures::{START_SCHEMA, TestStore};
const SESSION_EVENT_SCHEMA: &str = "aos.agent/SessionEvent@1";

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

/// End-to-end control channel over Unix socket: event-send -> state-get -> shutdown.
#[tokio::test]
async fn control_channel_round_trip() {
    if !control_socket_allowed() {
        eprintln!("skipping control_channel_round_trip: control socket bind/connect not permitted");
        return;
    }

    let store: Arc<TestStore> = fixtures::new_mem_store();

    // Build simple manifest: reducers set fixed state when invoked.
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
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let session_reducer_output = ReducerOutput {
        state: Some(vec![0xAB]),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };
    let mut session_reducer = fixtures::stub_reducer_module(
        &store,
        "com.acme/SessionEventEcho@1",
        &session_reducer_output,
    );
    session_reducer.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema(SESSION_EVENT_SCHEMA),
        event: fixtures::schema(SESSION_EVENT_SCHEMA),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let mut manifest = fixtures::build_loaded_manifest(
        vec![],
        vec![],
        vec![reducer, session_reducer],
        vec![
            fixtures::routing_event(START_SCHEMA, "com.acme/Echo@1"),
            fixtures::routing_event(SESSION_EVENT_SCHEMA, "com.acme/SessionEventEcho@1"),
        ],
    );
    helpers::insert_test_schemas(
        &mut manifest,
        vec![
            helpers::def_text_record_schema(START_SCHEMA, vec![("id", helpers::text_type())]),
            helpers::def_text_record_schema(
                SESSION_EVENT_SCHEMA,
                vec![
                    ("session_id", helpers::text_type()),
                    ("run_id", helpers::text_type()),
                    ("turn_id", helpers::text_type()),
                    ("step_id", helpers::text_type()),
                    ("event", helpers::text_type()),
                ],
            ),
        ],
    );

    let kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();
    let host = WorldHost::from_kernel(kernel, store.clone(), HostConfig::default());

    let (control_tx, control_rx) = mpsc::channel(16);
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    // Start daemon
    let mut daemon = aos_host::WorldDaemon::new(host, control_rx, shutdown_rx, None, None);
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

    // event-send
    let evt = RequestEnvelope {
        v: 1,
        id: "1".into(),
        cmd: "event-send".into(),
        payload: json!({
            "schema": START_SCHEMA,
            "value_b64": BASE64_STANDARD.encode(serde_cbor::to_vec(&serde_json::json!({"id": "x"})).unwrap())
        }),
    };
    let resp = client.request(&evt).await.unwrap();
    assert!(resp.ok, "event-send failed: {:?}", resp.error);

    // session lineage event (for correlation/lineage contract coverage)
    let session_evt = RequestEnvelope {
        v: 1,
        id: "1-session".into(),
        cmd: "event-send".into(),
        payload: json!({
            "schema": SESSION_EVENT_SCHEMA,
            "value_b64": BASE64_STANDARD.encode(
                serde_cbor::to_vec(&serde_json::json!({
                    "session_id": "sess-1",
                    "run_id": "run-1",
                    "turn_id": "turn-1",
                    "step_id": "step-1",
                    "event": "RunStarted"
                })).unwrap()
            )
        }),
    };
    let resp = client.request(&session_evt).await.unwrap();
    assert!(resp.ok, "session event-send failed: {:?}", resp.error);

    let journal_all = RequestEnvelope {
        v: 1,
        id: "journal-all".into(),
        cmd: "journal-list".into(),
        payload: json!({ "from": 0, "limit": 100 }),
    };
    let resp = client.request(&journal_all).await.unwrap();
    assert!(resp.ok, "journal-list failed: {:?}", resp.error);
    let all_entries = resp
        .result
        .as_ref()
        .and_then(|v| v.get("entries"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        all_entries.iter().any(|entry| {
            entry
                .get("kind")
                .and_then(|v| v.as_str())
                .map(|k| k == "domain_event")
                .unwrap_or(false)
        }),
        "journal-list should include domain_event entries"
    );
    let all_seqs: Vec<u64> = all_entries
        .iter()
        .filter_map(|entry| entry.get("seq").and_then(|v| v.as_u64()))
        .collect();
    assert!(
        all_seqs.len() >= 2,
        "expected at least two journal entries for pagination test"
    );
    assert!(
        all_seqs.windows(2).all(|w| w[0] < w[1]),
        "journal-list entries should be strictly ordered by seq"
    );

    let journal_filtered = RequestEnvelope {
        v: 1,
        id: "journal-filtered".into(),
        cmd: "journal-list".into(),
        payload: json!({ "from": 0, "limit": 100, "kinds": ["domain_event"] }),
    };
    let resp = client.request(&journal_filtered).await.unwrap();
    assert!(resp.ok, "journal-list filtered failed: {:?}", resp.error);
    let filtered_entries = resp
        .result
        .as_ref()
        .and_then(|v| v.get("entries"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(!filtered_entries.is_empty(), "expected filtered entries");
    assert!(
        filtered_entries.iter().all(|entry| {
            entry
                .get("kind")
                .and_then(|v| v.as_str())
                .map(|k| k == "domain_event")
                .unwrap_or(false)
        }),
        "filtered journal-list should only include domain_event entries"
    );

    let journal_page_1 = RequestEnvelope {
        v: 1,
        id: "journal-page-1".into(),
        cmd: "journal-list".into(),
        payload: json!({ "from": 0, "limit": 2 }),
    };
    let resp = client.request(&journal_page_1).await.unwrap();
    assert!(resp.ok, "journal-list page 1 failed: {:?}", resp.error);
    let page_1_entries = resp
        .result
        .as_ref()
        .and_then(|v| v.get("entries"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        !page_1_entries.is_empty(),
        "expected non-empty first page from journal-list"
    );
    let page_1_seqs: Vec<u64> = page_1_entries
        .iter()
        .filter_map(|entry| entry.get("seq").and_then(|v| v.as_u64()))
        .collect();
    let resume_from = *page_1_seqs.last().expect("page 1 seq cursor");

    let journal_page_2 = RequestEnvelope {
        v: 1,
        id: "journal-page-2".into(),
        cmd: "journal-list".into(),
        payload: json!({ "from": resume_from, "limit": 100 }),
    };
    let resp = client.request(&journal_page_2).await.unwrap();
    assert!(resp.ok, "journal-list page 2 failed: {:?}", resp.error);
    let page_2_entries = resp
        .result
        .as_ref()
        .and_then(|v| v.get("entries"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let page_2_seqs: Vec<u64> = page_2_entries
        .iter()
        .filter_map(|entry| entry.get("seq").and_then(|v| v.as_u64()))
        .collect();
    assert!(
        page_2_seqs.iter().all(|seq| !page_1_seqs.contains(seq)),
        "resume-from-cursor should not duplicate durable entries"
    );

    let mut combined = page_1_seqs.clone();
    combined.extend(page_2_seqs.iter().copied());
    combined.sort_unstable();
    assert_eq!(
        combined, all_seqs,
        "paginated journal-list should reconstruct full ordered entry sequence"
    );

    // trace-get by event hash
    let head = RequestEnvelope {
        v: 1,
        id: "head".into(),
        cmd: "journal-head".into(),
        payload: json!({}),
    };
    let resp = client.request(&head).await.unwrap();
    assert!(resp.ok, "journal-head failed: {:?}", resp.error);
    let to = resp
        .result
        .as_ref()
        .and_then(|v| v.get("meta"))
        .and_then(|v| v.get("journal_height"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let journal_scan = RequestEnvelope {
        v: 1,
        id: "scan".into(),
        cmd: "journal-list".into(),
        payload: json!({ "from": 0, "limit": to }),
    };
    let resp = client.request(&journal_scan).await.unwrap();
    assert!(resp.ok, "journal scan failed: {:?}", resp.error);
    let entries = resp
        .result
        .as_ref()
        .and_then(|v| v.get("entries"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let event_hash = entries
        .iter()
        .find(|entry| entry.get("kind").and_then(|v| v.as_str()) == Some("domain_event"))
        .and_then(|entry| entry.get("record"))
        .and_then(|record| {
            serde_json::from_value::<JournalRecord>(record.clone())
                .ok()
                .and_then(|record| match record {
                    JournalRecord::DomainEvent(rec) => Some(rec.event_hash),
                    _ => None,
                })
        })
        .expect("domain event hash");

    let trace = RequestEnvelope {
        v: 1,
        id: "trace".into(),
        cmd: "trace-get".into(),
        payload: json!({ "event_hash": event_hash, "window_limit": 64 }),
    };
    let resp = client.request(&trace).await.unwrap();
    assert!(resp.ok, "trace-get failed: {:?}", resp.error);
    let trace = resp.result.expect("trace result");
    assert!(trace.get("root_event").is_some());
    assert!(trace.get("root").is_some());
    assert!(trace.get("journal_window").is_some());
    assert!(trace.get("live_wait").is_some());
    assert!(trace.get("terminal_state").is_some());

    // trace-get by correlation (schema + field + value)
    let trace = RequestEnvelope {
        v: 1,
        id: "trace-correlation".into(),
        cmd: "trace-get".into(),
        payload: json!({
            "schema": START_SCHEMA,
            "correlate_by": "id",
            "value": "x",
            "window_limit": 64
        }),
    };
    let resp = client.request(&trace).await.unwrap();
    assert!(resp.ok, "trace-get correlation failed: {:?}", resp.error);
    let trace = resp.result.expect("trace correlation result");
    assert_eq!(trace["query"]["schema"], START_SCHEMA);
    assert_eq!(trace["query"]["correlate_by"], "id");
    assert_eq!(trace["query"]["value"], "x");
    assert_eq!(trace["root"]["event_hash"], event_hash);

    // trace-get by session lineage correlation
    let trace = RequestEnvelope {
        v: 1,
        id: "trace-session-correlation".into(),
        cmd: "trace-get".into(),
        payload: json!({
            "schema": SESSION_EVENT_SCHEMA,
            "correlate_by": "session_id",
            "value": "sess-1",
            "window_limit": 64
        }),
    };
    let resp = client.request(&trace).await.unwrap();
    assert!(
        resp.ok,
        "trace-get session correlation failed: {:?}",
        resp.error
    );
    let trace = resp.result.expect("trace session correlation result");
    assert_eq!(trace["query"]["schema"], SESSION_EVENT_SCHEMA);
    assert_eq!(trace["query"]["correlate_by"], "session_id");
    assert_eq!(trace["query"]["value"], "sess-1");
    let root_value = trace
        .get("root")
        .and_then(|v| v.get("value"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(root_value["session_id"], "sess-1");
    assert_eq!(root_value["run_id"], "run-1");
    assert_eq!(root_value["turn_id"], "turn-1");
    assert_eq!(root_value["step_id"], "step-1");

    let trace_summary = RequestEnvelope {
        v: 1,
        id: "trace-summary".into(),
        cmd: "trace-summary".into(),
        payload: json!({}),
    };
    let resp = client.request(&trace_summary).await.unwrap();
    assert!(resp.ok, "trace-summary failed: {:?}", resp.error);
    let summary = resp.result.expect("trace summary result");
    assert!(summary.get("totals").is_some());

    // journal-list should include domain_event entries
    // state-get
    let query = RequestEnvelope {
        v: 1,
        id: "2".into(),
        cmd: "state-get".into(),
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

    // defs-list
    let defs_ls = RequestEnvelope {
        v: 1,
        id: "def-list".into(),
        cmd: "def-list".into(),
        payload: json!({ "kinds": ["schema"], "prefix": "com.acme/" }),
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
        "defs-list should include the schema"
    );

    // state-get with key_b64 on a non-keyed reducer should return null (state keyed lookup unsupported)
    let query_key = RequestEnvelope {
        v: 1,
        id: "2b".into(),
        cmd: "state-get".into(),
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
    if !control_socket_allowed() {
        eprintln!("skipping control_channel_errors: control socket bind/connect not permitted");
        return;
    }

    let store: Arc<TestStore> = fixtures::new_mem_store();
    let manifest = fixtures::build_loaded_manifest(vec![], vec![], vec![], vec![]);
    let kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();
    let host = WorldHost::from_kernel(kernel, store.clone(), HostConfig::default());

    let (control_tx, control_rx) = mpsc::channel(4);
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
            cmd: "event-send".into(),
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

/// blob-put control verb: store data and get hash back.
#[tokio::test]
async fn control_channel_put_blob() {
    if !control_socket_allowed() {
        eprintln!("skipping control_channel_put_blob: control socket bind/connect not permitted");
        return;
    }

    let store: Arc<TestStore> = fixtures::new_mem_store();
    let manifest = fixtures::build_loaded_manifest(vec![], vec![], vec![], vec![]);
    let kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();
    let host = WorldHost::from_kernel(kernel, store.clone(), HostConfig::default());

    let (control_tx, control_rx) = mpsc::channel(4);
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
