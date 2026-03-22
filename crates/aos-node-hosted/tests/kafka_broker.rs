mod common;

use std::sync::Arc;
use std::time::SystemTime;
use std::time::{Duration, Instant};

use aos_cbor::Hash;
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_effect_types::{HashRef, HttpRequestReceipt, RequestTimings};
use aos_effects::ReceiptStatus;
use aos_kernel::{KernelConfig, journal::JournalRecord};
use aos_node::{
    BlobPlane, CborPayload, CheckpointPlane, CreateWorldRequest, CreateWorldSource,
    LocalStatePaths, PartitionCheckpoint, PlaneError, PromotableBaselineRef, ReceiptIngress,
    UniverseId, WorldCheckpointRef, WorldId,
};
use aos_node_hosted::kafka::{
    CellProjectionUpsert, HostedKafkaBackend, ProjectionKey, ProjectionRecord,
    ProjectionTopicEntry, ProjectionValue,
};
use aos_node_hosted::materializer::{
    CellStateProjectionRecord, Materializer, MaterializerConfig, MaterializerSqliteStore,
};
use aos_node_hosted::worker::HostedWorkerRuntime;
use aos_node_hosted::{HostedWorldSummary, WorkerSupervisor};
use aos_runtime::{ExternalEvent, WorldConfig};
use serde_json::json;
use serial_test::serial;

use common::*;

const TEST_WAIT_SLEEP: Duration = Duration::from_millis(5);
const TEST_WAIT_DEADLINE: Duration = Duration::from_secs(60);

async fn wait_for_world_summary(
    control: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    universe_id: UniverseId,
    world_id: WorldId,
) -> HostedWorldSummary {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        supervisor.run_once().await.unwrap();
        if let Ok(world) = control.get_world(universe_id, world_id) {
            return world;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    control.get_world(universe_id, world_id).unwrap()
}

async fn wait_for_world_registration(
    runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    world: &HostedWorldSummary,
) -> HostedWorldSummary {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        supervisor.run_once().await.unwrap();
        if let Ok(summary) = runtime.get_world(world.universe_id, world.world_id) {
            return summary;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    runtime
        .get_world(world.universe_id, world.world_id)
        .unwrap()
}

async fn wait_for_counter_state(
    runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    world: &HostedWorldSummary,
    expected_pc: serde_json::Value,
    expected_remaining: i64,
) -> serde_json::Value {
    let deadline = Instant::now() + TEST_WAIT_DEADLINE;
    while Instant::now() < deadline {
        supervisor.run_once().await.unwrap();
        if let Some(state) = runtime
            .active_state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
            .unwrap()
            .or_else(|| {
                runtime
                    .state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
                    .unwrap()
            })
            && state["pc"] == expected_pc
            && state["remaining"] == expected_remaining
        {
            return state;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    let state = runtime
        .active_state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap()
        .or_else(|| {
            runtime
                .state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
                .unwrap()
        })
        .unwrap_or_else(|| {
            panic!(
                "counter state for world {} in universe {} did not materialize within {:?}",
                world.world_id, world.universe_id, TEST_WAIT_DEADLINE
            )
        });
    assert_eq!(state["pc"], expected_pc);
    assert_eq!(state["remaining"], expected_remaining);
    state
}

async fn wait_for_checkpoint(
    ctx: &BrokerRuntimeTestContext,
    supervisor: &mut WorkerSupervisor,
    world: &HostedWorldSummary,
) -> PartitionCheckpoint {
    let mut blobstore = ctx.blob_meta_for_universe(world.universe_id);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        supervisor.run_once().await.unwrap();
        blobstore
            .prime_latest_checkpoints(&ctx.kafka_config.journal_topic, 1)
            .unwrap();
        if let Some(checkpoint) = blobstore
            .latest_checkpoint(&ctx.kafka_config.journal_topic, world.effective_partition)
            .cloned()
            && checkpoint.worlds.iter().any(|entry| {
                entry.universe_id == world.universe_id && entry.world_id == world.world_id
            })
        {
            return checkpoint;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }

    let checkpoint = blobstore
        .latest_checkpoint(&ctx.kafka_config.journal_topic, world.effective_partition)
        .unwrap()
        .clone();
    assert!(
        checkpoint
            .worlds
            .iter()
            .any(|entry| entry.universe_id == world.universe_id && entry.world_id == world.world_id)
    );
    checkpoint
}

async fn wait_for_fetch_notify_state(
    runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    world: &HostedWorldSummary,
    expected_pc: serde_json::Value,
    expected_status: Option<i64>,
) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        supervisor.run_once().await.unwrap();
        if let Some(state) = runtime
            .state_json(
                world.universe_id,
                world.world_id,
                "demo/FetchNotify@1",
                None,
            )
            .unwrap()
            && state["pc"] == expected_pc
            && state["last_status"].as_i64() == expected_status
        {
            return state;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    let state = runtime
        .state_json(
            world.universe_id,
            world.world_id,
            "demo/FetchNotify@1",
            None,
        )
        .unwrap()
        .unwrap();
    assert_eq!(state["pc"], expected_pc);
    assert_eq!(state["last_status"].as_i64(), expected_status);
    state
}

async fn wait_for_timer_state(
    runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    world: &HostedWorldSummary,
    expected_pc: serde_json::Value,
) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        supervisor.run_once().await.unwrap();
        if let Some(state) = runtime
            .state_json(world.universe_id, world.world_id, "demo/TimerSM@1", None)
            .unwrap()
            && state["pc"] == expected_pc
        {
            return state;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    let state = runtime
        .state_json(world.universe_id, world.world_id, "demo/TimerSM@1", None)
        .unwrap()
        .unwrap();
    assert_eq!(state["pc"], expected_pc);
    state
}

async fn wait_for_world_trace(
    runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    world: &HostedWorldSummary,
    done: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match supervisor.run_once().await {
            Ok(_) => {}
            Err(aos_node_hosted::WorkerError::LogFirst(PlaneError::UnknownWorld {
                universe_id,
                world_id,
            })) if universe_id == world.universe_id && world_id == world.world_id => {
                tokio::time::sleep(TEST_WAIT_SLEEP).await;
                continue;
            }
            Err(err) => panic!("wait_for_world_trace supervisor error: {err:?}"),
        }
        let trace = runtime
            .trace_summary(world.universe_id, world.world_id)
            .unwrap();
        if done(&trace) {
            return trace;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    runtime
        .trace_summary(world.universe_id, world.world_id)
        .unwrap()
}

async fn wait_for_assigned_partitions(
    runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    expected: &[u32],
) -> Vec<u32> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        supervisor.run_once().await.unwrap();
        let assigned = runtime.assigned_partitions().unwrap();
        if assigned == expected {
            return assigned;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    let assigned = runtime.assigned_partitions().unwrap();
    assert_eq!(assigned, expected);
    assigned
}

fn kafka_reader_for(ctx: &BrokerRuntimeTestContext, label: &str) -> HostedKafkaBackend {
    let mut kafka_config = ctx.kafka_config.clone();
    kafka_config.submission_group_prefix =
        format!("{}-reader-{label}", kafka_config.submission_group_prefix);
    kafka_config.transactional_id = format!("{}-reader-{label}", kafka_config.transactional_id);
    HostedKafkaBackend::new(1, kafka_config).unwrap()
}

fn decode_projection_entry(entry: &ProjectionTopicEntry) -> ProjectionRecord {
    let key: ProjectionKey = serde_cbor::from_slice(&entry.key).expect("decode projection key");
    let value = entry.value.as_ref().map(|bytes| {
        serde_cbor::from_slice::<ProjectionValue>(bytes).expect("decode projection value")
    });
    ProjectionRecord { key, value }
}

fn latest_projection_token(entries: &[ProjectionTopicEntry], world_id: WorldId) -> Option<String> {
    entries.iter().rev().find_map(|entry| {
        let record = decode_projection_entry(entry);
        match (record.key, record.value) {
            (
                ProjectionKey::WorldMeta {
                    world_id: entry_world_id,
                },
                Some(ProjectionValue::WorldMeta(meta)),
            ) if entry_world_id == world_id => Some(meta.projection_token),
            _ => None,
        }
    })
}

async fn wait_for_projection_entries(
    runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    partition: u32,
    done: impl Fn(&[ProjectionTopicEntry]) -> bool,
) -> Vec<ProjectionTopicEntry> {
    let deadline = Instant::now() + TEST_WAIT_DEADLINE;
    while Instant::now() < deadline {
        supervisor.run_once().await.unwrap();
        let entries = runtime.projection_entries(partition).unwrap();
        if done(&entries) {
            return entries;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    let entries = runtime.projection_entries(partition).unwrap();
    assert!(
        done(&entries),
        "projection entries did not reach expected state"
    );
    entries
}

async fn wait_for_effect_intent_hash(
    ctx: &BrokerRuntimeTestContext,
    world: &HostedWorldSummary,
) -> [u8; 32] {
    let mut reader = kafka_reader_for(ctx, "intent");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        reader
            .recover_partition_from_broker(world.effective_partition)
            .unwrap();
        for frame in reader.world_frames(world.world_id) {
            for record in &frame.records {
                if let JournalRecord::EffectIntent(intent) = record {
                    return intent.intent_hash;
                }
            }
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    panic!(
        "timed out waiting for effect intent for world {} in universe {}",
        world.world_id, world.universe_id
    );
}

async fn wait_for_effect_journal_records(
    ctx: &BrokerRuntimeTestContext,
    world: &HostedWorldSummary,
    expected_intents: usize,
    expected_receipts: usize,
) -> (usize, usize) {
    let deadline = Instant::now() + TEST_WAIT_DEADLINE;
    while Instant::now() < deadline {
        let mut reader = kafka_reader_for(ctx, "effect-count");
        match reader.recover_partition_from_broker(world.effective_partition) {
            Ok(()) => {}
            Err(PlaneError::UnknownWorld {
                universe_id,
                world_id,
            }) if universe_id == world.universe_id && world_id == world.world_id => {
                tokio::time::sleep(TEST_WAIT_SLEEP).await;
                continue;
            }
            Err(err) => panic!("recover partition for effect journal scan failed: {err}"),
        }
        let mut intent_count = 0usize;
        let mut receipt_count = 0usize;
        for frame in reader.world_frames(world.world_id) {
            for record in &frame.records {
                match record {
                    JournalRecord::EffectIntent(_) => {
                        intent_count += 1;
                    }
                    JournalRecord::EffectReceipt(_) => {
                        receipt_count += 1;
                    }
                    _ => {}
                }
            }
        }
        if intent_count == expected_intents && receipt_count == expected_receipts {
            return (intent_count, receipt_count);
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    let mut reader = kafka_reader_for(ctx, "effect-count-final");
    reader
        .recover_partition_from_broker(world.effective_partition)
        .unwrap();
    let mut intent_count = 0usize;
    let mut receipt_count = 0usize;
    for frame in reader.world_frames(world.world_id) {
        for record in &frame.records {
            match record {
                JournalRecord::EffectIntent(_) => intent_count += 1,
                JournalRecord::EffectReceipt(_) => receipt_count += 1,
                _ => {}
            }
        }
    }
    (intent_count, receipt_count)
}

async fn create_fetch_notify_world(
    ctx: &BrokerRuntimeTestContext,
    control: &HostedWorkerRuntime,
    worker_runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
) -> HostedWorldSummary {
    let universe_id = hosted_universe_id(control);
    let world = seed_fetch_notify_world(ctx, control, universe_id);
    wait_for_world_registration(worker_runtime, supervisor, &world).await
}

fn unix_time_ns() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

async fn create_timer_world(
    control: &HostedWorkerRuntime,
    worker_runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
) -> HostedWorldSummary {
    let universe_id = hosted_universe_id(control);
    let world = seed_timer_world(control, universe_id);
    wait_for_world_registration(worker_runtime, supervisor, &world).await
}

async fn start_timer_and_wait_awaiting(
    control: &HostedWorkerRuntime,
    worker_runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    world: &HostedWorldSummary,
    deliver_at_ns: u64,
    key: &str,
) -> serde_json::Value {
    control
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/TimerEvent@1".into(),
            value: json!({
                "Start": {
                    "deliver_at_ns": deliver_at_ns,
                    "key": key,
                }
            }),
            submission_id: Some(format!("worker-timer-start-{}", uuid::Uuid::new_v4())),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        supervisor.run_once().await.unwrap();
        if let Some(state) = worker_runtime
            .state_json(world.universe_id, world.world_id, "demo/TimerSM@1", None)
            .unwrap()
            && matches!(
                state["pc"].as_object().and_then(|tag| tag.get("$tag")),
                Some(serde_json::Value::String(value))
                    if value == "Awaiting" || value == "Done"
            )
        {
            return state;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    worker_runtime
        .state_json(world.universe_id, world.world_id, "demo/TimerSM@1", None)
        .unwrap()
        .unwrap()
}

async fn complete_fetch_notify_flow(
    ctx: &BrokerRuntimeTestContext,
    control: &HostedWorkerRuntime,
    worker_runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    world: &HostedWorldSummary,
) -> serde_json::Value {
    control
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/FetchNotifyEvent@1".into(),
            value: json!({
                "Start": {
                    "url": "https://example.com/data.json",
                    "method": "GET"
                }
            }),
            submission_id: Some(format!("worker-receipt-start-{}", uuid::Uuid::new_v4())),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();

    let pending = wait_for_world_trace(worker_runtime, supervisor, world, |trace| {
        trace["runtime_wait"]["pending_workflow_receipts"]
            .as_u64()
            .is_some_and(|count| count > 0)
    })
    .await;
    assert_eq!(pending["runtime_wait"]["queued_effects"], 0);

    let fetching = wait_for_fetch_notify_state(
        worker_runtime,
        supervisor,
        world,
        json!({ "$tag": "Fetching" }),
        None,
    )
    .await;
    assert_eq!(fetching["pending_request"], 0);

    let intent_hash = wait_for_effect_intent_hash(ctx, world).await;
    let receipt_payload = HttpRequestReceipt {
        status: 200,
        headers: Default::default(),
        body_ref: Some(
            HashRef::new("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .unwrap(),
        ),
        timings: RequestTimings {
            start_ns: 10,
            end_ns: 20,
        },
        adapter_id: "adapter.http.test".into(),
    };
    control
        .submit_receipt(
            world.universe_id,
            world.world_id,
            ReceiptIngress {
                intent_hash: intent_hash.to_vec(),
                effect_kind: "http.request".into(),
                adapter_id: "adapter.http.test".into(),
                status: ReceiptStatus::Ok,
                payload: CborPayload::inline(serde_cbor::to_vec(&receipt_payload).unwrap()),
                cost_cents: Some(0),
                signature: vec![1, 2, 3],
                correlation_id: Some(format!("worker-receipt-{}", uuid::Uuid::new_v4())),
            },
        )
        .unwrap();

    wait_for_fetch_notify_state(
        worker_runtime,
        supervisor,
        world,
        json!({ "$tag": "Done" }),
        Some(200),
    )
    .await
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn worker_creates_world_from_ingress_and_publishes_initial_checkpoint() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(ctx) = broker_runtime_test_context("worker-create", 1) else {
        return;
    };
    let control = ctx.control_runtime("control");
    let worker_runtime = ctx.worker_runtime("worker");
    let worker = aos_node_hosted::HostedWorker::new(aos_node_hosted::config::HostedWorkerConfig {
        checkpoint_on_create: true,
        ..worker_config()
    });
    let mut supervisor = worker.with_worker_runtime(worker_runtime.clone());
    let universe_id = hosted_universe_id(&control);
    let manifest_hash = upload_counter_manifest(&control, universe_id);

    let accepted = control
        .create_world(
            universe_id,
            CreateWorldRequest {
                world_id: None,
                universe_id,
                created_at_ns: 1,
                source: CreateWorldSource::Manifest {
                    manifest_hash: manifest_hash.clone(),
                },
            },
        )
        .unwrap();
    assert_eq!(accepted.effective_partition, 0);

    let world =
        wait_for_world_summary(&control, &mut supervisor, universe_id, accepted.world_id).await;
    let checkpoint = wait_for_checkpoint(&ctx, &mut supervisor, &world).await;
    assert_eq!(world.manifest_hash, manifest_hash);
    assert_eq!(checkpoint.partition, 0);
    assert_eq!(checkpoint.journal_topic, ctx.kafka_config.journal_topic);
    assert!(
        checkpoint
            .worlds
            .iter()
            .any(|entry| entry.universe_id == world.universe_id && entry.world_id == world.world_id)
    );
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn worker_restart_with_stale_partition_checkpoint_metadata_preserves_post_checkpoint_event() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(ctx) = broker_runtime_test_context("worker-stale-checkpoint", 1) else {
        return;
    };
    let universe_id = UniverseId::from(uuid::Uuid::new_v4());
    let stale_world_id = WorldId::from(uuid::Uuid::new_v4());
    let stale_snapshot_hash = ctx
        .remote_cas_for_universe(universe_id)
        .put_blob(universe_id, b"stale-checkpoint-snapshot")
        .unwrap();
    let mut blob_meta = ctx.blob_meta_for_universe(universe_id);
    blob_meta
        .commit_checkpoint(PartitionCheckpoint {
            journal_topic: ctx.kafka_config.journal_topic.clone(),
            partition: 0,
            journal_offset: 10,
            created_at_ns: 1,
            worlds: vec![WorldCheckpointRef {
                universe_id,
                world_id: stale_world_id,
                world_epoch: 1,
                checkpointed_at_ns: 1,
                world_seq: 10,
                baseline: PromotableBaselineRef {
                    snapshot_ref: stale_snapshot_hash.to_hex(),
                    snapshot_manifest_ref: None,
                    manifest_hash: "sha256:stale".into(),
                    universe_id,
                    height: 10,
                    logical_time_ns: 0,
                    receipt_horizon_height: 10,
                },
            }],
        })
        .unwrap();

    let control = ctx.control_runtime_in_universe("control", universe_id);
    let worker_runtime = ctx.worker_runtime_in_universe("worker", universe_id);
    let worker = aos_node_hosted::HostedWorker::new(aos_node_hosted::config::HostedWorkerConfig {
        checkpoint_on_create: true,
        ..worker_config()
    });
    let mut supervisor = worker.with_worker_runtime(worker_runtime.clone());

    let manifest_hash = upload_counter_manifest(&control, universe_id);
    let accepted = control
        .create_world(
            universe_id,
            CreateWorldRequest {
                world_id: None,
                universe_id,
                created_at_ns: 1,
                source: CreateWorldSource::Manifest {
                    manifest_hash: manifest_hash.clone(),
                },
            },
        )
        .unwrap();
    let world =
        wait_for_world_summary(&control, &mut supervisor, universe_id, accepted.world_id).await;
    let checkpoint = wait_for_checkpoint(&ctx, &mut supervisor, &world).await;
    assert_eq!(checkpoint.journal_offset, 10);
    assert!(
        checkpoint.worlds.len() >= 2,
        "expected stale checkpoint world to be merged forward"
    );
    let world_checkpoint = checkpoint
        .worlds
        .iter()
        .find(|entry| entry.universe_id == world.universe_id && entry.world_id == world.world_id)
        .expect("checkpoint entry for freshly created world");
    let baseline_height = world_checkpoint.baseline.height;

    let local = Arc::new(
        aos_node::FsCas::open_cas_root(temp_state_root("stale-checkpoint-frame-store")).unwrap(),
    );
    let remote = Arc::new(ctx.remote_cas_for_universe(universe_id));
    let store = Arc::new(aos_node_hosted::blobstore::HostedCas::new(local, remote));
    let create_request = CreateWorldRequest {
        world_id: Some(world.world_id),
        universe_id,
        created_at_ns: 1,
        source: CreateWorldSource::Manifest {
            manifest_hash: manifest_hash.clone(),
        },
    };
    let mut kernel_config = KernelConfig::default();
    kernel_config.universe_id = universe_id.as_uuid();
    let created = aos_node::create_plane_world_from_request(
        store,
        &create_request,
        universe_id,
        world.world_id,
        world.world_epoch,
        WorldConfig::default(),
        EffectAdapterConfig::default(),
        kernel_config,
    )
    .unwrap();
    let mut host = created.host;
    host.compact_journal_through(baseline_height).unwrap();
    let actual_tail_start = host.journal_bounds().next_seq;
    host.enqueue_external(ExternalEvent::DomainEvent {
        schema: "demo/CounterEvent@1".into(),
        value: serde_cbor::to_vec(&json!({ "Start": { "target": 2 } })).unwrap(),
        key: None,
    })
    .unwrap();
    host.enqueue_external(ExternalEvent::DomainEvent {
        schema: "demo/CounterEvent@1".into(),
        value: serde_cbor::to_vec(&json!({ "Tick": null })).unwrap(),
        key: None,
    })
    .unwrap();
    host.enqueue_external(ExternalEvent::DomainEvent {
        schema: "demo/CounterEvent@1".into(),
        value: serde_cbor::to_vec(&json!({ "Tick": null })).unwrap(),
        key: None,
    })
    .unwrap();
    host.drain().unwrap();
    let actual_tail = host.kernel().dump_journal_from(actual_tail_start).unwrap();
    assert!(
        !actual_tail.is_empty(),
        "local counter replay fixture produced no post-checkpoint journal tail"
    );
    let actual_records = actual_tail
        .iter()
        .map(|entry| serde_cbor::from_slice::<JournalRecord>(&entry.payload).unwrap())
        .collect::<Vec<_>>();
    let mut writer = kafka_reader_for(&ctx, "stale-checkpoint-writer");
    let broker_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < broker_deadline {
        writer
            .recover_partition_from_broker(world.effective_partition)
            .unwrap();
        if writer.next_world_seq(world.world_id) >= baseline_height {
            break;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    let broker_frame_ranges = writer
        .world_frames(world.world_id)
        .iter()
        .map(|frame| format!("{}..{}", frame.world_seq_start, frame.world_seq_end))
        .collect::<Vec<_>>();
    let actual_frame_seq_start = writer.next_world_seq(world.world_id);
    let post_checkpoint_frame = aos_node::WorldLogFrame {
        format_version: 1,
        universe_id,
        world_id: world.world_id,
        world_epoch: world.world_epoch,
        world_seq_start: actual_frame_seq_start,
        world_seq_end: actual_frame_seq_start + actual_records.len() as u64 - 1,
        records: actual_records,
    };
    assert!(
        post_checkpoint_frame.world_seq_end > baseline_height,
        "synthetic post-checkpoint frame did not advance past the checkpoint baseline; baseline_height={baseline_height} frame={}..{} broker_frames={broker_frame_ranges:?}",
        post_checkpoint_frame.world_seq_start,
        post_checkpoint_frame.world_seq_end,
    );
    writer.append_frame(post_checkpoint_frame).unwrap();

    let mut persisted_reader = kafka_reader_for(&ctx, "stale-checkpoint-persisted");
    persisted_reader
        .recover_partition_from_broker(world.effective_partition)
        .unwrap();
    let persisted_frame_ranges = persisted_reader
        .world_frames(world.world_id)
        .iter()
        .map(|frame| format!("{}..{}", frame.world_seq_start, frame.world_seq_end))
        .collect::<Vec<_>>();
    assert!(
        persisted_reader
            .world_frames(world.world_id)
            .iter()
            .any(|frame| frame.world_seq_end > baseline_height),
        "post-checkpoint frame was never durably written before restart; baseline_height={baseline_height} frames={persisted_frame_ranges:?}"
    );

    drop(supervisor);
    drop(worker_runtime);
    drop(control);

    let recovered_worker_runtime = ctx.worker_runtime_in_universe("worker-recovered", universe_id);
    let mut recovered_supervisor = worker.with_worker_runtime(recovered_worker_runtime.clone());
    wait_for_world_registration(&recovered_worker_runtime, &mut recovered_supervisor, &world).await;

    let mut reader = kafka_reader_for(&ctx, "stale-checkpoint-recovered");
    reader
        .recover_partition_from_broker(world.effective_partition)
        .unwrap();
    let recovered_frame_ranges = reader
        .world_frames(world.world_id)
        .iter()
        .map(|frame| format!("{}..{}", frame.world_seq_start, frame.world_seq_end))
        .collect::<Vec<_>>();
    let recovered_trace = recovered_worker_runtime
        .trace_summary(universe_id, world.world_id)
        .ok()
        .map(|trace| trace.to_string())
        .unwrap_or_else(|| "<trace unavailable>".into());
    let recovered_state = recovered_worker_runtime
        .state_json(universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap()
        .unwrap_or_else(|| {
            panic!(
                "counter state should survive restart with its post-checkpoint event; frames={recovered_frame_ranges:?} trace={recovered_trace}"
            )
        });
    assert_eq!(recovered_state["pc"], json!({ "$tag": "Done" }));
    assert_eq!(recovered_state["remaining"], 0);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn worker_processes_ingress_submission_and_persists_world_state() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(ctx) = broker_runtime_test_context("worker-process", 1) else {
        return;
    };
    let worker_runtime = ctx.direct_worker_runtime("worker", &[0]);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(worker_runtime.clone());
    let world = seed_counter_world(&worker_runtime, hosted_universe_id(&worker_runtime));
    let world = wait_for_world_registration(&worker_runtime, &mut supervisor, &world).await;

    worker_runtime
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 1 } }),
            submission_id: Some(format!("worker-process-{}", uuid::Uuid::new_v4())),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();

    let state = wait_for_counter_state(
        &worker_runtime,
        &mut supervisor,
        &world,
        json!({ "$tag": "Counting" }),
        1,
    )
    .await;
    assert_eq!(state["remaining"], 1);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn worker_processes_receipt_and_clears_pending_runtime_work() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(ctx) = broker_runtime_test_context("worker-receipt", 1) else {
        return;
    };
    let control = ctx.control_runtime("control");
    let worker_runtime = ctx.worker_runtime("worker");
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(worker_runtime.clone());
    let world = create_fetch_notify_world(&ctx, &control, &worker_runtime, &mut supervisor).await;
    let done =
        complete_fetch_notify_flow(&ctx, &control, &worker_runtime, &mut supervisor, &world).await;
    assert_eq!(
        done["last_body_ref"],
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    let trace = wait_for_world_trace(&worker_runtime, &mut supervisor, &world, |trace| {
        trace["runtime_wait"]["pending_workflow_receipts"] == 0
            && trace["runtime_wait"]["queued_effects"] == 0
            && trace["strict_quiescence"]["inflight_workflow_intents"] == 0
            && trace["strict_quiescence"]["pending_workflow_receipts"] == 0
            && trace["strict_quiescence"]["queued_effects"] == 0
    })
    .await;
    assert_eq!(trace["runtime_wait"]["pending_workflow_receipts"], 0);
    assert_eq!(trace["runtime_wait"]["queued_effects"], 0);
    assert_eq!(trace["strict_quiescence"]["inflight_workflow_intents"], 0);
    assert_eq!(trace["strict_quiescence"]["pending_workflow_receipts"], 0);
    assert_eq!(trace["strict_quiescence"]["queued_effects"], 0);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn worker_processes_hosted_timer_and_advances_world() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(ctx) = broker_runtime_test_context("worker-timer", 1) else {
        return;
    };
    let control = ctx.control_runtime("control");
    let worker_runtime = ctx.worker_runtime("worker");
    let mut supervisor = hosted_worker().with_worker_runtime(worker_runtime.clone());
    let world = create_timer_world(&control, &worker_runtime, &mut supervisor).await;

    let deliver_at_ns = unix_time_ns() + 1_000_000_000;
    let awaiting = start_timer_and_wait_awaiting(
        &control,
        &worker_runtime,
        &mut supervisor,
        &world,
        deliver_at_ns,
        "basic",
    )
    .await;
    assert_eq!(awaiting["deadline_ns"], deliver_at_ns);
    assert_eq!(awaiting["key"], "basic");

    let done = wait_for_timer_state(
        &worker_runtime,
        &mut supervisor,
        &world,
        json!({ "$tag": "Done" }),
    )
    .await;
    assert_eq!(done["fired_key"], "basic");
    let trace = wait_for_world_trace(&worker_runtime, &mut supervisor, &world, |trace| {
        trace["runtime_wait"]["pending_workflow_receipts"] == 0
            && trace["strict_quiescence"]["pending_workflow_receipts"] == 0
            && trace["totals"]["effects"]["intents"] == 1
            && trace["totals"]["effects"]["receipts"]["ok"] == 1
    })
    .await;
    assert_eq!(trace["runtime_wait"]["pending_workflow_receipts"], 0);
    assert_eq!(trace["totals"]["effects"]["intents"], 1);
    assert_eq!(trace["totals"]["effects"]["receipts"]["ok"], 1);

    let (intent_count, _) = wait_for_effect_journal_records(&ctx, &world, 1, 0).await;
    assert_eq!(intent_count, 1);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn worker_restart_recovers_pending_timer_and_fires_once() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(ctx) = broker_runtime_test_context("worker-timer-restart", 1) else {
        return;
    };
    let control = ctx.control_runtime("control");
    let worker_runtime = ctx.worker_runtime("worker");
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(worker_runtime.clone());
    let world = create_timer_world(&control, &worker_runtime, &mut supervisor).await;

    let deliver_at_ns = unix_time_ns() + 1_000_000_000;
    start_timer_and_wait_awaiting(
        &control,
        &worker_runtime,
        &mut supervisor,
        &world,
        deliver_at_ns,
        "restart",
    )
    .await;
    let checkpoint = worker_runtime
        .checkpoint_partition(world.effective_partition)
        .unwrap();
    assert!(
        checkpoint
            .worlds
            .iter()
            .any(|entry| entry.universe_id == world.universe_id && entry.world_id == world.world_id)
    );

    drop(supervisor);
    drop(worker_runtime);
    drop(control);

    let _recovered_control = ctx.control_runtime("control-recovered");
    let recovered_worker_runtime = ctx.worker_runtime("worker-recovered");
    let mut recovered_supervisor = worker.with_worker_runtime(recovered_worker_runtime.clone());
    wait_for_world_registration(&recovered_worker_runtime, &mut recovered_supervisor, &world).await;

    let trace = wait_for_world_trace(
        &recovered_worker_runtime,
        &mut recovered_supervisor,
        &world,
        |trace| {
            trace["runtime_wait"]["pending_workflow_receipts"] == 0
                && trace["totals"]["effects"]["intents"] == 1
                && trace["totals"]["effects"]["receipts"]["ok"] == 1
        },
    )
    .await;
    assert_eq!(trace["runtime_wait"]["pending_workflow_receipts"], 0);
    assert_eq!(trace["totals"]["effects"]["receipts"]["ok"], 1);
    let (intent_count, _) = wait_for_effect_journal_records(&ctx, &world, 1, 0).await;
    assert_eq!(intent_count, 1);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn worker_failover_recovers_pending_timer_and_fires_once() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(ctx) = broker_runtime_test_context("worker-timer-failover", 1) else {
        return;
    };
    let control = ctx.control_runtime("control");
    let worker = hosted_worker();

    let worker_a_runtime = ctx.worker_runtime("worker-a");
    let mut worker_a = worker.with_worker_runtime(worker_a_runtime.clone());
    wait_for_assigned_partitions(&worker_a_runtime, &mut worker_a, &[0]).await;

    let world = seed_timer_world(&control, hosted_universe_id(&control));
    let world = wait_for_world_registration(&worker_a_runtime, &mut worker_a, &world).await;

    let deliver_at_ns = unix_time_ns() + 1_000_000_000;
    start_timer_and_wait_awaiting(
        &control,
        &worker_a_runtime,
        &mut worker_a,
        &world,
        deliver_at_ns,
        "failover",
    )
    .await;
    let checkpoint = worker_a_runtime
        .checkpoint_partition(world.effective_partition)
        .unwrap();
    assert!(
        checkpoint
            .worlds
            .iter()
            .any(|entry| entry.universe_id == world.universe_id && entry.world_id == world.world_id)
    );

    drop(worker_a);
    drop(worker_a_runtime);

    let worker_b_runtime = ctx.worker_runtime("worker-b");
    let mut worker_b = worker.with_worker_runtime(worker_b_runtime.clone());
    wait_for_assigned_partitions(&worker_b_runtime, &mut worker_b, &[0]).await;
    wait_for_world_registration(&worker_b_runtime, &mut worker_b, &world).await;

    let trace = wait_for_world_trace(&worker_b_runtime, &mut worker_b, &world, |trace| {
        trace["runtime_wait"]["pending_workflow_receipts"] == 0
            && trace["totals"]["effects"]["intents"] == 1
            && trace["totals"]["effects"]["receipts"]["ok"] == 1
    })
    .await;
    assert_eq!(trace["runtime_wait"]["pending_workflow_receipts"], 0);
    assert_eq!(trace["totals"]["effects"]["receipts"]["ok"], 1);

    let (intent_count, _) = wait_for_effect_journal_records(&ctx, &world, 1, 0).await;
    assert_eq!(intent_count, 1);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn worker_reopen_after_completion_does_not_resurrect_pending_runtime_work() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(ctx) = broker_runtime_test_context("worker-reopen", 1) else {
        return;
    };
    let control = ctx.control_runtime("control");
    let worker_runtime = ctx.worker_runtime("worker");
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(worker_runtime.clone());
    let world = create_fetch_notify_world(&ctx, &control, &worker_runtime, &mut supervisor).await;

    let done =
        complete_fetch_notify_flow(&ctx, &control, &worker_runtime, &mut supervisor, &world).await;
    assert_eq!(done["last_status"], 200);
    let pre_restart_trace =
        wait_for_world_trace(&worker_runtime, &mut supervisor, &world, |trace| {
            trace["runtime_wait"]["pending_workflow_receipts"] == 0
                && trace["runtime_wait"]["queued_effects"] == 0
                && trace["strict_quiescence"]["inflight_workflow_intents"] == 0
                && trace["strict_quiescence"]["pending_workflow_receipts"] == 0
                && trace["strict_quiescence"]["queued_effects"] == 0
        })
        .await;
    assert_eq!(
        pre_restart_trace["runtime_wait"]["pending_workflow_receipts"],
        0
    );
    assert_eq!(pre_restart_trace["runtime_wait"]["queued_effects"], 0);
    assert_eq!(
        pre_restart_trace["strict_quiescence"]["inflight_workflow_intents"],
        0
    );
    assert_eq!(
        pre_restart_trace["strict_quiescence"]["pending_workflow_receipts"],
        0
    );
    assert_eq!(pre_restart_trace["strict_quiescence"]["queued_effects"], 0);

    let checkpoint = worker_runtime
        .checkpoint_partition(world.effective_partition)
        .unwrap();
    assert!(
        checkpoint
            .worlds
            .iter()
            .any(|entry| entry.universe_id == world.universe_id && entry.world_id == world.world_id)
    );

    drop(supervisor);
    drop(worker_runtime);
    drop(control);

    let recovered_worker_runtime = ctx.worker_runtime("worker-recovered");
    let mut recovered_supervisor = worker.with_worker_runtime(recovered_worker_runtime.clone());
    let trace = wait_for_world_trace(
        &recovered_worker_runtime,
        &mut recovered_supervisor,
        &world,
        |trace| {
            trace["runtime_wait"]["pending_workflow_receipts"] == 0
                && trace["runtime_wait"]["queued_effects"] == 0
                && trace["strict_quiescence"]["inflight_workflow_intents"] == 0
                && trace["strict_quiescence"]["pending_workflow_receipts"] == 0
                && trace["strict_quiescence"]["queued_effects"] == 0
        },
    )
    .await;
    assert_eq!(trace["runtime_wait"]["pending_workflow_receipts"], 0);
    assert_eq!(trace["runtime_wait"]["queued_effects"], 0);
    assert_eq!(trace["strict_quiescence"]["inflight_workflow_intents"], 0);
    assert_eq!(trace["strict_quiescence"]["pending_workflow_receipts"], 0);
    assert_eq!(trace["strict_quiescence"]["queued_effects"], 0);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn worker_failover_continues_inflight_work_from_durable_state() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(ctx) = broker_runtime_test_context("worker-failover", 1) else {
        return;
    };
    let control = ctx.control_runtime("control");
    let worker = hosted_worker();

    let worker_a_runtime = ctx.worker_runtime("worker-a");
    let mut worker_a = worker.with_worker_runtime(worker_a_runtime.clone());
    wait_for_assigned_partitions(&worker_a_runtime, &mut worker_a, &[0]).await;

    let world = seed_fetch_notify_world(&ctx, &control, hosted_universe_id(&control));
    let world = wait_for_world_registration(&worker_a_runtime, &mut worker_a, &world).await;
    control
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/FetchNotifyEvent@1".into(),
            value: json!({
                "Start": {
                    "url": "https://example.com/failover.json",
                    "method": "GET"
                }
            }),
            submission_id: Some(format!("worker-failover-start-{}", uuid::Uuid::new_v4())),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();

    let pending_trace = wait_for_world_trace(&worker_a_runtime, &mut worker_a, &world, |trace| {
        trace["runtime_wait"]["pending_workflow_receipts"]
            .as_u64()
            .is_some_and(|count| count > 0)
            && trace["runtime_wait"]["queued_effects"] == 0
    })
    .await;
    assert_eq!(pending_trace["runtime_wait"]["queued_effects"], 0);
    assert_eq!(
        pending_trace["strict_quiescence"]["pending_workflow_receipts"],
        1
    );
    let intent_hash = wait_for_effect_intent_hash(&ctx, &world).await;
    let checkpoint = worker_a_runtime
        .checkpoint_partition(world.effective_partition)
        .unwrap();
    assert!(
        checkpoint
            .worlds
            .iter()
            .any(|entry| entry.universe_id == world.universe_id && entry.world_id == world.world_id)
    );

    drop(worker_a);
    drop(worker_a_runtime);

    let worker_b_runtime = ctx.worker_runtime("worker-b");
    let mut worker_b = worker.with_worker_runtime(worker_b_runtime.clone());
    wait_for_assigned_partitions(&worker_b_runtime, &mut worker_b, &[0]).await;
    wait_for_world_registration(&worker_b_runtime, &mut worker_b, &world).await;

    let receipt_payload = HttpRequestReceipt {
        status: 200,
        headers: Default::default(),
        body_ref: Some(
            HashRef::new("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
                .unwrap(),
        ),
        timings: RequestTimings {
            start_ns: 30,
            end_ns: 40,
        },
        adapter_id: "adapter.http.test".into(),
    };
    control
        .submit_receipt(
            world.universe_id,
            world.world_id,
            ReceiptIngress {
                intent_hash: intent_hash.to_vec(),
                effect_kind: "http.request".into(),
                adapter_id: "adapter.http.test".into(),
                status: ReceiptStatus::Ok,
                payload: CborPayload::inline(serde_cbor::to_vec(&receipt_payload).unwrap()),
                cost_cents: Some(0),
                signature: vec![9, 8, 7],
                correlation_id: Some(format!("worker-failover-receipt-{}", uuid::Uuid::new_v4())),
            },
        )
        .unwrap();

    let trace = wait_for_world_trace(&worker_b_runtime, &mut worker_b, &world, |trace| {
        trace["runtime_wait"]["pending_workflow_receipts"] == 0
            && trace["runtime_wait"]["queued_effects"] == 0
            && trace["strict_quiescence"]["inflight_workflow_intents"] == 0
            && trace["strict_quiescence"]["pending_workflow_receipts"] == 0
            && trace["strict_quiescence"]["queued_effects"] == 0
            && trace["totals"]["effects"]["receipts"]["ok"] == 1
    })
    .await;
    assert_eq!(trace["runtime_wait"]["pending_workflow_receipts"], 0);
    assert_eq!(trace["runtime_wait"]["queued_effects"], 0);
    assert_eq!(trace["strict_quiescence"]["inflight_workflow_intents"], 0);
    assert_eq!(trace["strict_quiescence"]["pending_workflow_receipts"], 0);
    assert_eq!(trace["strict_quiescence"]["queued_effects"], 0);
    assert_eq!(trace["totals"]["effects"]["receipts"]["ok"], 1);

    let (intent_count, _) = wait_for_effect_journal_records(&ctx, &world, 1, 0).await;
    assert_eq!(intent_count, 1);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn worker_retries_aborted_batch_from_ingress_after_restart() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(ctx) = broker_runtime_test_context("worker-abort-retry", 1) else {
        return;
    };
    let control = ctx.control_runtime("control");
    let worker_runtime = ctx.worker_runtime("worker");
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(worker_runtime.clone());
    let world = seed_counter_world(&control, hosted_universe_id(&control));
    let world = wait_for_world_registration(&worker_runtime, &mut supervisor, &world).await;

    control
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 1 } }),
            submission_id: Some(format!("worker-abort-{}", uuid::Uuid::new_v4())),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    worker_runtime.debug_fail_next_batch_commit().unwrap();

    let mut saw_failed_batch = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if supervisor.run_once().await.is_err() {
            saw_failed_batch = true;
            break;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    assert!(
        saw_failed_batch,
        "worker never hit the injected batch commit failure"
    );

    drop(supervisor);
    drop(worker_runtime);
    drop(control);

    let _recovered_control = ctx.control_runtime("control-recovered");
    let recovered_worker_runtime = ctx.worker_runtime("worker-recovered");
    let mut recovered_supervisor =
        hosted_worker().with_worker_runtime(recovered_worker_runtime.clone());

    wait_for_counter_state(
        &recovered_worker_runtime,
        &mut recovered_supervisor,
        &world,
        json!({ "$tag": "Counting" }),
        1,
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn worker_live_submission_after_create_checkpoint_survives_restart() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(ctx) = broker_runtime_test_context("worker-live-after-create-checkpoint", 1) else {
        return;
    };
    let universe_id = UniverseId::from(uuid::Uuid::new_v4());
    let control = ctx.control_runtime_in_universe("control", universe_id);
    let worker_runtime = ctx.worker_runtime_in_universe("worker", universe_id);
    let worker = aos_node_hosted::HostedWorker::new(aos_node_hosted::config::HostedWorkerConfig {
        checkpoint_on_create: true,
        ..worker_config()
    });
    let mut supervisor = worker.with_worker_runtime(worker_runtime.clone());

    let manifest_hash = upload_counter_manifest(&control, universe_id);
    let accepted = control
        .create_world(
            universe_id,
            CreateWorldRequest {
                world_id: None,
                universe_id,
                created_at_ns: 1,
                source: CreateWorldSource::Manifest { manifest_hash },
            },
        )
        .unwrap();
    let world =
        wait_for_world_summary(&control, &mut supervisor, universe_id, accepted.world_id).await;
    let checkpoint = wait_for_checkpoint(&ctx, &mut supervisor, &world).await;
    let baseline_height = checkpoint
        .worlds
        .iter()
        .find(|entry| entry.universe_id == world.universe_id && entry.world_id == world.world_id)
        .expect("checkpoint entry for freshly created world")
        .baseline
        .height;

    control
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 1 } }),
            submission_id: Some(format!(
                "worker-live-after-create-checkpoint-{}",
                uuid::Uuid::new_v4()
            )),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    wait_for_counter_state(
        &worker_runtime,
        &mut supervisor,
        &world,
        json!({ "$tag": "Counting" }),
        1,
    )
    .await;

    let mut reader = kafka_reader_for(&ctx, "live-after-create-checkpoint");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        reader
            .recover_partition_from_broker(world.effective_partition)
            .unwrap();
        if reader.next_world_seq(world.world_id) > baseline_height {
            break;
        }
        supervisor.run_once().await.unwrap();
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    let persisted_frame_ranges = reader
        .world_frames(world.world_id)
        .iter()
        .map(|frame| format!("{}..{}", frame.world_seq_start, frame.world_seq_end))
        .collect::<Vec<_>>();
    assert!(
        reader.next_world_seq(world.world_id) > baseline_height,
        "live post-checkpoint frame was never durably published; baseline_height={baseline_height} frames={persisted_frame_ranges:?}"
    );

    drop(supervisor);
    drop(worker_runtime);
    drop(control);

    let recovered_worker_runtime = ctx.worker_runtime_in_universe("worker-recovered", universe_id);
    let mut recovered_supervisor = worker.with_worker_runtime(recovered_worker_runtime.clone());
    wait_for_world_registration(&recovered_worker_runtime, &mut recovered_supervisor, &world).await;

    let recovered_state = recovered_worker_runtime
        .state_json(universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap()
        .unwrap_or_else(|| {
            panic!(
                "counter state should survive restart after live post-checkpoint submission; frames={persisted_frame_ranges:?}"
            )
        });
    assert_eq!(recovered_state["pc"], json!({ "$tag": "Counting" }));
    assert_eq!(recovered_state["remaining"], 1);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
#[ignore = "stress test for checkpoint publication under a large hot stream"]
async fn worker_periodic_checkpoint_under_large_hot_stream_preserves_world_sequence() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    const TICK_COUNT: usize = 50_000;

    let ctx = broker_runtime_test_context("worker-checkpoint-repro", 1).unwrap();
    let control = ctx.control_runtime("control");
    let worker_runtime = ctx.worker_runtime("worker");
    let worker = aos_node_hosted::HostedWorker::new(aos_node_hosted::config::HostedWorkerConfig {
        checkpoint_every_events: Some(1_000),
        ..worker_config()
    });
    let mut supervisor = worker.with_worker_runtime(worker_runtime.clone());
    let universe_id = hosted_universe_id(&worker_runtime);
    let manifest_hash = upload_counter_manifest(&worker_runtime, universe_id);
    let accepted = worker_runtime
        .create_world(
            universe_id,
            CreateWorldRequest {
                world_id: None,
                universe_id,
                created_at_ns: 1,
                source: CreateWorldSource::Manifest { manifest_hash },
            },
        )
        .unwrap();
    let world = wait_for_world_summary(
        &worker_runtime,
        &mut supervisor,
        universe_id,
        accepted.world_id,
    )
    .await;
    let send_world =
        wait_for_world_summary(&control, &mut supervisor, universe_id, accepted.world_id).await;

    control
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": TICK_COUNT } }),
            submission_id: Some(format!("worker-checkpoint-start-{}", uuid::Uuid::new_v4())),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();

    wait_for_counter_state(
        &worker_runtime,
        &mut supervisor,
        &world,
        json!({ "$tag": "Counting" }),
        TICK_COUNT as i64,
    )
    .await;

    let sender = control.clone();
    let sender_handle = tokio::task::spawn_blocking(move || {
        for index in 0..TICK_COUNT {
            sender
                .submit_event(aos_node_hosted::SubmitEventRequest {
                    universe_id: send_world.universe_id,
                    world_id: send_world.world_id,
                    schema: "demo/CounterEvent@1".into(),
                    value: json!({ "Tick": null }),
                    submission_id: Some(format!(
                        "worker-checkpoint-tick-{}-{index}",
                        uuid::Uuid::new_v4()
                    )),
                    expected_world_epoch: Some(send_world.world_epoch),
                })
                .unwrap();
        }
    });

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut sender_finished = false;
    while Instant::now() < deadline {
        if let Err(err) = supervisor.run_once().await {
            panic!("unexpected supervisor error during checkpoint stress test: {err:?}");
        }

        if !sender_finished && sender_handle.is_finished() {
            sender_finished = true;
        }
        if sender_finished
            && let Some(state) = worker_runtime
                .state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
                .unwrap()
            && state["pc"] == json!({ "$tag": "Done" })
            && state["remaining"] == 0
        {
            sender_handle.await.unwrap();
            return;
        }
    }

    sender_handle.await.unwrap();
    panic!("timed out waiting for counter completion under periodic checkpoints");
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn worker_restart_breaks_projection_continuity_and_mints_new_projection_token() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(ctx) = broker_runtime_test_context("worker-projection-reset", 1) else {
        return;
    };
    let worker_runtime = ctx.direct_worker_runtime("worker-initial", &[0]);
    let mut supervisor = hosted_worker().with_worker_runtime(worker_runtime.clone());
    let universe_id = hosted_universe_id(&worker_runtime);
    let world = create_counter_world(&worker_runtime, &mut supervisor, universe_id).await;
    let partition = world.effective_partition;

    let initial_entries =
        wait_for_projection_entries(&worker_runtime, &mut supervisor, partition, |entries| {
            latest_projection_token(entries, world.world_id).is_some()
        })
        .await;
    let initial_token = latest_projection_token(&initial_entries, world.world_id)
        .expect("initial projection token");

    drop(supervisor);
    drop(worker_runtime);

    let recovered_runtime = ctx.direct_worker_runtime("worker-recovered", &[0]);
    let mut recovered_supervisor = hosted_worker().with_worker_runtime(recovered_runtime.clone());
    let recovered_world =
        wait_for_world_registration(&recovered_runtime, &mut recovered_supervisor, &world).await;

    recovered_runtime
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id,
            world_id: recovered_world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 1 } }),
            submission_id: Some(format!("worker-projection-reset-{}", uuid::Uuid::new_v4())),
            expected_world_epoch: Some(recovered_world.world_epoch),
        })
        .unwrap();
    wait_for_counter_state(
        &recovered_runtime,
        &mut recovered_supervisor,
        &recovered_world,
        json!({ "$tag": "Counting" }),
        1,
    )
    .await;

    let recovered_entries = wait_for_projection_entries(
        &recovered_runtime,
        &mut recovered_supervisor,
        partition,
        |entries| latest_projection_token(entries, recovered_world.world_id).is_some(),
    )
    .await;
    let recovered_token = latest_projection_token(&recovered_entries, recovered_world.world_id)
        .expect("recovered projection token");
    assert_ne!(recovered_token, initial_token);
    assert!(
        recovered_entries.iter().any(|entry| {
            let record = decode_projection_entry(entry);
            matches!(
                record,
                ProjectionRecord {
                    key: ProjectionKey::Cell { world_id, .. },
                    value: Some(ProjectionValue::Cell(ref value)),
                } if world_id == recovered_world.world_id && value.projection_token == recovered_token
            )
        }),
        "recovered publish did not include any cell rows under the new projection token"
    );
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn materializer_cold_bootstrap_rebuilds_latest_projection_state_after_token_reset() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(ctx) = broker_runtime_test_context("materializer-projection-rebuild", 1) else {
        return;
    };
    let worker_runtime = ctx.direct_worker_runtime("worker-initial", &[0]);
    let mut supervisor = hosted_worker().with_worker_runtime(worker_runtime.clone());
    let universe_id = hosted_universe_id(&worker_runtime);
    let world = create_counter_world(&worker_runtime, &mut supervisor, universe_id).await;
    let partition = world.effective_partition;

    let initial_entries =
        wait_for_projection_entries(&worker_runtime, &mut supervisor, partition, |entries| {
            latest_projection_token(entries, world.world_id).is_some()
        })
        .await;
    let initial_token = latest_projection_token(&initial_entries, world.world_id)
        .expect("initial projection token");

    let stale_key = b"stale-retained".to_vec();
    let stale_state = serde_cbor::to_vec(&json!({ "status": "stale-retained" })).unwrap();
    let stale_record = ProjectionRecord {
        key: ProjectionKey::Cell {
            world_id: world.world_id,
            workflow: "demo/Injected@1".into(),
            key_hash: Hash::of_bytes(&stale_key).as_bytes().to_vec(),
        },
        value: Some(ProjectionValue::Cell(CellProjectionUpsert {
            projection_token: initial_token.clone(),
            record: CellStateProjectionRecord {
                journal_head: 1,
                workflow: "demo/Injected@1".into(),
                key_hash: Hash::of_bytes(&stale_key).as_bytes().to_vec(),
                key_bytes: stale_key.clone(),
                state_hash: Hash::of_bytes(&stale_state).to_hex(),
                size: stale_state.len() as u64,
                last_active_ns: 1,
            },
            state_payload: CborPayload::inline(stale_state),
        })),
    };
    let mut injected = HostedKafkaBackend::new(1, ctx.kafka_config.clone()).unwrap();
    injected
        .publish_projection_records(vec![stale_record])
        .unwrap();
    let injected_entries = injected
        .projection_entries(&ctx.kafka_config.projection_topic, partition)
        .to_vec();

    drop(supervisor);
    drop(worker_runtime);

    let recovered_runtime = ctx.direct_worker_runtime("worker-recovered", &[0]);
    let mut recovered_supervisor = hosted_worker().with_worker_runtime(recovered_runtime.clone());
    let recovered_world =
        wait_for_world_registration(&recovered_runtime, &mut recovered_supervisor, &world).await;

    recovered_runtime
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id,
            world_id: recovered_world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 1 } }),
            submission_id: Some(format!("materializer-rebuild-{}", uuid::Uuid::new_v4())),
            expected_world_epoch: Some(recovered_world.world_epoch),
        })
        .unwrap();
    wait_for_counter_state(
        &recovered_runtime,
        &mut recovered_supervisor,
        &recovered_world,
        json!({ "$tag": "Counting" }),
        1,
    )
    .await;
    let recovered_entries = wait_for_projection_entries(
        &recovered_runtime,
        &mut recovered_supervisor,
        partition,
        |entries| latest_projection_token(entries, recovered_world.world_id).is_some(),
    )
    .await;
    let recovered_token = latest_projection_token(&recovered_entries, recovered_world.world_id)
        .expect("recovered projection token");
    assert_ne!(recovered_token, initial_token);

    let materializer_root = temp_state_root("materializer-cold-bootstrap");
    let materializer_paths = LocalStatePaths::new(&materializer_root);
    let mut retained_entries = initial_entries
        .iter()
        .chain(injected_entries.iter())
        .chain(recovered_entries.iter())
        .map(|entry| (entry.offset, decode_projection_entry(entry)))
        .collect::<Vec<_>>();
    retained_entries.sort_by_key(|(offset, _)| *offset);

    let mut materializer =
        Materializer::<aos_node_hosted::blobstore::HostedCas>::from_config(MaterializerConfig {
            projection_topic: ctx.kafka_config.projection_topic.clone(),
            ..MaterializerConfig::from_paths(&materializer_paths, "aos-journal")
        })
        .unwrap();
    materializer
        .bootstrap_projection_partition(partition, &retained_entries)
        .unwrap();

    let sqlite = MaterializerSqliteStore::from_paths(&materializer_paths).unwrap();
    assert_eq!(
        sqlite
            .load_projection_token(recovered_world.world_id)
            .unwrap(),
        Some(recovered_token)
    );
    assert!(
        sqlite
            .load_cell_projection(
                universe_id,
                recovered_world.world_id,
                "demo/CounterSM@1",
                b""
            )
            .unwrap()
            .is_some()
    );
    assert!(
        sqlite
            .load_cell_projection(
                universe_id,
                recovered_world.world_id,
                "demo/Injected@1",
                &stale_key
            )
            .unwrap()
            .is_none()
    );
    assert!(
        sqlite
            .load_source_offset(&ctx.kafka_config.projection_topic, partition)
            .unwrap()
            .is_some()
    );
}
