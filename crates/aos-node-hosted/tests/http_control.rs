mod common;

use std::sync::Arc;
use std::time::Duration;

use aos_kernel::SecretResolver;
use aos_node::{CborPayload, SnapshotRecord, UniverseId, WorldId};
use aos_node_hosted::control::{CommandSubmitBody, CreateWorldBody, SubmitEventBody};
use aos_node_hosted::kafka::{
    ProjectionKey, ProjectionRecord, ProjectionValue, WorldMetaProjection,
};
use aos_node_hosted::materializer::{
    CellStateProjectionRecord, MaterializedCellRow, Materializer, MaterializerSqliteStore,
};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use base64::Engine;
use serde_json::{Value, json};
use serial_test::serial;
use tower::util::ServiceExt;

use common::*;

fn control_app(runtime: &aos_node_hosted::worker::HostedWorkerRuntime) -> axum::Router {
    let facade = Arc::new(
        aos_node_hosted::test_support::control_facade_from_worker_runtime(runtime.clone())
            .expect("open hosted control"),
    );
    aos_node_hosted::control::router(facade)
}

fn materialize_partition(
    runtime: &aos_node_hosted::worker::HostedWorkerRuntime,
    partition: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let journal_entries = runtime.partition_entries(partition)?;
    let projection_entries = runtime.projection_entries(partition)?;
    let mut materializer = Materializer::<aos_node_hosted::blobstore::HostedCas>::from_config(
        aos_node_hosted::materializer::MaterializerConfig::from_paths(
            runtime.paths(),
            "aos-journal",
        ),
    )?;
    for entry in projection_entries {
        let key: ProjectionKey = serde_cbor::from_slice(&entry.key)?;
        let value = entry
            .value
            .as_ref()
            .map(|bytes| serde_cbor::from_slice::<ProjectionValue>(bytes))
            .transpose()?;
        materializer.apply_projection_record(
            partition,
            entry.offset,
            &ProjectionRecord { key, value },
        )?;
    }
    materializer.materialize_partition(partition, &journal_entries)?;
    Ok(())
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
async fn http_control_does_not_restore_projection_reads_without_materialization() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone());
    let universe_id = hosted_universe_id(&runtime);
    let world = create_counter_world(&runtime, &mut supervisor, universe_id).await;

    runtime
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 1 } }),
            submission_id: Some("reads-start".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    supervisor.run_once().await.unwrap();

    let response = app
        .oneshot(
            Request::get(format!("/v1/worlds/{}/manifest", world.world_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        String::from_utf8_lossy(&body).contains("materialized head projection"),
        "body: {}",
        String::from_utf8_lossy(&body)
    );
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
    assert_eq!(version["version"], 1);
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
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0]["latest_version"], 1);

    let stored_version: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::get(format!(
                    "/v1/secrets/bindings/{encoded_binding_id}/versions/1"
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
        .resolve(binding_id, 1, None)
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
async fn http_control_serves_materialized_state_manifest_journal_and_runtime() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone());
    let universe_id = hosted_universe_id(&runtime);
    seed_timer_builtins(&runtime, universe_id);
    let manifest_hash = upload_timer_manifest(&runtime, universe_id);
    let world =
        create_world_from_manifest(&runtime, &mut supervisor, universe_id, manifest_hash).await;
    materialize_partition(&runtime, 0).unwrap();

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
async fn http_control_serves_materialized_state_endpoints_from_sqlite() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let universe_id = hosted_universe_id(&runtime);
    let world_id = aos_node::WorldId::from(uuid::Uuid::new_v4());
    let workflow = "demo/TestWorkflow@1";
    let key_bytes = b"cell-key".to_vec();
    let state_bytes = serde_cbor::to_vec(&json!({
        "status": "ready",
        "remaining": 3,
    }))
    .unwrap();
    let state_hash = runtime.put_blob(universe_id, &state_bytes).unwrap();
    let mut sqlite = MaterializerSqliteStore::from_paths(runtime.paths()).unwrap();
    sqlite
        .apply_world_meta_projection(
            world_id,
            &WorldMetaProjection {
                universe_id,
                projection_token: "tok-http-control".into(),
                world_epoch: 1,
                journal_head: 7,
                manifest_hash: state_hash.to_hex(),
                active_baseline: SnapshotRecord {
                    snapshot_ref: "sha256:baseline".into(),
                    height: 7,
                    universe_id,
                    logical_time_ns: 0,
                    receipt_horizon_height: Some(7),
                    manifest_hash: Some(state_hash.to_hex()),
                },
                updated_at_ns: 0,
            },
        )
        .unwrap();
    sqlite
        .apply_cell_projection(
            world_id,
            "tok-http-control",
            &MaterializedCellRow {
                cell: CellStateProjectionRecord {
                    journal_head: 7,
                    workflow: workflow.into(),
                    key_hash: aos_cbor::Hash::of_bytes(&key_bytes).as_bytes().to_vec(),
                    key_bytes: key_bytes.clone(),
                    state_hash: state_hash.to_hex(),
                    size: state_bytes.len() as u64,
                    last_active_ns: 123,
                },
                state_payload: CborPayload::externalized(state_hash, state_bytes.len() as u64),
            },
        )
        .unwrap();

    let key_b64 = base64::engine::general_purpose::STANDARD.encode(&key_bytes);
    let state_get: Value = response_json(
        app.clone()
            .oneshot(
                Request::get(format!(
                    "/v1/worlds/{world_id}/state/demo%2FTestWorkflow@1?key_b64={key_b64}"
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(state_get["journal_head"], 7);
    assert_eq!(state_get["workflow"], workflow);
    assert!(state_get["state_b64"].as_str().is_some());

    let state_list: Value = response_json(
        app.oneshot(
            Request::get(format!(
                "/v1/worlds/{world_id}/state/demo%2FTestWorkflow@1/cells"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(state_list["cells"].as_array().unwrap().len(), 1);
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
    let mut supervisor = worker.with_worker_runtime(runtime.clone());
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
        supervisor.run_once().await.unwrap();
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
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id,
            world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 1 } }),
            submission_id: Some("domain-start".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    supervisor.run_once().await.unwrap();
    materialize_partition(&runtime, 0).unwrap();

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

    let sqlite = MaterializerSqliteStore::from_paths(runtime.paths()).unwrap();
    let projection_token = sqlite
        .load_projection_token(world_id)
        .unwrap()
        .expect("materialized projection token");
    let key_bytes = b"domain-key".to_vec();
    let state_bytes = serde_cbor::to_vec(&json!({
        "status": "ready",
        "universe": universe_id.to_string(),
    }))
    .unwrap();
    let state_hash = runtime.put_blob(universe_id, &state_bytes).unwrap();
    sqlite
        .apply_cell_projection(
            world_id,
            &projection_token,
            &MaterializedCellRow {
                cell: CellStateProjectionRecord {
                    journal_head: manifest["journal_head"].as_u64().unwrap(),
                    workflow: "demo/TestWorkflow@1".into(),
                    key_hash: aos_cbor::Hash::of_bytes(&key_bytes).as_bytes().to_vec(),
                    key_bytes: key_bytes.clone(),
                    state_hash: state_hash.to_hex(),
                    size: state_bytes.len() as u64,
                    last_active_ns: 1,
                },
                state_payload: CborPayload::externalized(state_hash, state_bytes.len() as u64),
            },
        )
        .unwrap();
    let key_b64 = base64::engine::general_purpose::STANDARD.encode(key_bytes);
    let state_get: Value = response_json(
        app.oneshot(
            Request::get(format!(
                "/v1/worlds/{world_id}/state/demo%2FTestWorkflow@1?key_b64={key_b64}"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert!(state_get["state_b64"].as_str().is_some());
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_uses_world_id_only_world_api_shape() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone());
    let universe_id = hosted_universe_id(&runtime);
    let manifest_hash = upload_counter_manifest(&runtime, universe_id);

    let world_id =
        WorldId::from(uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000111").unwrap());
    let accepted: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::post("/v1/worlds")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&CreateWorldBody {
                            world_id: Some(world_id),
                            universe_id: UniverseId::nil(),
                            created_at_ns: 1,
                            source: aos_node::CreateWorldSource::Manifest { manifest_hash },
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
    assert_eq!(accepted["world_id"], world_id.to_string());
    let mut world = None;
    for _ in 0..200 {
        supervisor.run_once().await.unwrap();
        if let Ok(summary) = runtime.get_world(universe_id, world_id) {
            world = Some(summary);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let world = world.expect("world created");
    materialize_partition(&runtime, 0).unwrap();

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

    let second = create_counter_world(&runtime, &mut supervisor, universe_id).await;
    materialize_partition(&runtime, 0).unwrap();
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
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone());
    let universe_id = hosted_universe_id(&runtime);
    let world = create_counter_world(&runtime, &mut supervisor, universe_id).await;

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
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone());
    let universe_id = hosted_universe_id(&runtime);
    let world = create_counter_world(&runtime, &mut supervisor, universe_id).await;

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

    supervisor.run_once().await.unwrap();

    let finished: Value = response_json(
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
    assert_eq!(finished["command"], "gov-shadow");
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
    let worker_runtime = ctx.direct_worker_runtime("worker", &[0]);
    let control_runtime = ctx.control_runtime("control");
    let app = control_app(&control_runtime);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(worker_runtime.clone());
    let universe_id = hosted_universe_id(&worker_runtime);
    let manifest_hash =
        upload_authored_manifest(&worker_runtime, universe_id, &counter_world_root());

    let created: Value = response_json_with_status(
        app.clone()
            .oneshot(
                Request::post("/v1/worlds")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&CreateWorldBody {
                            world_id: None,
                            universe_id,
                            created_at_ns: 1,
                            source: aos_node::CreateWorldSource::Manifest { manifest_hash },
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
    let world_id = created["world_id"]
        .as_str()
        .unwrap()
        .parse::<WorldId>()
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        supervisor.run_once().await.unwrap();
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
                Request::post(format!("/v1/worlds/{world_id}/events"))
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
                            params: aos_effect_types::GovShadowParams { proposal_id: 1 },
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

    supervisor.run_once().await.unwrap();
    supervisor.run_once().await.unwrap();

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

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn http_control_exposes_trace_routes_without_lifecycle_routes() {
    let runtime = embedded_runtime(1);
    let app = control_app(&runtime);
    let worker = hosted_worker();
    let mut supervisor = worker.with_worker_runtime(runtime.clone());
    let universe_id = hosted_universe_id(&runtime);
    let world = create_counter_world(&runtime, &mut supervisor, universe_id).await;

    runtime
        .submit_event(aos_node_hosted::SubmitEventRequest {
            universe_id,
            world_id: world.world_id,
            schema: "demo/CounterEvent@1".into(),
            value: json!({ "Start": { "target": 1 } }),
            submission_id: Some("trace-start".into()),
            expected_world_epoch: Some(world.world_epoch),
        })
        .unwrap();
    supervisor.run_once().await.unwrap();

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
    assert_eq!(trace.status(), StatusCode::BAD_REQUEST);

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
