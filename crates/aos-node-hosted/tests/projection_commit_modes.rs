mod common;

use std::time::{Duration, Instant};

use aos_node_hosted::SubmitEventRequest;
use aos_node_hosted::config::{HostedWorkerConfig, ProjectionCommitMode};
use aos_node_hosted::kafka::{ProjectionKey, ProjectionValue, WorldMetaProjection};
use serde_json::json;
use serial_test::serial;

use common::{create_counter_world, embedded_runtime, hosted_universe_id};

fn latest_world_meta_head(
    runtime: &aos_node_hosted::worker::HostedWorkerRuntime,
    partition: u32,
    world_id: aos_node::WorldId,
) -> Option<u64> {
    runtime
        .projection_entries(partition)
        .expect("projection entries")
        .into_iter()
        .filter_map(|entry| {
            let key: ProjectionKey = serde_cbor::from_slice(&entry.key).ok()?;
            let value = entry.value.as_ref()?;
            let value: ProjectionValue = serde_cbor::from_slice(value).ok()?;
            match (key, value) {
                (
                    ProjectionKey::WorldMeta {
                        world_id: key_world_id,
                    },
                    ProjectionValue::WorldMeta(WorldMetaProjection { journal_head, .. }),
                ) if key_world_id == world_id => Some(journal_head),
                _ => None,
            }
        })
        .max()
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn background_projection_mode_publishes_world_updates() {
    let runtime = embedded_runtime(1);
    runtime
        .set_projection_commit_mode(ProjectionCommitMode::Background)
        .unwrap();
    let universe_id = hosted_universe_id(&runtime);
    let world = create_counter_world(&runtime, universe_id);
    let initial_head =
        latest_world_meta_head(&runtime, world.effective_partition, world.world_id).unwrap();

    let worker = aos_node_hosted::HostedWorker::new(HostedWorkerConfig {
        projection_commit_mode: ProjectionCommitMode::Background,
        ..HostedWorkerConfig::default()
    });
    let mut supervisor = worker.with_worker_runtime(runtime.clone()).spawn().unwrap();

    runtime
        .submit_event(SubmitEventRequest {
            universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 1 } }),
            submission_id: Some("projection-background".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let _ = supervisor
            .observe_interval(Duration::from_millis(10))
            .await
            .unwrap();
        let Some(latest_head) =
            latest_world_meta_head(&runtime, world.effective_partition, world.world_id)
        else {
            continue;
        };
        if latest_head > initial_head {
            supervisor.shutdown().await.unwrap();
            return;
        }
    }

    supervisor.shutdown().await.unwrap();
    panic!("projection world meta head did not advance beyond {initial_head} before timeout");
}
