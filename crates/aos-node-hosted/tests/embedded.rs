mod common;

use std::time::Duration;

use aos_cbor::Hash;
use aos_node::{CreateWorldRequest, CreateWorldSource};
use aos_node_hosted::config::HostedWorkerConfig;
use serde_json::json;
use serial_test::serial;
use tempfile::tempdir;

use common::*;

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn embedded_duplicate_submission_id_is_rejected_without_double_apply() {
    let runtime = embedded_runtime(1);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone());
    let world = create_counter_world(&runtime, &mut supervisor, hosted_universe_id(&runtime)).await;

    runtime
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 2 } }),
            submission_id: Some("dup-start".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    supervisor.run_once().await.unwrap();

    runtime
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Tick": null }),
            submission_id: Some("dup-tick".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    supervisor.run_once().await.unwrap();

    runtime
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Tick": null }),
            submission_id: Some("dup-tick".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    supervisor.run_once().await.unwrap();

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
    let mut supervisor = worker.with_worker_runtime(runtime.clone());
    let world = create_counter_world(&runtime, &mut supervisor, hosted_universe_id(&runtime)).await;

    let initial_state = runtime
        .state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap();

    runtime
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 1 } }),
            submission_id: Some("rollback-start".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    runtime.debug_fail_next_batch_commit().unwrap();

    assert!(supervisor.run_once().await.is_err());

    let state_after_failure = runtime
        .state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap();
    assert_eq!(state_after_failure, initial_state);

    supervisor.run_once().await.unwrap();

    let retried_state = runtime
        .state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap()
        .unwrap();
    assert_eq!(retried_state["pc"], json!({ "$tag": "Counting" }));
    assert_eq!(retried_state["remaining"], 1);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn embedded_failed_create_batch_commit_retries_without_leaving_registered_ghost_state() {
    let runtime = embedded_runtime(1);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone());
    let universe_id = hosted_universe_id(&runtime);
    let manifest_hash = upload_counter_manifest(&runtime, universe_id);

    let accepted = runtime
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
    runtime.debug_fail_next_batch_commit().unwrap();

    assert!(supervisor.run_once().await.is_err());
    assert!(matches!(
        runtime.get_world(universe_id, accepted.world_id),
        Err(aos_node_hosted::WorkerError::UnknownWorld { .. })
    ));

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        supervisor.run_once().await.unwrap();
        if let Ok(world) = runtime.get_world(universe_id, accepted.world_id) {
            assert_eq!(world.world_id, accepted.world_id);
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let world = runtime.get_world(universe_id, accepted.world_id).unwrap();
    assert_eq!(world.world_id, accepted.world_id);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn embedded_worker_checkpoints_after_configured_event_count() {
    let runtime = embedded_runtime(1);
    let worker = aos_node_hosted::HostedWorker::new(HostedWorkerConfig {
        worker_id: "checkpoint-worker".into(),
        partition_count: 1,
        supervisor_poll_interval: Duration::from_millis(10),
        checkpoint_interval: Duration::from_secs(3600),
        checkpoint_every_events: Some(2),
        checkpoint_on_create: false,
    });
    let mut supervisor = worker.with_worker_runtime(runtime.clone());
    let world = create_counter_world(&runtime, &mut supervisor, hosted_universe_id(&runtime)).await;

    runtime
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 2 } }),
            submission_id: Some("checkpoint-start".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    let first = supervisor.run_once().await.unwrap();
    assert_eq!(first.frames_appended, 1);
    assert_eq!(first.checkpoints_published, 0);

    runtime
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Tick": null }),
            submission_id: Some("checkpoint-tick".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    let second = supervisor.run_once().await.unwrap();
    assert_eq!(second.frames_appended, 1);
    assert_eq!(second.checkpoints_published, 1);

    let idle = supervisor.run_once().await.unwrap();
    assert_eq!(idle.frames_appended, 0);
    assert_eq!(idle.checkpoints_published, 0);

    let state = runtime
        .state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap()
        .unwrap();
    assert_eq!(state["pc"], json!({ "$tag": "Counting" }));
    assert_eq!(state["remaining"], 1);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn embedded_worker_executes_timer_effects_inline() {
    let runtime = embedded_runtime(1);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone());
    let universe_id = hosted_universe_id(&runtime);
    let manifest_hash = upload_timer_manifest(&runtime, universe_id);
    seed_timer_builtins(&runtime, universe_id);
    let world =
        create_world_from_manifest(&runtime, &mut supervisor, universe_id, manifest_hash).await;

    runtime
        .submit_event(aos_node_hosted::SubmitEventRequest {
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

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        supervisor.run_once().await.unwrap();
        if let Some(state) = runtime
            .state_json(world.universe_id, world.world_id, "demo/TimerSM@1", None)
            .unwrap()
            && state["pc"] == json!({ "$tag": "Done" })
        {
            assert_eq!(state["fired_key"], json!("hosted-inline-timer"));
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let state = runtime
        .state_json(world.universe_id, world.world_id, "demo/TimerSM@1", None)
        .unwrap()
        .unwrap();
    assert_eq!(state["pc"], json!({ "$tag": "Done" }));
    assert_eq!(state["fired_key"], json!("hosted-inline-timer"));
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn embedded_checkpoint_keeps_hot_world_running_after_compaction() {
    let runtime = embedded_runtime(1);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone());

    let world = create_counter_world(&runtime, &mut supervisor, hosted_universe_id(&runtime)).await;

    runtime
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 1 } }),
            submission_id: Some("embedded-checkpoint-start".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    supervisor.run_once().await.unwrap();

    let checkpoint = runtime.checkpoint_partition(0).unwrap();
    assert_eq!(checkpoint.partition, 0);
    assert_eq!(checkpoint.worlds.len(), 1);

    runtime
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id: world.universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Tick": null }),
            submission_id: Some("embedded-checkpoint-tick".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    supervisor.run_once().await.unwrap();

    let state = runtime
        .state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap()
        .unwrap();
    assert_eq!(state["pc"], json!({ "$tag": "Done" }));
    assert_eq!(state["remaining"], 0);
}

#[test]
#[serial]
fn embedded_runtimes_can_share_same_state_root_cas() {
    let root = tempdir().unwrap();
    let runtime_a =
        aos_node_hosted::worker::HostedWorkerRuntime::new_embedded_with_state_root(1, root.path())
            .unwrap();
    let runtime_b =
        aos_node_hosted::worker::HostedWorkerRuntime::new_embedded_with_state_root(1, root.path())
            .unwrap();

    let universe_id = hosted_universe_id(&runtime_a);
    let payload = br#"shared-root-cas-payload"#;
    let hash = runtime_a.put_blob(universe_id, payload).unwrap();

    assert_eq!(hash, Hash::of_bytes(payload));
    assert_eq!(runtime_b.get_blob(universe_id, hash).unwrap(), payload);
}
