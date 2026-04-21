mod common;

use std::convert::Infallible;
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aos_cbor::Hash;
use aos_effects::ReceiptStatus;
use aos_effects::builtins::HostExecProgressFrame;
use aos_kernel::journal::{JournalRecord, OwnedJournalEntry};
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use fabric_protocol::{
    ControllerExecRequest, ExecEvent, ExecEventKind, ExecId, FabricBytes, SessionId,
};
use serde::Deserialize;
use serde_cbor::Value as CborValue;
use serde_json::json;
use serial_test::serial;
use tempfile::tempdir;
use tokio::task::JoinHandle;

use common::*;

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn embedded_duplicate_submission_id_is_rejected_without_double_apply() {
    let runtime = embedded_runtime(1);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone()).spawn().unwrap();
    let world = create_counter_world(&runtime, hosted_universe_id(&runtime));

    runtime
        .submit_event(aos_node::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 2 } }),
            submission_id: Some("dup-start".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    wait_for_worker(&mut supervisor).await;

    runtime
        .submit_event(aos_node::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Tick": null }),
            submission_id: Some("dup-tick".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    wait_for_worker(&mut supervisor).await;

    runtime
        .submit_event(aos_node::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Tick": null }),
            submission_id: Some("dup-tick".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    wait_for_worker(&mut supervisor).await;

    let state = runtime
        .state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap()
        .unwrap();
    assert_eq!(state["pc"], json!({ "$tag": "Counting" }));
    assert_eq!(state["remaining"], 1);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn embedded_failed_batch_commit_rolls_back_live_state_and_retries_cleanly() {
    let runtime = embedded_runtime(1);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone()).spawn().unwrap();
    let world = create_counter_world(&runtime, hosted_universe_id(&runtime));

    let initial_state = runtime
        .state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap();

    runtime.debug_fail_next_batch_commit().unwrap();
    runtime
        .submit_event(aos_node::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 1 } }),
            submission_id: Some("rollback-start".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_failed_batch = false;
    while Instant::now() < deadline {
        if supervisor
            .wait_for_progress(Duration::from_millis(5))
            .await
            .is_err()
        {
            saw_failed_batch = true;
            break;
        }
    }
    assert!(saw_failed_batch);

    let state_after_failure = runtime
        .state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap();
    assert_eq!(state_after_failure, initial_state);

    drop(supervisor);
    let mut recovered_supervisor = worker.with_worker_runtime(runtime.clone()).spawn().unwrap();
    wait_for_worker(&mut recovered_supervisor).await;

    let retried_state = runtime
        .state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap()
        .unwrap();
    assert_eq!(retried_state["pc"], json!({ "$tag": "Counting" }));
    assert_eq!(retried_state["remaining"], 1);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn embedded_worker_executes_timer_effects_inline() {
    let runtime = embedded_runtime(1);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone()).spawn().unwrap();
    let universe_id = hosted_universe_id(&runtime);
    let world = seed_timer_world(&runtime, universe_id);

    runtime
        .submit_event(aos_node::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/TimerEvent@1".into(),
            value: json!({
                "Start": {
                    "deliver_at_ns": 1,
                    "key": "hosted-inline-timer"
                }
            }),
            submission_id: Some("embedded-inline-timer-start".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        wait_for_worker(&mut supervisor).await;
        if let Some(state) = runtime
            .state_json(world.universe_id, world.world_id, "demo/TimerSM@1", None)
            .unwrap()
            && state["pc"] == json!({ "$tag": "Done" })
        {
            assert_eq!(state["fired_key"], json!("hosted-inline-timer"));
            return;
        }
    }

    let state = runtime
        .state_json(world.universe_id, world.world_id, "demo/TimerSM@1", None)
        .unwrap()
        .unwrap();
    assert_eq!(state["pc"], json!({ "$tag": "Done" }));
    assert_eq!(state["fired_key"], json!("hosted-inline-timer"));
}

#[test]
#[serial]
fn embedded_runtimes_can_share_same_state_root_cas() {
    let root = tempdir().unwrap();
    let runtime_a =
        aos_node::HostedWorkerRuntime::new_embedded_kafka_with_state_root(1, root.path()).unwrap();
    let runtime_b =
        aos_node::HostedWorkerRuntime::new_embedded_kafka_with_state_root(1, root.path()).unwrap();

    let universe_id = hosted_universe_id(&runtime_a);
    let payload = br#"shared-root-cas-payload"#;
    let hash = runtime_a.put_blob(universe_id, payload).unwrap();

    assert_eq!(hash, Hash::of_bytes(payload));
    assert_eq!(runtime_b.get_blob(universe_id, hash).unwrap(), payload);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn embedded_create_world_publishes_initial_checkpoint() {
    let runtime = embedded_runtime(1);
    let worker = aos_node::HostedWorker::new(worker_config());
    let mut supervisor = worker.with_worker_runtime(runtime.clone()).spawn().unwrap();
    let universe_id = hosted_universe_id(&runtime);
    let manifest_hash = upload_counter_manifest(&runtime, universe_id);
    let world = create_world_from_manifest(&runtime, universe_id, manifest_hash);

    wait_for_worker(&mut supervisor).await;

    let checkpoint = wait_for_checkpoint(&runtime, &mut supervisor, &world).await;
    assert_eq!(checkpoint.universe_id, world.universe_id);
    assert_eq!(checkpoint.world_id, world.world_id);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn embedded_fabric_exec_progress_is_admitted_through_runtime_and_kernel() {
    let fake = FakeFabricController::start().await;
    let vars = [
        ("AOS_FABRIC_CONTROLLER_URL", Some(fake.base_url.as_str())),
        ("AOS_FABRIC_EXEC_PROGRESS_INTERVAL_SECS", Some("1")),
        ("AOS_ADAPTER_ROUTES", None),
    ];

    temp_env::async_with_vars(vars, async {
        let runtime = embedded_runtime(1);
        let worker = hosted_worker();
        let mut supervisor = worker.with_worker_runtime(runtime.clone()).spawn().unwrap();
        let universe_id = hosted_universe_id(&runtime);
        let manifest_hash = upload_fabric_exec_progress_manifest(&runtime, universe_id);
        let world = create_world_from_manifest(&runtime, universe_id, manifest_hash);

        runtime
            .submit_event(aos_node::SubmitEventRequest {
                universe_id: world.universe_id,
                world_id: world.world_id,
                schema: "demo/FabricExecProgressEvent@1".into(),
                value: json!({
                    "Start": {
                        "session_id": "fabric-session-1"
                    }
                }),
                submission_id: Some("fabric-exec-progress-start".into()),
                expected_world_epoch: Some(world.world_epoch),
            })
            .unwrap();

        let (stream_record, receipt_record) =
            wait_for_fabric_exec_progress_journal(&runtime, &mut supervisor, &world).await;

        assert_eq!(stream_record.adapter_id, "host.exec.fabric");
        assert_eq!(stream_record.origin_module_id, "demo/FabricExecProgress@1");
        assert_eq!(stream_record.origin_instance_key, None);
        assert_eq!(stream_record.effect_kind, "host.exec");
        assert_eq!(stream_record.seq, 1);
        assert_eq!(stream_record.frame_kind, "host.exec.progress");
        let progress: HostExecProgressFrame =
            serde_cbor::from_slice(&stream_record.payload_cbor).unwrap();
        assert_eq!(progress.exec_id.as_deref(), Some("fake-exec"));
        assert_eq!(progress.stdout_delta, b"e2e-progress\n");
        assert_eq!(progress.stdout_bytes, "e2e-progress\n".len() as u64);

        assert_eq!(receipt_record.adapter_id, "host.exec.fabric");
        assert_eq!(receipt_record.status, ReceiptStatus::Ok);
        let receipt: JournalHostExecReceipt =
            serde_cbor::from_slice(&receipt_record.payload_cbor).unwrap();
        assert_eq!(receipt.exit_code, 0);
        assert_eq!(receipt.status, "ok");
        assert_eq!(
            host_output_text(receipt.stdout.as_ref().unwrap()),
            "e2e-progress\n"
        );

        let state = runtime
            .state_json(
                world.universe_id,
                world.world_id,
                "demo/FabricExecProgress@1",
                None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(state["pc"], json!({ "$tag": "Done" }));
        assert_eq!(state["progress_frames"], 1);
        assert_eq!(state["last_stream_seq"], 1);
        assert_eq!(state["last_stream_kind"], "host.exec.progress");
        assert_eq!(state["receipt_status"], "ok");
    })
    .await;
}

async fn wait_for_fabric_exec_progress_journal(
    runtime: &aos_node::HostedWorkerRuntime,
    supervisor: &mut aos_node::WorkerSupervisorHandle,
    world: &aos_node::HostedWorldSummary,
) -> (
    aos_kernel::journal::StreamFrameRecord,
    aos_kernel::journal::EffectReceiptRecord,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        wait_for_worker(supervisor).await;
        let records = journal_records(runtime, world);
        assert_no_identity_mismatch(&records);
        let stream = records.iter().find_map(|record| match record {
            JournalRecord::StreamFrame(frame) if frame.frame_kind == "host.exec.progress" => {
                Some(frame.clone())
            }
            _ => None,
        });
        let receipt = records.iter().find_map(|record| match record {
            JournalRecord::EffectReceipt(receipt) if receipt.adapter_id == "host.exec.fabric" => {
                Some(receipt.clone())
            }
            _ => None,
        });
        if let (Some(stream), Some(receipt)) = (stream, receipt) {
            return (stream, receipt);
        }
    }

    let records = journal_records(runtime, world);
    panic!("timed out waiting for Fabric exec progress and receipt; records: {records:?}");
}

fn journal_records(
    runtime: &aos_node::HostedWorkerRuntime,
    world: &aos_node::HostedWorldSummary,
) -> Vec<JournalRecord> {
    runtime
        .journal_entries_raw(world.universe_id, world.world_id, 0, 256)
        .unwrap()
        .entries
        .into_iter()
        .map(|entry| {
            let entry: OwnedJournalEntry = serde_cbor::from_slice(&entry.entry_cbor).unwrap();
            serde_cbor::from_slice::<JournalRecord>(&entry.payload).unwrap_or_else(|err| {
                panic!(
                    "decode journal record {:?} at seq {}: {err}",
                    entry.kind, entry.seq
                )
            })
        })
        .collect()
}

fn assert_no_identity_mismatch(records: &[JournalRecord]) {
    for record in records {
        if let JournalRecord::Custom(custom) = record {
            assert_ne!(custom.tag, "stream.identity_mismatch");
        }
    }
}

#[derive(Debug, Deserialize)]
struct JournalHostExecReceipt {
    exit_code: i32,
    status: String,
    stdout: Option<CborValue>,
}

fn host_output_text(output: &CborValue) -> String {
    let CborValue::Map(output) = output else {
        panic!("expected tagged host output map");
    };
    assert_eq!(
        output.get(&CborValue::Text("$tag".into())),
        Some(&CborValue::Text("inline_text".into()))
    );
    let Some(CborValue::Map(value)) = output.get(&CborValue::Text("$value".into())) else {
        panic!("expected inline_text payload");
    };
    let Some(CborValue::Text(text)) = value.get(&CborValue::Text("text".into())) else {
        panic!("expected inline_text.text");
    };
    text.clone()
}

#[derive(Default)]
struct FakeFabricControllerState {
    exec_requests: Mutex<Vec<ControllerExecRequest>>,
}

struct FakeFabricController {
    base_url: String,
    _state: Arc<FakeFabricControllerState>,
    _task: JoinHandle<()>,
}

impl FakeFabricController {
    async fn start() -> Self {
        let state = Arc::new(FakeFabricControllerState::default());
        let router = Router::new()
            .route("/v1/sessions/{session_id}/exec", post(exec_session))
            .with_state(state.clone());
        let addr = free_loopback_addr();
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            base_url: format!("http://{addr}"),
            _state: state,
            _task: task,
        }
    }
}

async fn exec_session(
    State(state): State<Arc<FakeFabricControllerState>>,
    Path(session_id): Path<String>,
    Json(request): Json<ControllerExecRequest>,
) -> impl IntoResponse {
    assert_eq!(
        SessionId(session_id),
        SessionId("fabric-session-1".to_string())
    );
    assert_eq!(request.argv, vec!["slow".to_string()]);
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
            data: Some(FabricBytes::from_bytes_auto(b"e2e-progress\n".to_vec())),
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
    let body = Body::from_stream(futures::stream::unfold(0, move |step| {
        let events = events.clone();
        async move {
            match step {
                0 => Some((Ok::<Bytes, Infallible>(event_line(&events[0])), 1)),
                1 => Some((Ok::<Bytes, Infallible>(event_line(&events[1])), 2)),
                2 => {
                    tokio::time::sleep(Duration::from_millis(1_200)).await;
                    Some((Ok::<Bytes, Infallible>(event_line(&events[2])), 3))
                }
                _ => None,
            }
        }
    }));
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        body,
    )
}

fn event_line(event: &ExecEvent) -> Bytes {
    Bytes::from(format!("{}\n", serde_json::to_string(event).unwrap()))
}

fn free_loopback_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}
