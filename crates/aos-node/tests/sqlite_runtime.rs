mod common;

use aos_node::control::AcceptWaitQuery;
use aos_node::{HostedWorkerRuntime, SubmitEventRequest};
use serde_json::json;
use serial_test::serial;
use tempfile::tempdir;

use common::{create_counter_world, hosted_universe_id};

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn sqlite_runtime_reopens_world_and_replays_journal_tail() {
    let state_root = tempdir().unwrap();
    let runtime = HostedWorkerRuntime::new_sqlite_with_state_root(state_root.path()).unwrap();
    let universe_id = hosted_universe_id(&runtime);
    let world = create_counter_world(&runtime, universe_id);

    runtime
        .submit_event_with_wait(
            SubmitEventRequest {
                universe_id: world.universe_id,
                world_id: world.world_id,
                schema: "demo/CounterEvent@1".into(),
                value: json!({ "Start": { "target": 2 } }),
                submission_id: Some("sqlite-start".into()),
                expected_world_epoch: Some(world.world_epoch),
            },
            AcceptWaitQuery {
                wait_for_flush: true,
                wait_timeout_ms: Some(5_000),
            },
        )
        .unwrap();

    let state_before_reopen = runtime
        .state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap()
        .unwrap();
    let frames_before_reopen = runtime.world_frames(world.world_id).unwrap();
    assert!(frames_before_reopen.len() >= 2);

    drop(runtime);

    let reopened = HostedWorkerRuntime::new_sqlite_with_state_root(state_root.path()).unwrap();
    let state_after_reopen = reopened
        .state_json(world.universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap()
        .unwrap();
    assert_eq!(state_after_reopen, state_before_reopen);

    let frames = reopened.world_frames(world.world_id).unwrap();
    assert!(frames.len() >= 2);
}
