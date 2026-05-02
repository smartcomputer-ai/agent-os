mod common;

use std::sync::Arc;
use std::time::Duration;

use aos_kernel::SecretResolver;
use aos_node::control::{CommandSubmitBody, CreateWorldBody, SubmitEventBody};
use aos_node::{UniverseId, WorldId};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use base64::Engine;
use futures::StreamExt;
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

async fn first_body_chunk(response: axum::response::Response) -> String {
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let chunk = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("SSE chunk")
        .expect("SSE stream item")
        .expect("SSE bytes");
    String::from_utf8(chunk.to_vec()).expect("SSE utf8")
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_serves_hot_reads_without_materialization() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone()).spawn().unwrap();
    let universe_id = hosted_universe_id(&runtime);
    let world = create_counter_world(&runtime, universe_id);

    runtime
        .submit_event(aos_node::SubmitEventRequest {
            universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 1 } }),
            submission_id: Some("reads-start".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    wait_for_worker(&mut supervisor).await;

    let manifest: Value = response_json(
        app.oneshot(
            Request::get(format!("/v1/worlds/{}/manifest", world.world_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert!(manifest["journal_head"].as_u64().unwrap() >= 1);
    assert!(manifest["manifest_hash"].as_str().is_some());
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_journal_wait_returns_existing_durable_entries() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let universe_id = hosted_universe_id(&runtime);
    let world = create_counter_world(&runtime, universe_id);

    let response: Value = response_json(
        app.oneshot(
            Request::get(format!(
                "/v1/worlds/{}/journal/wait?from=0&timeout_ms=1&limit=10",
                world.world_id
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;

    assert_eq!(
        response["world_id"].as_str().unwrap(),
        world.world_id.to_string()
    );
    assert!(!response["timed_out"].as_bool().unwrap());
    assert!(!response["gap"].as_bool().unwrap());
    assert!(response["next_from"].as_u64().unwrap() > 0);
    assert!(
        response["entries"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty())
    );
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_journal_wait_wakes_after_durable_flush() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone()).spawn().unwrap();
    let universe_id = hosted_universe_id(&runtime);
    let world = create_counter_world(&runtime, universe_id);
    let head = runtime
        .journal_head(universe_id, world.world_id)
        .unwrap()
        .journal_head;

    let wait_request = Request::get(format!(
        "/v1/worlds/{}/journal/wait?from={head}&timeout_ms=5000&limit=20",
        world.world_id
    ))
    .body(Body::empty())
    .unwrap();
    let wait_future = app.clone().oneshot(wait_request);
    let submit_future = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        runtime
            .submit_event(aos_node::SubmitEventRequest {
                universe_id,
                world_id: world.world_id,
                schema: "demo/CounterEvent@1".into(),
                value: json!({ "Start": { "target": 1 } }),
                submission_id: Some("wait-wakeup-start".into()),
                expected_world_epoch: Some(world.world_epoch),
            })
            .unwrap();
        wait_for_worker(&mut supervisor).await;
    };
    let (wait_response, ()) = tokio::join!(wait_future, submit_future);
    let response: Value = response_json(wait_response.unwrap()).await;

    assert!(!response["timed_out"].as_bool().unwrap());
    assert!(response["next_from"].as_u64().unwrap() > head);
    assert!(
        response["entries"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty())
    );
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_journal_stream_emits_sse_records_and_head_id() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let universe_id = hosted_universe_id(&runtime);
    let world = create_counter_world(&runtime, universe_id);

    let response = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/v1/worlds/{}/journal/stream?from=0&limit=1",
                world.world_id
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );
    let chunk = first_body_chunk(response).await;

    assert!(chunk.contains("event: journal_record"));
    assert!(chunk.contains("id: 0"));
    assert!(chunk.contains("event: world_head"));
    assert!(chunk.contains("\"next_from\":1"));

    let filtered_response = app
        .oneshot(
            Request::get(format!(
                "/v1/worlds/{}/journal/stream?from=0&limit=1&kind=no_such_kind&kind=also_missing",
                world.world_id
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    let filtered_chunk = first_body_chunk(filtered_response).await;
    assert!(!filtered_chunk.contains("event: journal_record"));
    assert!(filtered_chunk.contains("event: world_head"));
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_secret_binding_routes_roundtrip_and_resolve() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let universe_id = hosted_universe_id(&runtime);
    let binding_id = "llm/api";
    let encoded_binding_id = "llm%2Fapi";
    let plaintext = b"hosted-test-key";
    let expected_digest = aos_cbor::Hash::of_bytes(plaintext).to_hex();

    let binding: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::put(format!("/v1/secrets/bindings/{encoded_binding_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "source_kind": "node_secret_store",
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(binding["binding_id"], binding_id);
    assert_eq!(binding["source_kind"], "node_secret_store");

    let version: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::post(format!("/v1/secrets/bindings/{encoded_binding_id}/versions"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "plaintext_b64": base64::engine::general_purpose::STANDARD.encode(plaintext),
                        "expected_digest": expected_digest,
                        "actor": "test",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
            )
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(version["binding_id"], binding_id);
    let version_number = version["version"].as_u64().expect("secret version number");
    assert!(version_number >= 1);
    assert_eq!(version["digest"], expected_digest);
    assert!(version.get("ciphertext").is_some());

    let bindings: Vec<Value> = response_json_with_status(
        app.clone()
            .oneshot(
                Request::get("/v1/secrets/bindings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let listed = bindings
        .iter()
        .find(|entry| entry["binding_id"] == binding_id)
        .expect("listed binding");
    assert_eq!(listed["latest_version"], version_number);

    let stored_version: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::get(format!(
                    "/v1/secrets/bindings/{encoded_binding_id}/versions/{version_number}"
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(stored_version["digest"], expected_digest);

    let resolver = runtime.vault().unwrap().resolver_for_universe(universe_id);
    let resolved = resolver
        .resolve(binding_id, version_number, None)
        .expect("resolve hosted secret");
    assert_eq!(resolved.value, plaintext);

    let deleted: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::delete(format!("/v1/secrets/bindings/{encoded_binding_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(deleted["binding_id"], binding_id);

    let response = app
        .oneshot(
            Request::get(format!("/v1/secrets/bindings/{encoded_binding_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_serves_hot_state_manifest_journal_and_runtime() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let universe_id = hosted_universe_id(&runtime);
    seed_timer_builtins(&runtime, universe_id);
    let manifest_hash = upload_timer_manifest(&runtime, universe_id);
    let world = create_world_from_manifest(&runtime, universe_id, manifest_hash);

    let manifest: Value = response_json(
        app.clone()
            .oneshot(
                Request::get(format!("/v1/worlds/{}/manifest", world.world_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(manifest["journal_head"].as_u64().unwrap() >= 1);
    assert!(manifest["manifest_hash"].as_str().is_some());

    let defs: Value = response_json(
        app.clone()
            .oneshot(
                Request::get(format!("/v1/worlds/{}/defs", world.world_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(defs["defs"].as_array().is_some_and(|defs| !defs.is_empty()));

    let journal_head: Value = response_json(
        app.clone()
            .oneshot(
                Request::get(format!("/v1/worlds/{}/journal/head", world.world_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        journal_head["journal_head"].as_u64().unwrap()
            <= manifest["journal_head"].as_u64().unwrap()
    );

    let journal: Value = response_json(
        app.clone()
            .oneshot(
                Request::get(format!("/v1/worlds/{}/journal", world.world_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(journal["entries"].as_array().is_some());

    let raw = app
        .clone()
        .oneshot(
            Request::get(format!("/v1/worlds/{}/journal", world.world_id))
                .header("accept", "application/cbor")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(raw.status(), StatusCode::OK);
    assert_eq!(
        raw.headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/cbor")
    );

    let runtime_info: Value = response_json(
        app.clone()
            .oneshot(
                Request::get(format!("/v1/worlds/{}/runtime", world.world_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(runtime_info["world_id"], world.world_id.to_string());

    let workspace: Value = response_json(
        app.oneshot(
            Request::get(format!(
                "/v1/worlds/{}/workspace/resolve?workspace=workflow",
                world.world_id
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(workspace["workspace"], "workflow");
    assert!(workspace["exists"].is_boolean());
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_supports_workspace_and_cas_endpoints() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);

    let blob_put: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::post("/v1/cas/blobs")
                    .body(Body::from("hello world".as_bytes().to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap(),
        StatusCode::CREATED,
    )
    .await;
    let blob_hash = blob_put["hash"].as_str().unwrap().to_owned();

    let root: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::post("/v1/workspace/roots")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
        StatusCode::CREATED,
    )
    .await;
    let root_hash = root["root_hash"].as_str().unwrap().to_owned();

    let applied: Value = response_json(
        app.clone()
            .oneshot(
                Request::post(format!("/v1/workspace/roots/{root_hash}/apply"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "operations": [
                                {
                                    "op": "write_ref",
                                    "path": "docs/readme.txt",
                                    "blob_hash": blob_hash,
                                }
                            ]
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let new_root_hash = applied["new_root_hash"].as_str().unwrap().to_owned();

    let entry: Value = response_json(
        app.clone()
            .oneshot(
                Request::get(format!(
                    "/v1/workspace/roots/{new_root_hash}/entry?path=docs/readme.txt"
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(entry["kind"], "file");

    let bytes_response = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/v1/workspace/roots/{new_root_hash}/bytes?path=docs/readme.txt"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bytes_response.status(), StatusCode::OK);
    let bytes = to_bytes(bytes_response.into_body(), usize::MAX)
        .await
        .expect("read workspace bytes");
    assert_eq!(bytes.as_ref(), b"hello world");

    let diff: Value = response_json(
        app.clone()
            .oneshot(
                Request::post("/v1/workspace/diffs")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "root_a": root_hash,
                            "root_b": new_root_hash,
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(!diff["changes"].as_array().unwrap().is_empty());

    let head = app
        .oneshot(
            Request::head(format!("/v1/cas/blobs/{blob_hash}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_cas_endpoints_are_scoped_by_universe() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let universe_id = UniverseId::from(uuid::Uuid::new_v4());

    let blob_put: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::post(format!("/v1/cas/blobs?universe_id={universe_id}"))
                    .body(Body::from("domain isolated".as_bytes().to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap(),
        StatusCode::CREATED,
    )
    .await;
    let blob_hash = blob_put["hash"].as_str().unwrap().to_owned();

    let shared_head = app
        .clone()
        .oneshot(
            Request::head(format!("/v1/cas/blobs/{blob_hash}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(shared_head.status(), StatusCode::NOT_FOUND);

    let scoped_head = app
        .clone()
        .oneshot(
            Request::head(format!(
                "/v1/cas/blobs/{blob_hash}?universe_id={universe_id}"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(scoped_head.status(), StatusCode::OK);

    let scoped_get = app
        .oneshot(
            Request::get(format!(
                "/v1/cas/blobs/{blob_hash}?universe_id={universe_id}"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(scoped_get.status(), StatusCode::OK);
    let body = to_bytes(scoped_get.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), b"domain isolated");
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_accepts_cas_uploads_larger_than_axum_default_limit() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let payload = vec![b'x'; 3 * 1024 * 1024];

    let blob_put: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::post("/v1/cas/blobs")
                    .body(Body::from(payload.clone()))
                    .unwrap(),
            )
            .await
            .unwrap(),
        StatusCode::CREATED,
    )
    .await;
    let blob_hash = blob_put["hash"].as_str().unwrap().to_owned();

    let fetched = app
        .oneshot(
            Request::get(format!("/v1/cas/blobs/{blob_hash}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fetched.status(), StatusCode::OK);

    let body = to_bytes(fetched.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.len(), payload.len());
    assert_eq!(body.as_ref(), payload.as_slice());
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_workspace_endpoints_are_scoped_by_universe() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let universe_id = UniverseId::from(uuid::Uuid::new_v4());

    let created: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::post(format!("/v1/workspace/roots?universe_id={universe_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
        StatusCode::CREATED,
    )
    .await;
    let root_hash = created["root_hash"].as_str().unwrap().to_owned();

    let applied: Value = response_json(
        app.clone()
            .oneshot(
                Request::post(format!(
                    "/v1/workspace/roots/{root_hash}/apply?universe_id={universe_id}"
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "operations": [
                            {
                                "op": "write_bytes",
                                "path": "hello.txt",
                                "bytes_b64": base64::engine::general_purpose::STANDARD.encode("hi"),
                            }
                        ]
                    }))
                    .unwrap(),
                ))
                .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let new_root = applied["new_root_hash"].as_str().unwrap();

    let scoped_bytes = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/v1/workspace/roots/{new_root}/bytes?path=hello.txt&universe_id={universe_id}"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(scoped_bytes.status(), StatusCode::OK);
    let body = to_bytes(scoped_bytes.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), b"hi");

    let shared_bytes = app
        .oneshot(
            Request::get(format!(
                "/v1/workspace/roots/{new_root}/bytes?path=hello.txt"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(shared_bytes.status(), StatusCode::OK);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_serves_nonshared_universe_worlds() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone()).spawn().unwrap();
    let universe_id = UniverseId::from(uuid::Uuid::new_v4());
    let manifest_hash =
        upload_manifest_for_world_root_in_domain(&runtime, universe_id, &counter_world_root());

    let accepted: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::post("/v1/worlds")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&CreateWorldBody {
                            world_id: None,
                            universe_id,
                            created_at_ns: 1,
                            source: aos_node::CreateWorldSource::Manifest {
                                manifest_hash: manifest_hash.clone(),
                            },
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
    let world_id = accepted["world_id"]
        .as_str()
        .unwrap()
        .parse::<WorldId>()
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let world = loop {
        wait_for_worker(&mut supervisor).await;
        if let Ok(world) = runtime.get_world(universe_id, world_id) {
            break world;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "world did not materialize in time"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert_eq!(world.universe_id, universe_id);

    runtime
        .submit_event(aos_node::SubmitEventRequest {
            universe_id,
            world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 1 } }),
            submission_id: Some("domain-start".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    wait_for_worker(&mut supervisor).await;

    let runtime_info: Value = response_json(
        app.clone()
            .oneshot(
                Request::get(format!("/v1/worlds/{world_id}/runtime"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        runtime_info["universe_id"].as_str().unwrap(),
        universe_id.to_string()
    );

    let manifest: Value = response_json(
        app.clone()
            .oneshot(
                Request::get(format!("/v1/worlds/{world_id}/manifest"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(manifest["manifest_hash"].as_str().unwrap(), manifest_hash);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_uses_world_id_only_world_api_shape() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let universe_id = hosted_universe_id(&runtime);
    let manifest_hash = upload_counter_manifest(&runtime, universe_id);

    let world_id =
        WorldId::from(uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000111").unwrap());
    runtime
        .create_world(
            universe_id,
            aos_node::CreateWorldRequest {
                world_id: Some(world_id),
                universe_id,
                created_at_ns: 1,
                source: aos_node::CreateWorldSource::Manifest { manifest_hash },
            },
        )
        .unwrap();
    let world = runtime
        .get_world(universe_id, world_id)
        .expect("world created");

    let fetched: Value = response_json(
        app.clone()
            .oneshot(
                Request::get(format!("/v1/worlds/{}", world.world_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(fetched["runtime"]["world_id"], world.world_id.to_string());
    assert!(fetched["runtime"].get("meta").is_none());

    let second = create_counter_world(&runtime, universe_id);
    let worlds: Value = response_json(
        app.clone()
            .oneshot(
                Request::get("/v1/worlds?limit=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(worlds.as_array().unwrap().len(), 1);

    let first_listed = worlds[0]["world_id"].as_str().unwrap().to_owned();
    let paged: Value = response_json(
        app.clone()
            .oneshot(
                Request::get(format!("/v1/worlds?after={first_listed}&limit=10"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        paged
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["world_id"].as_str().unwrap() > first_listed.as_str())
    );
    assert!(
        paged
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["world_id"] == second.world_id.to_string())
    );
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_accepts_events_endpoint() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let universe_id = hosted_universe_id(&runtime);
    let world = create_counter_world(&runtime, universe_id);

    let response: Value = response_json_with_status(
        app.oneshot(
            Request::post(format!("/v1/worlds/{}/events", world.world_id))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&SubmitEventBody {
                        schema: "demo/CounterEvent@1".into(),
                        value: Some(json!({ "Start": { "target": 1 } })),
                        value_json: None,
                        value_b64: None,
                        key_b64: None,
                        correlation_id: None,
                        submission_id: Some("counter-start".into()),
                        expected_world_epoch: Some(world.world_epoch),
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

    assert_eq!(response["submission_id"], "counter-start");
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_wait_for_flush_applies_event_before_response() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let universe_id = hosted_universe_id(&runtime);
    let world = create_counter_world(&runtime, universe_id);

    let response: Value = response_json_with_status(
        app.oneshot(
            Request::post(format!(
                "/v1/worlds/{}/events?wait_for_flush=true&wait_timeout_ms=1000",
                world.world_id
            ))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&SubmitEventBody {
                    schema: "demo/CounterEvent@1".into(),
                    value: Some(json!({ "Start": { "target": 1 } })),
                    value_json: None,
                    value_b64: None,
                    key_b64: None,
                    correlation_id: None,
                    submission_id: Some("counter-start-wait".into()),
                    expected_world_epoch: Some(world.world_epoch),
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

    assert_eq!(response["submission_id"], "counter-start-wait");

    let state = runtime
        .state_json(universe_id, world.world_id, "demo/CounterSM@1", None)
        .unwrap()
        .unwrap();
    assert_eq!(state["pc"], json!({ "$tag": "Counting" }));
    assert_eq!(state["remaining"], 1);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_wait_for_flush_creates_world_before_response() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let universe_id = hosted_universe_id(&runtime);
    let manifest_hash = upload_counter_manifest(&runtime, universe_id);

    let accepted: Value = response_json_with_status(
        app.oneshot(
            Request::post("/v1/worlds?wait_for_flush=true&wait_timeout_ms=1000")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&CreateWorldBody {
                        world_id: None,
                        universe_id,
                        created_at_ns: 1,
                        source: aos_node::CreateWorldSource::Manifest {
                            manifest_hash: manifest_hash.clone(),
                        },
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

    let world_id = accepted["world_id"]
        .as_str()
        .unwrap()
        .parse::<WorldId>()
        .unwrap();

    let world = runtime.get_world(universe_id, world_id).unwrap();
    assert_eq!(world.universe_id, universe_id);

    let world_frames = runtime.world_frames(world_id).unwrap();
    assert!(
        world_frames.iter().any(|frame| frame.world_id == world_id),
        "expected create-world frame to be durably present after wait_for_flush"
    );
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_does_not_expose_removed_platform_routes() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);

    for request in [
        Request::get("/v1/worlds/by-handle/alpha-world")
            .body(Body::empty())
            .unwrap(),
        Request::post("/v1/partitions/0/checkpoint")
            .body(Body::empty())
            .unwrap(),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_submits_and_reads_command_records() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let universe_id = hosted_universe_id(&runtime);
    let world = create_counter_world(&runtime, universe_id);

    let submitted: Value = response_json(
        app.clone()
            .oneshot(
                Request::post(format!("/v1/worlds/{}/governance/shadow", world.world_id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&CommandSubmitBody {
                            command_id: Some("cmd-gov-shadow".into()),
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
    assert_eq!(submitted["command_id"], "cmd-gov-shadow");

    let queued: Value = response_json(
        app.clone()
            .oneshot(
                Request::get(format!(
                    "/v1/worlds/{}/commands/cmd-gov-shadow",
                    world.world_id
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(queued["status"], "queued");

    let record: Value = response_json(
        app.oneshot(
            Request::get(format!(
                "/v1/worlds/{}/commands/cmd-gov-shadow",
                world.world_id
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(record["command"], "gov-shadow");
    assert_eq!(record["status"], "queued");
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_exposes_trace_routes_without_lifecycle_routes() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let universe_id = hosted_universe_id(&runtime);
    let world = create_counter_world(&runtime, universe_id);

    let trace_summary: Value = response_json(
        app.clone()
            .oneshot(
                Request::get(format!(
                    "/v1/worlds/{}/trace-summary?recent_limit=5",
                    world.world_id
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(trace_summary.get("totals").is_some());
    assert!(trace_summary.get("strict_quiescence").is_some());

    let trace = app
        .clone()
        .oneshot(
            Request::get(format!("/v1/worlds/{}/trace", world.world_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(trace.status(), StatusCode::NOT_FOUND);
    assert_ne!(trace.status(), StatusCode::METHOD_NOT_ALLOWED);

    for (method, path, expected_status) in [
        (
            "DELETE",
            format!("/v1/worlds/{}", world.world_id),
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            "POST",
            format!("/v1/worlds/{}/pause", world.world_id),
            StatusCode::NOT_FOUND,
        ),
        (
            "POST",
            format!("/v1/worlds/{}/archive", world.world_id),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), expected_status);
    }
}
