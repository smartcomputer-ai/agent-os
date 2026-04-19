mod common;

use std::time::{Duration, Instant};

use aos_cbor::Hash;
use serde_json::json;
use serial_test::serial;
use tempfile::tempdir;

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
