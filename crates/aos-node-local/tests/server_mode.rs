mod common;
mod support;

use std::time::Duration;

use aos_cbor::to_canonical_cbor;
use aos_node::api::SubmitEventBody;
use aos_node::{
    CborPayload, CreateWorldRequest, CreateWorldSource, DomainEventIngress, LocalControl,
};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::util::ServiceExt;

use common::world;

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

#[test]
fn http_control_routes_events_into_durable_local_world_frames()
-> Result<(), Box<dyn std::error::Error>> {
    let (_temp, paths) = common::temp_state_root();
    let control = LocalControl::open(paths.root())?;
    support::create_simple_world(&control, &paths, world(), 123)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let app = aos_node::api::http::router(control.clone());
        let submit: Value = response_json_with_status(
            app.clone()
                .oneshot(
                    Request::post(format!("/v1/worlds/{}/events", world()))
                        .header("content-type", "application/json")
                        .body(Body::from(serde_json::to_vec(&SubmitEventBody {
                            schema: support::fixtures::START_SCHEMA.into(),
                            value: Some(support::fixtures::start_event("http-queued-1")),
                            value_json: None,
                            value_b64: None,
                            key_b64: None,
                            correlation_id: Some("http-queued-1".into()),
                            submission_id: None,
                            expected_world_epoch: None,
                        })?))
                        .unwrap(),
                )
                .await
                .unwrap(),
            StatusCode::ACCEPTED,
        )
        .await;
        assert_eq!(
            submit["inbox_seq"],
            serde_json::json!([0, 0, 0, 0, 0, 0, 0, 0])
        );

        let runtime: aos_node::WorldRuntimeInfo = response_json_with_status(
            app.clone()
                .oneshot(
                    Request::get(format!("/v1/worlds/{}/runtime", world()))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert!(!runtime.has_pending_inbox);
        assert!(!runtime.has_pending_effects);

        let state: Value = response_json_with_status(
            app.oneshot(
                Request::get(format!("/v1/worlds/{}/state/com.acme%2FSimple@1", world()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(state["state_b64"], "qg==");
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;
    Ok(())
}

#[test]
fn reopened_local_control_restores_durable_state_without_ephemeral_server_queue()
-> Result<(), Box<dyn std::error::Error>> {
    let (_temp, paths) = common::temp_state_root();
    let control = LocalControl::open(paths.root())?;
    support::create_simple_world(&control, &paths, world(), 123)?;

    let seq = control.enqueue_event(
        world(),
        DomainEventIngress {
            schema: support::fixtures::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&support::fixtures::start_event(
                "queued-restart",
            ))?),
            key: None,
            correlation_id: Some("queued-restart".into()),
        },
    )?;
    assert_eq!(seq.to_string(), "0000000000000000");

    drop(control);

    let reopened = LocalControl::open(paths.root())?;
    let runtime = reopened.runtime(world())?;
    assert!(!runtime.has_pending_inbox);
    assert!(!runtime.has_pending_effects);

    let state = reopened.state_get(world(), "com.acme/Simple@1", None, None)?;
    assert_eq!(state.state_b64.as_deref(), Some("qg=="));
    Ok(())
}

#[test]
fn server_mode_processes_timer_continuations_without_follow_up_requests()
-> Result<(), Box<dyn std::error::Error>> {
    let (_temp, paths) = common::temp_state_root();
    let control = LocalControl::open(paths.root())?;
    let manifest_hash = support::install_timer_manifest(&paths)?;
    control.create_world(CreateWorldRequest {
        world_id: Some(world()),
        universe_id: aos_node::UniverseId::nil(),
        created_at_ns: 123,
        source: CreateWorldSource::Manifest { manifest_hash },
    })?;

    control.enqueue_event(
        world(),
        DomainEventIngress {
            schema: support::fixtures::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&support::fixtures::start_event(
                "timer-server",
            ))?),
            key: None,
            correlation_id: Some("timer-server".into()),
        },
    )?;

    std::thread::sleep(Duration::from_millis(50));

    let runtime = control.runtime(world())?;
    assert!(!runtime.has_pending_inbox);
    assert!(!runtime.has_pending_effects);
    assert_eq!(runtime.next_timer_due_at_ns, None);
    Ok(())
}

#[test]
fn http_checkpoint_route_runs_scheduler_maintenance_and_clears_retained_tail()
-> Result<(), Box<dyn std::error::Error>> {
    let (_temp, paths) = common::temp_state_root();
    let control = LocalControl::open(paths.root())?;
    support::create_simple_world(&control, &paths, world(), 123)?;
    control.enqueue_event(
        world(),
        DomainEventIngress {
            schema: support::fixtures::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&support::fixtures::start_event(
                "checkpoint-http",
            ))?),
            key: None,
            correlation_id: Some("checkpoint-http".into()),
        },
    )?;
    assert!(control.runtime(world())?.has_pending_maintenance);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let app = aos_node::api::http::router(control.clone());
        let checkpointed: Value = response_json_with_status(
            app.oneshot(
                Request::post(format!("/v1/worlds/{}/checkpoint", world()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            checkpointed["runtime"]["has_pending_maintenance"],
            serde_json::json!(false)
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;

    let head = control.journal_head(world())?;
    assert_eq!(head.retained_from, head.journal_head);
    Ok(())
}
