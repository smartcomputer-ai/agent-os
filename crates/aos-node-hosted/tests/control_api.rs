use std::sync::Arc;

use aos_air_types::{CURRENT_AIR_VERSION, Manifest, SecretDecl, SecretEntry};
use aos_cbor::Hash;
use aos_effect_types::{GovPatchInput, GovProposeParams, HashRef};
use aos_effects::ReceiptStatus;
use aos_fdb::{
    CommandStatus, CommandStore, CreateUniverseRequest, CreateWorldRequest, CreateWorldSeedRequest,
    CreateWorldSource, DomainEventIngress, HostedCoordinationStore, MemoryWorldPersistence,
    NodeCatalog, PersistenceConfig, ProjectionStore, SecretStore, SeedKind, SnapshotRecord,
    UniverseId, UniverseStore, WorkerHeartbeat, WorldAdminStore, WorldId, WorldIngressStore,
    WorldSeed, WorldStore, default_universe_handle, default_world_handle,
};
use aos_kernel::governance::ManifestPatch;
use aos_kernel::journal::{
    DomainEventRecord, EffectReceiptRecord, JournalKind, JournalRecord, OwnedJournalEntry,
};
use aos_kernel::snapshot::{KernelSnapshot, SnapshotRootCompleteness, WorkflowStateEntry};
use aos_node_hosted::config::FdbWorkerConfig;
use aos_node_hosted::control::{
    self, ControlFacade, CreateUniverseBody, PatchUniverseBody, PatchWorldBody,
};
use aos_node_hosted::{FdbWorker, WorkerSupervisor};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use indexmap::IndexMap;
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

fn universe() -> UniverseId {
    UniverseId::from(Uuid::new_v4())
}

fn world() -> WorldId {
    WorldId::from(Uuid::new_v4())
}

fn empty_manifest() -> Manifest {
    Manifest {
        air_version: CURRENT_AIR_VERSION.to_string(),
        schemas: Vec::new(),
        modules: Vec::new(),
        effects: Vec::new(),
        effect_bindings: Vec::new(),
        caps: Vec::new(),
        policies: Vec::new(),
        secrets: Vec::new(),
        defaults: None,
        module_bindings: IndexMap::new(),
        routing: None,
    }
}

fn secret_manifest(binding_id: &str) -> Manifest {
    let mut manifest = empty_manifest();
    manifest.secrets.push(SecretEntry::Decl(SecretDecl {
        alias: "llm/api".into(),
        version: 1,
        binding_id: binding_id.into(),
        expected_digest: Some(HashRef::new(Hash::of_bytes(b"top-secret").to_hex()).unwrap()),
        policy: None,
    }));
    manifest
}

fn workspace_history_bytes(workspace_name: &str) -> (Vec<u8>, String, String) {
    #[derive(serde::Serialize)]
    struct WorkspaceCommitMeta<'a> {
        root_hash: &'a str,
        owner: &'a str,
        created_at: u64,
    }
    #[derive(serde::Serialize)]
    struct WorkspaceHistory<'a> {
        latest: u64,
        versions: std::collections::BTreeMap<u64, WorkspaceCommitMeta<'a>>,
    }

    let root_v1 = Hash::of_bytes(b"workspace-root-v1").to_hex();
    let root_v2 = Hash::of_bytes(b"workspace-root-v2").to_hex();
    let versions = std::collections::BTreeMap::from([
        (
            1,
            WorkspaceCommitMeta {
                root_hash: &root_v1,
                owner: "alice",
                created_at: 111,
            },
        ),
        (
            2,
            WorkspaceCommitMeta {
                root_hash: &root_v2,
                owner: "bob",
                created_at: 222,
            },
        ),
    ]);
    let bytes = serde_cbor::to_vec(&WorkspaceHistory {
        latest: 2,
        versions,
    })
    .expect("encode workspace history");
    let _ = workspace_name;
    (bytes, root_v1, root_v2)
}

fn seed_world(
    persistence: &MemoryWorldPersistence,
    universe: UniverseId,
    world: WorldId,
) -> CreateWorldSeedRequest {
    let manifest = empty_manifest();
    let manifest_bytes = aos_cbor::to_canonical_cbor(&manifest).expect("manifest bytes");
    let manifest_hash = persistence
        .cas_put_verified(universe, &manifest_bytes)
        .expect("store manifest");

    let (workspace_state, _root_v1, _root_v2) = workspace_history_bytes("shell");
    let mut snapshot = KernelSnapshot::new(
        5,
        vec![
            WorkflowStateEntry {
                workflow: "com.acme/Simple@1".into(),
                key: None,
                state: b"hello-state".to_vec(),
                state_hash: *Hash::of_bytes(b"hello-state").as_bytes(),
                last_active_ns: 777,
            },
            WorkflowStateEntry {
                workflow: "sys/Workspace@1".into(),
                key: Some(serde_cbor::to_vec(&"shell".to_string()).expect("workspace key bytes")),
                state: workspace_state.clone(),
                state_hash: *Hash::of_bytes(&workspace_state).as_bytes(),
                last_active_ns: 888,
            },
        ],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        50,
        Some(*manifest_hash.as_bytes()),
    );
    snapshot.set_root_completeness(SnapshotRootCompleteness {
        manifest_hash: Some(manifest_hash.as_bytes().to_vec()),
        ..SnapshotRootCompleteness::default()
    });
    let snapshot_bytes = serde_cbor::to_vec(&snapshot).expect("snapshot bytes");
    let snapshot_hash = persistence
        .cas_put_verified(universe, &snapshot_bytes)
        .expect("store snapshot");

    CreateWorldSeedRequest {
        world_id: Some(world),
        handle: None,
        seed: WorldSeed {
            baseline: SnapshotRecord {
                snapshot_ref: snapshot_hash.to_hex(),
                height: 5,
                logical_time_ns: 50,
                receipt_horizon_height: Some(5),
                manifest_hash: Some(manifest_hash.to_hex()),
            },
            seed_kind: SeedKind::Genesis,
            imported_from: None,
        },
        placement_pin: Some("gpu".into()),
        created_at_ns: 123,
    }
}

fn build_facade() -> (
    Arc<MemoryWorldPersistence>,
    ControlFacade<MemoryWorldPersistence>,
    UniverseId,
    WorldId,
) {
    let persistence = Arc::new(MemoryWorldPersistence::with_config(
        PersistenceConfig::default(),
    ));
    let universe = universe();
    let world = world();
    persistence
        .create_universe(CreateUniverseRequest {
            universe_id: Some(universe),
            handle: None,
            created_at_ns: 11,
        })
        .expect("create universe");
    persistence
        .world_create_from_seed(universe, seed_world(&persistence, universe, world))
        .expect("create world");
    (
        Arc::clone(&persistence),
        ControlFacade::new(persistence),
        universe,
        world,
    )
}

fn build_worker_ready_facade() -> (
    Arc<MemoryWorldPersistence>,
    ControlFacade<MemoryWorldPersistence>,
    UniverseId,
    WorldId,
) {
    let persistence = Arc::new(MemoryWorldPersistence::with_config(
        PersistenceConfig::default(),
    ));
    let universe = universe();
    let world = world();
    persistence
        .create_universe(CreateUniverseRequest {
            universe_id: Some(universe),
            handle: None,
            created_at_ns: 11,
        })
        .expect("create universe");
    persistence
        .world_create_from_seed(universe, empty_seed_world(&persistence, universe, world))
        .expect("create world");
    (
        Arc::clone(&persistence),
        ControlFacade::new(persistence),
        universe,
        world,
    )
}

fn empty_seed_world(
    persistence: &MemoryWorldPersistence,
    universe: UniverseId,
    world: WorldId,
) -> CreateWorldSeedRequest {
    let manifest = empty_manifest();
    let manifest_bytes = aos_cbor::to_canonical_cbor(&manifest).expect("manifest bytes");
    let manifest_hash = persistence
        .cas_put_verified(universe, &manifest_bytes)
        .expect("store manifest");

    let mut snapshot = KernelSnapshot::new(
        0,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        0,
        Some(*manifest_hash.as_bytes()),
    );
    snapshot.set_root_completeness(SnapshotRootCompleteness {
        manifest_hash: Some(manifest_hash.as_bytes().to_vec()),
        ..SnapshotRootCompleteness::default()
    });
    let snapshot_bytes = serde_cbor::to_vec(&snapshot).expect("snapshot bytes");
    let snapshot_hash = persistence
        .cas_put_verified(universe, &snapshot_bytes)
        .expect("store snapshot");

    CreateWorldSeedRequest {
        world_id: Some(world),
        handle: None,
        seed: WorldSeed {
            baseline: SnapshotRecord {
                snapshot_ref: snapshot_hash.to_hex(),
                height: 0,
                logical_time_ns: 0,
                receipt_horizon_height: Some(0),
                manifest_hash: Some(manifest_hash.to_hex()),
            },
            seed_kind: SeedKind::Genesis,
            imported_from: None,
        },
        placement_pin: Some("gpu".into()),
        created_at_ns: 123,
    }
}

fn append_journal_records(
    persistence: &MemoryWorldPersistence,
    universe: UniverseId,
    world: WorldId,
    expected_head: u64,
    records: &[JournalRecord],
) {
    let entries = records
        .iter()
        .enumerate()
        .map(|(offset, record)| {
            let entry = OwnedJournalEntry {
                seq: expected_head + offset as u64,
                kind: record.kind(),
                payload: serde_cbor::to_vec(record).expect("encode journal record"),
            };
            serde_cbor::to_vec(&entry).expect("encode owned entry")
        })
        .collect::<Vec<_>>();
    persistence
        .journal_append_batch(universe, world, expected_head, &entries)
        .expect("append journal records");
}

#[test]
fn facade_serves_admin_and_projection_reads() {
    let (persistence, facade, universe, world) = build_facade();

    let universes = facade.list_universes(None, 10).expect("list universes");
    assert_eq!(universes.len(), 1);
    assert_eq!(universes[0].record.universe_id, universe);
    assert_eq!(
        universes[0].record.meta.handle,
        default_universe_handle(universe)
    );
    let universe_by_handle = facade
        .get_universe_by_handle(&default_universe_handle(universe))
        .expect("get universe by handle");
    assert_eq!(universe_by_handle.record.universe_id, universe);

    let created = facade
        .create_universe(CreateUniverseBody {
            universe_id: None,
            handle: None,
            created_at_ns: 22,
        })
        .expect("create second universe");
    assert_eq!(created.record.created_at_ns, 22);

    let worlds = facade.list_worlds(universe, None, 10).expect("list worlds");
    assert_eq!(worlds.len(), 1);
    assert_eq!(worlds[0].world_id, world);
    assert_eq!(worlds[0].meta.handle, default_world_handle(world));
    let world_by_handle = facade
        .get_world_by_handle(universe, &default_world_handle(world))
        .expect("get world by handle");
    assert_eq!(world_by_handle.runtime.world_id, world);

    let renamed_universe = facade
        .patch_universe(
            universe,
            PatchUniverseBody {
                handle: Some("prod-core".into()),
            },
        )
        .expect("patch universe");
    assert_eq!(renamed_universe.record.meta.handle, "prod-core");

    let world_info = facade.get_world(universe, world).expect("get world");
    assert_eq!(
        world_info.runtime.meta.placement_pin.as_deref(),
        Some("gpu")
    );
    assert_eq!(world_info.active_baseline.height, 5);

    let patched = facade
        .patch_world(
            universe,
            world,
            PatchWorldBody {
                handle: Some("shell-main".into()),
                placement_pin: Some(Some("cpu".into())),
            },
        )
        .expect("patch world");
    assert_eq!(patched.runtime.meta.placement_pin.as_deref(), Some("cpu"));
    assert_eq!(patched.runtime.meta.handle, "shell-main");
    let renamed_world = facade
        .get_world_by_handle(universe, "shell-main")
        .expect("renamed world by handle");
    assert_eq!(renamed_world.runtime.world_id, world);

    let manifest = facade.manifest(universe, world).expect("manifest");
    assert_eq!(manifest.journal_head, 5);
    assert_eq!(manifest.manifest.air_version, CURRENT_AIR_VERSION);

    let defs = facade
        .defs_list(universe, world, None, None)
        .expect("defs list");
    assert!(!defs.defs.is_empty());

    let state = facade
        .state_get(
            universe,
            world,
            "com.acme/Simple@1",
            None,
            Some("latest_durable"),
        )
        .expect("state get");
    assert_eq!(
        BASE64_STANDARD
            .decode(state.state_b64.expect("state bytes"))
            .unwrap(),
        b"hello-state"
    );

    let cells = facade
        .state_list(universe, world, "com.acme/Simple@1", 10, Some("head"))
        .expect("state list");
    assert_eq!(cells.cells.len(), 1);

    let resolved = facade
        .workspace_resolve(universe, world, "shell", Some(2))
        .expect("workspace resolve");
    assert!(resolved.receipt.exists);
    assert_eq!(resolved.receipt.resolved_version, Some(2));

    let empty_root = facade.workspace_empty_root(universe).expect("empty root");
    let applied = facade
        .workspace_apply(
            universe,
            empty_root.clone(),
            control::WorkspaceApplyRequest {
                operations: vec![control::WorkspaceApplyOp::WriteBytes {
                    path: "notes.txt".into(),
                    bytes_b64: BASE64_STANDARD.encode(b"hello ws"),
                    mode: Some(0o644),
                }],
            },
        )
        .expect("workspace apply");
    let entry = facade
        .workspace_entry(universe, &applied.new_root_hash, "notes.txt")
        .expect("workspace entry")
        .expect("entry present");
    assert_eq!(entry.kind, "file");
    let bytes = facade
        .workspace_bytes(universe, &applied.new_root_hash, "notes.txt", None)
        .expect("workspace bytes");
    assert_eq!(bytes, b"hello ws");
    let diff = facade
        .workspace_diff(universe, &empty_root, &applied.new_root_hash, None)
        .expect("workspace diff");
    assert_eq!(diff.changes.len(), 1);

    let blob = facade
        .put_blob(universe, b"blob-data", None)
        .expect("put blob");
    let blob_hash = Hash::from_hex_str(&blob.hash).expect("hash parse");
    assert!(
        facade
            .head_blob(universe, blob_hash)
            .expect("head blob")
            .exists
    );
    assert_eq!(
        facade.get_blob(universe, blob_hash).expect("get blob"),
        b"blob-data"
    );

    let seq = facade
        .enqueue_event(
            universe,
            world,
            DomainEventIngress {
                schema: "com.acme/Test@1".into(),
                value: aos_fdb::CborPayload::inline(
                    serde_cbor::to_vec(&serde_json::json!({"ok": true})).unwrap(),
                ),
                key: None,
                correlation_id: Some("corr-1".into()),
            },
        )
        .expect("enqueue event");
    assert!(!seq.as_bytes().is_empty());

    let journal_entry = OwnedJournalEntry {
        seq: 6,
        kind: JournalKind::Custom,
        payload: serde_cbor::to_vec(&serde_json::json!({"kind":"custom"})).unwrap(),
    };
    let raw = serde_cbor::to_vec(&journal_entry).unwrap();
    persistence
        .journal_append_batch(universe, world, 6, &[raw])
        .expect("append journal");

    let head = facade.journal_head(universe, world).expect("journal head");
    assert_eq!(head.journal_head, 7);

    let journal = facade
        .journal_entries(universe, world, 6, 10)
        .expect("journal entries");
    assert_eq!(journal.entries.len(), 1);
    assert_eq!(journal.entries[0].seq, 6);
}

#[test]
fn facade_lists_workers_and_worker_worlds() {
    let (persistence, facade, universe, world) = build_facade();
    let now_ns = aos_runtime::now_wallclock_ns();
    persistence
        .heartbeat_worker(WorkerHeartbeat {
            worker_id: "worker-a".into(),
            pins: vec!["default".into()],
            last_seen_ns: now_ns,
            expires_at_ns: now_ns + 30_000_000_000,
        })
        .expect("heartbeat");
    let _lease = persistence
        .acquire_world_lease(universe, world, "worker-a", now_ns, 30_000_000_000)
        .expect("lease");

    let workers = facade.workers(universe, 10).expect("workers");
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0].worker_id, "worker-a");

    let worlds = facade
        .worker_worlds(universe, "worker-a", 10)
        .expect("worker worlds");
    assert_eq!(worlds.len(), 1);
    assert_eq!(worlds[0].world_id, world);
}

#[test]
fn facade_serves_coarse_trace_and_trace_summary() {
    let (persistence, facade, universe, world) = build_facade();
    append_journal_records(
        &persistence,
        universe,
        world,
        6,
        &[
            JournalRecord::DomainEvent(DomainEventRecord {
                schema: "com.acme/Test@1".into(),
                value: serde_cbor::to_vec(&serde_json::json!({
                    "request": { "id": "req-1" },
                    "ok": true
                }))
                .unwrap(),
                key: None,
                now_ns: 100,
                logical_now_ns: 100,
                journal_height: 6,
                entropy: Vec::new(),
                event_hash: "evt-1".into(),
                manifest_hash: String::new(),
            }),
            JournalRecord::EffectReceipt(EffectReceiptRecord {
                intent_hash: *Hash::of_bytes(b"intent-1").as_bytes(),
                adapter_id: "adapter-a".into(),
                status: ReceiptStatus::Ok,
                payload_cbor: serde_cbor::to_vec(&serde_json::json!({"ok": true})).unwrap(),
                payload_ref: None,
                payload_size: None,
                payload_sha256: None,
                cost_cents: None,
                signature: Vec::new(),
                now_ns: 101,
                logical_now_ns: 101,
                journal_height: 6,
                entropy: Vec::new(),
                manifest_hash: String::new(),
            }),
        ],
    );

    let trace = facade
        .trace(universe, world, Some("evt-1"), None, None, None, Some(10))
        .expect("trace");
    assert_eq!(trace["root"]["event_hash"], "evt-1");
    assert_eq!(trace["terminal_state"], "completed");
    assert_eq!(
        trace["journal_window"]["entries"].as_array().unwrap().len(),
        2
    );
    assert_eq!(trace["coarse"], true);

    let correlated = facade
        .trace(
            universe,
            world,
            None,
            Some("com.acme/Test@1"),
            Some("$.request.id"),
            Some(serde_json::Value::String("req-1".into())),
            Some(10),
        )
        .expect("correlated trace");
    assert_eq!(correlated["root"]["event_hash"], "evt-1");

    let summary = facade
        .trace_summary(universe, world, 5)
        .expect("trace summary");
    assert_eq!(summary["totals"]["journal"]["domain_events"], 1);
    assert_eq!(summary["totals"]["effects"]["receipts"]["ok"], 1);
    assert_eq!(summary["coarse"], true);
    assert_eq!(summary["recent_journal"].as_array().unwrap().len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn http_router_serves_core_routes() {
    let (persistence, facade, universe, world) = build_facade();
    append_journal_records(
        &persistence,
        universe,
        world,
        6,
        &[
            JournalRecord::DomainEvent(DomainEventRecord {
                schema: "com.acme/Test@1".into(),
                value: serde_cbor::to_vec(&serde_json::json!({"request": {"id": "http-1"}}))
                    .unwrap(),
                key: None,
                now_ns: 200,
                logical_now_ns: 200,
                journal_height: 6,
                entropy: Vec::new(),
                event_hash: "evt-http-1".into(),
                manifest_hash: String::new(),
            }),
            JournalRecord::EffectReceipt(EffectReceiptRecord {
                intent_hash: *Hash::of_bytes(b"http-intent").as_bytes(),
                adapter_id: "adapter-http".into(),
                status: ReceiptStatus::Ok,
                payload_cbor: serde_cbor::to_vec(&serde_json::json!({"ok": true})).unwrap(),
                payload_ref: None,
                payload_size: None,
                payload_sha256: None,
                cost_cents: None,
                signature: Vec::new(),
                now_ns: 201,
                logical_now_ns: 201,
                journal_height: 6,
                entropy: Vec::new(),
                manifest_hash: String::new(),
            }),
        ],
    );
    let router = control::router(Arc::new(facade));

    let health = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("health response");
    assert_eq!(health.status(), StatusCode::OK);

    let info = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("info response");
    assert_eq!(info.status(), StatusCode::NOT_FOUND);

    let runtime = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/universes/{universe}/worlds/{world}/runtime"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("runtime response");
    assert_eq!(runtime.status(), StatusCode::OK);

    let event_body = serde_json::json!({
        "schema": "com.acme/Test@1",
        "value_json": { "ok": true }
    });
    let events = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/universes/{universe}/worlds/{world}/events"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&event_body).unwrap()))
                .unwrap(),
        )
        .await
        .expect("events response");
    assert_eq!(events.status(), StatusCode::ACCEPTED);

    let pause_body = serde_json::json!({
        "command_id": "cmd-pause-http",
        "actor": "ops@example.com",
        "reason": "maintenance window"
    });
    let pause = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/universes/{universe}/worlds/{world}/pause"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&pause_body).unwrap()))
                .unwrap(),
        )
        .await
        .expect("pause response");
    assert_eq!(pause.status(), StatusCode::ACCEPTED);
    let pause_body = axum::body::to_bytes(pause.into_body(), usize::MAX)
        .await
        .unwrap();
    let pause_json: serde_json::Value = serde_json::from_slice(&pause_body).unwrap();
    assert_eq!(pause_json["command_id"], "cmd-pause-http");
    assert_eq!(
        pause_json["poll_url"],
        format!("/v1/universes/{universe}/worlds/{world}/commands/cmd-pause-http")
    );
    assert_eq!(pause_json["status"], "queued");

    let pause_status = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/universes/{universe}/worlds/{world}/commands/cmd-pause-http"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("pause status response");
    assert_eq!(pause_status.status(), StatusCode::OK);
    let pause_status_body = axum::body::to_bytes(pause_status.into_body(), usize::MAX)
        .await
        .unwrap();
    let pause_status_json: serde_json::Value = serde_json::from_slice(&pause_status_body).unwrap();
    assert_eq!(pause_status_json["command"], "world-pause");
    assert_eq!(pause_status_json["status"], "queued");
    assert_eq!(
        persistence
            .command_record(universe, world, "cmd-pause-http")
            .expect("pause command record")
            .expect("pause command exists")
            .status,
        CommandStatus::Queued
    );

    let patch_hash = HashRef::new(Hash::of_bytes(b"http-patch").to_hex()).expect("patch hash");
    let gov_body = serde_json::to_value(GovProposeParams {
        patch: GovPatchInput::Hash(patch_hash),
        summary: None,
        manifest_base: None,
        description: Some("http governance".into()),
    })
    .unwrap();
    let gov_body = match gov_body {
        serde_json::Value::Object(mut map) => {
            map.insert(
                "command_id".into(),
                serde_json::Value::String("cmd-gov-http".into()),
            );
            serde_json::Value::Object(map)
        }
        _ => unreachable!("gov propose params serialize to object"),
    };
    let gov = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/universes/{universe}/worlds/{world}/governance/propose"
                ))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&gov_body).unwrap()))
                .unwrap(),
        )
        .await
        .expect("governance response");
    assert_eq!(gov.status(), StatusCode::ACCEPTED);
    assert_eq!(
        persistence
            .command_record(universe, world, "cmd-gov-http")
            .expect("gov command record")
            .expect("gov command exists")
            .command,
        "gov-propose"
    );

    let empty_root = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/universes/{universe}/workspace/roots"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("empty root response");
    assert_eq!(empty_root.status(), StatusCode::CREATED);
    let empty_root_body = axum::body::to_bytes(empty_root.into_body(), usize::MAX)
        .await
        .unwrap();
    let root_hash =
        serde_json::from_slice::<serde_json::Value>(&empty_root_body).unwrap()["root_hash"]
            .as_str()
            .unwrap()
            .to_string();

    let apply_body = serde_json::json!({
        "operations": [
            {
                "op": "write_bytes",
                "path": "api.txt",
                "bytes_b64": BASE64_STANDARD.encode(b"http apply")
            }
        ]
    });
    let apply = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/universes/{universe}/workspace/roots/{root_hash}/apply"
                ))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&apply_body).unwrap()))
                .unwrap(),
        )
        .await
        .expect("apply response");
    assert_eq!(apply.status(), StatusCode::OK);
    let apply_body = axum::body::to_bytes(apply.into_body(), usize::MAX)
        .await
        .unwrap();
    let new_root_hash =
        serde_json::from_slice::<serde_json::Value>(&apply_body).unwrap()["new_root_hash"]
            .as_str()
            .unwrap()
            .to_string();

    let diff = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/universes/{universe}/workspace/diffs"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "root_a": root_hash,
                        "root_b": new_root_hash,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .expect("diff response");
    assert_eq!(diff.status(), StatusCode::OK);
    let diff_json = response_json(diff).await;
    assert_eq!(diff_json["changes"].as_array().unwrap().len(), 1);

    let bytes = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/universes/{universe}/workspace/roots/{new_root_hash}/bytes?path=api.txt"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("bytes response");
    assert_eq!(bytes.status(), StatusCode::OK);
    let body = axum::body::to_bytes(bytes.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"http apply");

    let trace = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/universes/{universe}/worlds/{world}/trace?event_hash=evt-http-1"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("trace response");
    assert_eq!(trace.status(), StatusCode::OK);
    let trace_body = axum::body::to_bytes(trace.into_body(), usize::MAX)
        .await
        .unwrap();
    let trace_json: serde_json::Value = serde_json::from_slice(&trace_body).unwrap();
    assert_eq!(trace_json["root"]["event_hash"], "evt-http-1");

    let summary = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/universes/{universe}/worlds/{world}/trace-summary?recent_limit=5"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("trace summary response");
    assert_eq!(summary.status(), StatusCode::OK);
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_json: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(summary_json["totals"]["journal"]["domain_events"], 1);

    let cas = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/universes/{universe}/cas/blobs"))
                .body(Body::from("blob-http"))
                .unwrap(),
        )
        .await
        .expect("cas response");
    assert_eq!(cas.status(), StatusCode::CREATED);
}

#[tokio::test(flavor = "current_thread")]
async fn http_cas_put_accepts_blob_larger_than_default_body_limit() {
    let (_persistence, facade, universe, _world) = build_facade();
    let router = control::router(Arc::new(facade));
    let payload = vec![0x5a; 3 * 1024 * 1024];
    let hash = Hash::of_bytes(&payload).to_hex();

    let response = router
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/v1/universes/{universe}/cas/blobs/{hash}"))
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .expect("cas put response");

    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test(flavor = "current_thread")]
async fn http_journal_entries_can_return_raw_cbor() {
    #[derive(serde::Deserialize)]
    struct RawJournalEntry {
        seq: u64,
        #[serde(with = "serde_bytes")]
        entry_cbor: Vec<u8>,
    }

    #[derive(serde::Deserialize)]
    struct RawJournalEntries {
        from: u64,
        next_from: u64,
        entries: Vec<RawJournalEntry>,
    }

    let (persistence, facade, universe, world) = build_facade();
    append_journal_records(
        &persistence,
        universe,
        world,
        6,
        &[JournalRecord::DomainEvent(DomainEventRecord {
            schema: "com.acme/Cbor@1".into(),
            value: serde_cbor::to_vec(&serde_json::json!({"ok": true})).unwrap(),
            key: None,
            now_ns: 300,
            logical_now_ns: 300,
            journal_height: 6,
            entropy: Vec::new(),
            event_hash: "evt-cbor-1".into(),
            manifest_hash: String::new(),
        })],
    );
    let router = control::router(Arc::new(facade));

    let response = router
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/universes/{universe}/worlds/{world}/journal?from=6&limit=10"
                ))
                .header("accept", "application/cbor")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("journal cbor response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/cbor")
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let raw: RawJournalEntries = serde_cbor::from_slice(&body).unwrap();
    assert_eq!(raw.from, 6);
    assert_eq!(raw.next_from, 7);
    assert_eq!(raw.entries.len(), 1);
    assert_eq!(raw.entries[0].seq, 6);

    let entry: OwnedJournalEntry = serde_cbor::from_slice(&raw.entries[0].entry_cbor).unwrap();
    assert_eq!(entry.seq, 6);
    assert_eq!(entry.kind, JournalKind::DomainEvent);
}

#[tokio::test(flavor = "current_thread")]
async fn http_create_world_from_manifest_bootstraps_active_baseline() {
    let persistence = Arc::new(MemoryWorldPersistence::with_config(
        PersistenceConfig::default(),
    ));
    let universe = universe();
    persistence
        .create_universe(CreateUniverseRequest {
            universe_id: Some(universe),
            handle: None,
            created_at_ns: 11,
        })
        .expect("create universe");
    let manifest_bytes = aos_cbor::to_canonical_cbor(&empty_manifest()).expect("manifest bytes");
    let manifest_hash = persistence
        .cas_put_verified(universe, &manifest_bytes)
        .expect("store manifest");
    let router = control::router(Arc::new(ControlFacade::new(Arc::clone(&persistence))));

    let create_body = serde_json::to_vec(&CreateWorldRequest {
        world_id: None,
        handle: None,
        placement_pin: Some("gpu".into()),
        created_at_ns: 123,
        source: CreateWorldSource::Manifest {
            manifest_hash: manifest_hash.to_hex(),
        },
    })
    .expect("encode create request");
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/universes/{universe}/worlds"))
                .header("content-type", "application/json")
                .body(Body::from(create_body))
                .unwrap(),
        )
        .await
        .expect("create world response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let world_id = WorldId::from(
        Uuid::parse_str(
            created["record"]["world_id"]
                .as_str()
                .expect("world id string"),
        )
        .expect("parse world id"),
    );

    let runtime = persistence
        .world_runtime_info(universe, world_id, 0)
        .expect("runtime info");
    assert_eq!(runtime.meta.placement_pin.as_deref(), Some("gpu"));
    assert_eq!(
        runtime.meta.lineage,
        Some(aos_fdb::WorldLineage::Genesis { created_at_ns: 123 })
    );
    let baseline = persistence
        .snapshot_active_baseline(universe, world_id)
        .expect("active baseline");
    let expected_manifest_hash = manifest_hash.to_hex();
    assert_eq!(
        baseline.manifest_hash.as_deref(),
        Some(expected_manifest_hash.as_str())
    );
    assert_eq!(baseline.height, 1);

    let head = persistence
        .head_projection(universe, world_id)
        .expect("head projection lookup")
        .expect("head projection present");
    assert_eq!(head.journal_head, baseline.height);
    assert_eq!(head.manifest_hash, expected_manifest_hash);

    let manifest = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/universes/{universe}/worlds/{world_id}/manifest"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("manifest response");
    assert_eq!(manifest.status(), StatusCode::OK);
}

#[tokio::test(flavor = "current_thread")]
async fn http_create_world_from_seed_bootstraps_query_projections() {
    let persistence = Arc::new(MemoryWorldPersistence::with_config(
        PersistenceConfig::default(),
    ));
    let universe = universe();
    let world_id = world();
    persistence
        .create_universe(CreateUniverseRequest {
            universe_id: Some(universe),
            handle: None,
            created_at_ns: 11,
        })
        .expect("create universe");
    let seed = seed_world(&persistence, universe, world_id);
    let router = control::router(Arc::new(ControlFacade::new(Arc::clone(&persistence))));

    let create_body = serde_json::to_vec(&CreateWorldRequest {
        world_id: Some(world_id),
        handle: Some("seeded-world".into()),
        placement_pin: Some("gpu".into()),
        created_at_ns: 123,
        source: CreateWorldSource::Seed {
            seed: seed.seed.clone(),
        },
    })
    .expect("encode create request");
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/universes/{universe}/worlds"))
                .header("content-type", "application/json")
                .body(Body::from(create_body))
                .unwrap(),
        )
        .await
        .expect("create world response");
    assert_eq!(response.status(), StatusCode::CREATED);

    let head = persistence
        .head_projection(universe, world_id)
        .expect("head projection lookup")
        .expect("head projection present");
    assert_eq!(head.journal_head, seed.seed.baseline.height);
    assert_eq!(
        head.manifest_hash,
        seed.seed
            .baseline
            .manifest_hash
            .clone()
            .expect("seed baseline manifest hash"),
    );

    let manifest = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/universes/{universe}/worlds/{world_id}/manifest"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("manifest response");
    assert_eq!(manifest.status(), StatusCode::OK);
}

#[tokio::test(flavor = "current_thread")]
async fn http_fork_world_uses_path_world_id_as_source() {
    let (persistence, facade, universe, world) = build_facade();
    let router = control::router(Arc::new(facade));
    let src_baseline = persistence
        .snapshot_active_baseline(universe, world)
        .expect("source active baseline");

    let response = json_request(
        &router,
        "POST",
        format!("/v1/universes/{universe}/worlds/{world}/fork"),
        serde_json::json!({
            "src_snapshot": { "kind": "active_baseline" },
            "handle": "forked-http-world",
            "forked_at_ns": 456
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response_json(response).await;
    let forked_world_id = WorldId::from(
        Uuid::parse_str(
            body["record"]["world_id"]
                .as_str()
                .expect("forked world id string"),
        )
        .expect("parse forked world id"),
    );

    let runtime = persistence
        .world_runtime_info(universe, forked_world_id, 0)
        .expect("forked runtime info");
    assert_eq!(runtime.meta.handle, "forked-http-world");
    assert_eq!(
        runtime.meta.lineage,
        Some(aos_fdb::WorldLineage::Fork {
            forked_at_ns: 456,
            src_universe_id: universe,
            src_world_id: world,
            src_snapshot_ref: src_baseline.snapshot_ref,
            src_height: src_baseline.height,
        })
    );
}

const BASE64_STANDARD: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn worker_config(worker_id: &str) -> FdbWorkerConfig {
    FdbWorkerConfig {
        worker_id: worker_id.into(),
        worker_pins: std::collections::BTreeSet::from(["default".to_string(), "gpu".to_string()]),
        heartbeat_interval: Duration::from_millis(1),
        heartbeat_ttl: Duration::from_secs(5),
        lease_ttl: Duration::from_secs(5),
        lease_renew_interval: Duration::from_secs(30),
        maintenance_idle_after: Duration::ZERO,
        idle_release_after: Duration::ZERO,
        effect_claim_timeout: Duration::from_secs(5),
        timer_claim_timeout: Duration::from_secs(5),
        ready_scan_limit: 32,
        world_scan_limit: 32,
        max_inbox_batch: 8,
        max_tick_steps_per_cycle: 16,
        max_effects_per_cycle: 8,
        max_timers_per_cycle: 8,
        supervisor_poll_interval: Duration::from_millis(1),
        ..FdbWorkerConfig::default()
    }
}

fn test_supervisor(
    persistence: Arc<MemoryWorldPersistence>,
    universe: UniverseId,
) -> WorkerSupervisor<MemoryWorldPersistence> {
    FdbWorker::new(worker_config("http-test-worker"))
        .with_runtime_for_universes(persistence, [universe])
}

fn run_supervisor_until<F>(
    supervisor: &mut WorkerSupervisor<MemoryWorldPersistence>,
    max_iters: usize,
    mut done: F,
) where
    F: FnMut(&WorkerSupervisor<MemoryWorldPersistence>) -> bool,
{
    for _ in 0..max_iters {
        supervisor.run_once_blocking().expect("run supervisor");
        if done(supervisor) {
            return;
        }
        std::thread::yield_now();
    }
    panic!("worker test condition was not reached");
}

async fn json_request(
    router: &axum::Router,
    method: &str,
    uri: String,
    body: serde_json::Value,
) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .expect("http response")
}

async fn empty_request(
    router: &axum::Router,
    method: &str,
    uri: String,
) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("http response")
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn http_pause_command_executes_and_polls_terminal_success() {
    let (persistence, facade, universe, world) = build_worker_ready_facade();
    let router = control::router(Arc::new(facade));

    let response = json_request(
        &router,
        "POST",
        format!("/v1/universes/{universe}/worlds/{world}/pause"),
        serde_json::json!({
            "command_id": "cmd-pause-e2e",
            "actor": "ops@example.com",
            "reason": "maintenance"
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let mut supervisor = test_supervisor(Arc::clone(&persistence), universe);
    run_supervisor_until(&mut supervisor, 12, |_| {
        let record = persistence
            .command_record(universe, world, "cmd-pause-e2e")
            .unwrap()
            .unwrap();
        record.status == CommandStatus::Succeeded
            && persistence
                .world_runtime_info(universe, world, aos_runtime::now_wallclock_ns())
                .unwrap()
                .meta
                .admin
                .status
                == aos_fdb::WorldAdminStatus::Paused
    });

    let poll = empty_request(
        &router,
        "GET",
        format!("/v1/universes/{universe}/worlds/{world}/commands/cmd-pause-e2e"),
    )
    .await;
    assert_eq!(poll.status(), StatusCode::OK);
    let poll_json = response_json(poll).await;
    assert_eq!(poll_json["status"], "succeeded");
    assert_eq!(poll_json["command"], "world-pause");
    assert!(
        persistence
            .command_record(universe, world, "cmd-pause-e2e")
            .unwrap()
            .unwrap()
            .result_payload
            .is_some()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn http_world_delete_marks_deleting_then_worker_finalizes_deleted() {
    let (persistence, facade, universe, world) = build_facade();
    let router = control::router(Arc::new(facade));

    let before = persistence
        .world_runtime_info(universe, world, aos_runtime::now_wallclock_ns())
        .unwrap();
    let handle = before.meta.handle.clone();

    let response = json_request(
        &router,
        "DELETE",
        format!("/v1/universes/{universe}/worlds/{world}"),
        serde_json::json!({
            "command_id": "delete-op-1",
            "reason": "corrupted world"
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = response_json(response).await;
    assert_eq!(body["runtime"]["world_id"], world.to_string());
    assert_eq!(body["runtime"]["meta"]["admin"]["status"], "deleting");
    assert_eq!(
        body["runtime"]["meta"]["admin"]["operation_id"],
        "delete-op-1"
    );
    assert_eq!(
        body["runtime"]["meta"]["admin"]["reason"],
        "corrupted world"
    );

    let runtime = persistence
        .world_runtime_info(universe, world, aos_runtime::now_wallclock_ns())
        .unwrap();
    assert_eq!(
        runtime.meta.admin.status,
        aos_fdb::WorldAdminStatus::Deleting
    );
    let mut supervisor = test_supervisor(Arc::clone(&persistence), universe);
    run_supervisor_until(&mut supervisor, 12, |_| {
        persistence
            .world_runtime_info(universe, world, aos_runtime::now_wallclock_ns())
            .unwrap()
            .meta
            .admin
            .status
            == aos_fdb::WorldAdminStatus::Deleted
    });
    let runtime = persistence
        .world_runtime_info(universe, world, aos_runtime::now_wallclock_ns())
        .unwrap();
    assert_eq!(
        runtime.meta.admin.status,
        aos_fdb::WorldAdminStatus::Deleted
    );
    assert!(
        persistence
            .world_runtime_info_by_handle(universe, &handle, aos_runtime::now_wallclock_ns())
            .is_err()
    );
    assert!(
        persistence
            .command_record(universe, world, "delete-op-1")
            .unwrap()
            .is_none()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn http_world_archive_marks_archiving_then_worker_finalizes_archived() {
    let (persistence, facade, universe, world) = build_facade();
    let router = control::router(Arc::new(facade));

    let response = json_request(
        &router,
        "POST",
        format!("/v1/universes/{universe}/worlds/{world}/archive"),
        serde_json::json!({
            "command_id": "archive-op-1",
            "reason": "retired"
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = response_json(response).await;
    assert_eq!(body["runtime"]["meta"]["admin"]["status"], "archiving");
    assert_eq!(
        body["runtime"]["meta"]["admin"]["operation_id"],
        "archive-op-1"
    );
    assert_eq!(body["runtime"]["meta"]["admin"]["reason"], "retired");

    let mut supervisor = test_supervisor(Arc::clone(&persistence), universe);
    run_supervisor_until(&mut supervisor, 12, |_| {
        persistence
            .world_runtime_info(universe, world, aos_runtime::now_wallclock_ns())
            .unwrap()
            .meta
            .admin
            .status
            == aos_fdb::WorldAdminStatus::Archived
    });
    let runtime = persistence
        .world_runtime_info(universe, world, aos_runtime::now_wallclock_ns())
        .unwrap();
    assert_eq!(
        runtime.meta.admin.status,
        aos_fdb::WorldAdminStatus::Archived
    );
}

#[tokio::test(flavor = "current_thread")]
async fn http_world_delete_marks_deleting_when_world_has_live_work() {
    let (persistence, facade, universe, world) = build_facade();
    persistence
        .enqueue_ingress(
            universe,
            world,
            aos_fdb::InboxItem::DomainEvent(DomainEventIngress {
                schema: "com.acme/Test@1".into(),
                value: aos_fdb::CborPayload::inline(
                    serde_cbor::to_vec(&serde_json::json!({"busy": true})).unwrap(),
                ),
                key: None,
                correlation_id: Some("delete-busy".into()),
            }),
        )
        .unwrap();
    let router = control::router(Arc::new(facade));

    let response = json_request(
        &router,
        "DELETE",
        format!("/v1/universes/{universe}/worlds/{world}"),
        serde_json::json!({
            "command_id": "delete-op-busy",
            "reason": "busy world"
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = response_json(response).await;
    assert_eq!(body["runtime"]["meta"]["admin"]["status"], "deleting");
}

#[tokio::test(flavor = "current_thread")]
async fn http_governance_propose_executes_and_polls_terminal_success() {
    let (persistence, facade, universe, world) = build_worker_ready_facade();
    let router = control::router(Arc::new(facade));
    let patch = ManifestPatch {
        manifest: empty_manifest(),
        nodes: Vec::new(),
    };
    let patch_bytes = serde_cbor::to_vec(&patch).unwrap();
    let patch_hash = persistence
        .cas_put_verified(universe, &patch_bytes)
        .unwrap();

    let response = json_request(
        &router,
        "POST",
        format!("/v1/universes/{universe}/worlds/{world}/governance/propose"),
        serde_json::json!({
            "command_id": "cmd-gov-success",
            "patch": {
                "$tag": "Hash",
                "$value": patch_hash.to_hex()
            },
            "description": "http success"
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let mut supervisor = test_supervisor(Arc::clone(&persistence), universe);
    run_supervisor_until(&mut supervisor, 12, |_| {
        persistence
            .command_record(universe, world, "cmd-gov-success")
            .unwrap()
            .unwrap()
            .status
            == CommandStatus::Succeeded
    });

    let poll = empty_request(
        &router,
        "GET",
        format!("/v1/universes/{universe}/worlds/{world}/commands/cmd-gov-success"),
    )
    .await;
    assert_eq!(poll.status(), StatusCode::OK);
    let poll_json = response_json(poll).await;
    assert_eq!(poll_json["status"], "succeeded");
    assert_eq!(poll_json["command"], "gov-propose");
    let record = persistence
        .command_record(universe, world, "cmd-gov-success")
        .unwrap()
        .unwrap();
    let payload = record.result_payload.expect("result payload");
    let inline = payload.inline_cbor.expect("inline cbor");
    let receipt: serde_json::Value = serde_cbor::from_slice(&inline).unwrap();
    assert!(receipt["proposal_id"].as_u64().is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn http_governance_propose_invalid_patch_polls_terminal_failure() {
    let (persistence, facade, universe, world) = build_worker_ready_facade();
    let router = control::router(Arc::new(facade));

    let response = json_request(
        &router,
        "POST",
        format!("/v1/universes/{universe}/worlds/{world}/governance/propose"),
        serde_json::json!({
            "command_id": "cmd-gov-fail",
            "patch": {
                "$tag": "Hash",
                "$value": Hash::of_bytes(b"missing-patch").to_hex()
            },
            "description": "http failure"
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let mut supervisor = test_supervisor(Arc::clone(&persistence), universe);
    run_supervisor_until(&mut supervisor, 12, |_| {
        persistence
            .command_record(universe, world, "cmd-gov-fail")
            .unwrap()
            .unwrap()
            .status
            == CommandStatus::Failed
    });

    let poll = empty_request(
        &router,
        "GET",
        format!("/v1/universes/{universe}/worlds/{world}/commands/cmd-gov-fail"),
    )
    .await;
    assert_eq!(poll.status(), StatusCode::OK);
    let poll_json = response_json(poll).await;
    assert_eq!(poll_json["status"], "failed");
    assert_eq!(poll_json["command"], "gov-propose");
    assert!(poll_json["error"]["code"].as_str().is_some());
    assert!(poll_json["error"]["message"].as_str().is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn http_command_submission_is_idempotent_and_rejects_payload_mismatch() {
    let (persistence, facade, universe, world) = build_facade();
    let router = control::router(Arc::new(facade));

    let first = json_request(
        &router,
        "POST",
        format!("/v1/universes/{universe}/worlds/{world}/pause"),
        serde_json::json!({
            "command_id": "cmd-idempotent",
            "reason": "same"
        }),
    )
    .await;
    assert_eq!(first.status(), StatusCode::ACCEPTED);

    let second = json_request(
        &router,
        "POST",
        format!("/v1/universes/{universe}/worlds/{world}/pause"),
        serde_json::json!({
            "command_id": "cmd-idempotent",
            "reason": "same"
        }),
    )
    .await;
    assert_eq!(second.status(), StatusCode::ACCEPTED);

    let second_json = response_json(second).await;
    assert_eq!(second_json["command_id"], "cmd-idempotent");
    assert_eq!(
        persistence
            .command_record(universe, world, "cmd-idempotent")
            .unwrap()
            .unwrap()
            .status,
        CommandStatus::Queued
    );

    let conflict = json_request(
        &router,
        "POST",
        format!("/v1/universes/{universe}/worlds/{world}/pause"),
        serde_json::json!({
            "command_id": "cmd-idempotent",
            "reason": "different"
        }),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
}

#[tokio::test(flavor = "current_thread")]
async fn http_secret_binding_crud_and_version_upload_work() {
    let (persistence, facade, universe, _world) = build_facade();
    let router = control::router(Arc::new(facade));

    let put_binding = json_request(
        &router,
        "PUT",
        format!("/v1/universes/{universe}/secrets/bindings/app-openai"),
        serde_json::json!({
            "source_kind": "node_secret_store",
            "created_at_ns": 111,
            "updated_at_ns": 111,
            "actor": "tester"
        }),
    )
    .await;
    assert_eq!(put_binding.status(), StatusCode::OK);
    let binding_json = response_json(put_binding).await;
    assert_eq!(binding_json["binding_id"], "app-openai");
    assert_eq!(binding_json["source_kind"], "node_secret_store");

    let put_secret = json_request(
        &router,
        "POST",
        format!("/v1/universes/{universe}/secrets/bindings/app-openai/versions"),
        serde_json::json!({
            "plaintext_b64": BASE64_STANDARD.encode("top-secret"),
            "expected_digest": Hash::of_bytes(b"top-secret").to_hex(),
            "created_at_ns": 222,
            "actor": "tester"
        }),
    )
    .await;
    assert_eq!(put_secret.status(), StatusCode::CREATED);
    let put_json = response_json(put_secret).await;
    assert_eq!(put_json["binding_id"], "app-openai");
    assert_eq!(put_json["version"], 1);

    let versions = empty_request(
        &router,
        "GET",
        format!("/v1/universes/{universe}/secrets/bindings/app-openai/versions"),
    )
    .await;
    assert_eq!(versions.status(), StatusCode::OK);
    let versions_json = response_json(versions).await;
    assert_eq!(versions_json.as_array().unwrap().len(), 1);
    assert_eq!(
        versions_json[0]["digest"],
        Hash::of_bytes(b"top-secret").to_hex()
    );
    assert!(versions_json[0]["ciphertext"].is_array());

    let delete = empty_request(
        &router,
        "DELETE",
        format!("/v1/universes/{universe}/secrets/bindings/app-openai?actor=tester"),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);
    let delete_json = response_json(delete).await;
    assert_eq!(delete_json["status"], "disabled");

    let binding = persistence
        .get_secret_binding(universe, "app-openai")
        .unwrap()
        .unwrap();
    assert_eq!(binding.latest_version, Some(1));
}

#[tokio::test(flavor = "current_thread")]
async fn http_create_world_from_manifest_fails_when_secret_binding_missing() {
    let (persistence, facade, universe, _world) = build_facade();
    let router = control::router(Arc::new(facade));
    let manifest = secret_manifest("missing-binding");
    let manifest_bytes = aos_cbor::to_canonical_cbor(&manifest).unwrap();
    let manifest_hash = persistence
        .cas_put_verified(universe, &manifest_bytes)
        .unwrap();

    let response = json_request(
        &router,
        "POST",
        format!("/v1/universes/{universe}/worlds"),
        serde_json::json!({
            "created_at_ns": 333,
            "source": {
                "kind": "manifest",
                "manifest_hash": manifest_hash.to_hex()
            }
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["code"], "invalid_request");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("secret_binding_missing")
    );
}
