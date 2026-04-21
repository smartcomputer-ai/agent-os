#[path = "../tests/common/mod.rs"]
mod common;

use std::sync::Arc;
use std::time::Duration;

use aos_node::control::{CommandSubmitBody, CreateWorldBody, SubmitEventBody};
use aos_node::{CreateWorldSource, WorldId};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use serial_test::serial;
use tower::util::ServiceExt;

use common::*;

fn control_app(runtime: &aos_node::worker::HostedWorkerRuntime) -> axum::Router {
    let facade = Arc::new(
        aos_node::test_support::control_facade_from_worker_runtime(runtime.clone())
            .expect("open node control"),
    );
    aos_node::control::router(facade)
}

async fn response_json_with_status<T: serde::de::DeserializeOwned>(
    response: axum::response::Response,
    expected_status: StatusCode,
) -> T {
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    assert_eq!(
        status,
        expected_status,
        "body: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("decode json body")
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_broker_mode_submits_via_standalone_services() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(ctx) = broker_runtime_test_context("http-control-standalone", 1) else {
        return;
    };
    let worker_runtime = ctx.worker_runtime("worker");
    let world_id = WorldId::from(uuid::Uuid::new_v4());
    worker_runtime
        .configure_owned_worlds([world_id])
        .expect("configure worker world ownership");
    let control_runtime = ctx.control_runtime("control");
    let app = control_app(&control_runtime);
    let worker = hosted_worker();
    let mut supervisor = worker
        .with_worker_runtime(worker_runtime.clone())
        .spawn()
        .unwrap();
    let universe_id = hosted_universe_id(&worker_runtime);
    let manifest_hash =
        upload_authored_manifest(&worker_runtime, universe_id, &counter_world_root());

    let created: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::post("/v1/worlds?wait_for_flush=true")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&CreateWorldBody {
                            world_id: Some(world_id),
                            universe_id,
                            created_at_ns: 1,
                            source: CreateWorldSource::Manifest { manifest_hash },
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
        StatusCode::CREATED,
    )
    .await;
    let created_world_id = created["world_id"]
        .as_str()
        .unwrap()
        .parse::<WorldId>()
        .unwrap();
    assert_eq!(created_world_id, world_id);

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        wait_for_worker(&mut supervisor).await;
        if worker_runtime.get_world(universe_id, world_id).is_ok() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "world was not created in time"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let event: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::post(format!("/v1/worlds/{world_id}/events?wait_for_flush=true"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&SubmitEventBody {
                            schema: "demo/CounterEvent@1".into(),
                            value: Some(json!({ "Start": { "target": 1 } })),
                            value_json: None,
                            value_b64: None,
                            key_b64: None,
                            correlation_id: None,
                            submission_id: Some(format!("evt-{}", uuid::Uuid::new_v4())),
                            expected_world_epoch: Some(1),
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(event["world_epoch"], 1);

    let command: Value = response_json(
        app.clone()
            .oneshot(
                Request::post(format!("/v1/worlds/{world_id}/governance/shadow"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&CommandSubmitBody {
                            command_id: Some("cmd-broker-shadow".into()),
                            actor: None,
                            params: aos_effect_types::GovShadowParams { proposal_id: 999 },
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(command["command_id"], "cmd-broker-shadow");

    let _ = supervisor.wait_for_progress(Duration::from_millis(5)).await;
    let _ = supervisor.wait_for_progress(Duration::from_millis(5)).await;

    let record: Value = response_json(
        app.clone()
            .oneshot(
                Request::get(format!("/v1/worlds/{world_id}/commands/cmd-broker-shadow"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(record["command"], "gov-shadow");

    let trace_summary: Value = response_json(
        app.oneshot(
            Request::get(format!(
                "/v1/worlds/{world_id}/trace-summary?recent_limit=5"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert!(trace_summary.get("totals").is_some());
}
