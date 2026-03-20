#[path = "worker_test_fixtures.rs"]
mod worker_fixtures;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aos_air_types::{
    AirNode, DefModule, DefPolicy, DefSchema, EmptyObject, HashRef, NamedRef, SchemaRef, TypeExpr,
    TypePrimitive, TypePrimitiveText, TypeRecord, TypeRef, TypeVariant, WorkflowAbi,
};
use aos_effects::ReceiptStatus;
use aos_effects::builtins::{HttpRequestParams, PortalSendMode, PortalSendParams, TimerSetParams};
use aos_fdb::{
    CborPayload, CommandIngress, CommandRecord, CommandStatus, CommandStore,
    CreateWorldSeedRequest, DomainEventIngress, EffectDispatchItem, ExternalInboxIngress,
    HostedCoordinationStore, HostedEffectQueueStore, HostedRuntimeStore, MemoryWorldPersistence,
    NodeCatalog, PersistenceConfig, ProjectionStore, QueryProjectionMaterialization,
    ReceiptIngress, SeedKind, SnapshotMaintenanceConfig, SnapshotRecord, TimerFiredIngress,
    UniverseId, WorkerHeartbeat, WorldAdminStore, WorldId, WorldRuntimeInfo, WorldSeed, WorldStore,
};
use aos_kernel::StateReader;
use aos_kernel::Store;
use aos_kernel::journal::{JournalRecord, OwnedJournalEntry};
use aos_kernel::query::Consistency;
use aos_node::{
    HostedStore, open_hosted_from_manifest_hash, open_hosted_world, snapshot_hosted_world,
};
use aos_runtime::WorldHost;
use aos_wasm_abi::{DomainEvent, PureOutput, WorkflowEffect, WorkflowOutput};
use indexmap::IndexMap;
use wat::parse_str;
use worker_fixtures::{self as fixtures, START_SCHEMA};

use super::*;

fn universe() -> UniverseId {
    UniverseId::from(uuid::Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap())
}

fn world() -> WorldId {
    WorldId::from(uuid::Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap())
}

fn second_world() -> WorldId {
    WorldId::from(uuid::Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap())
}

fn memory_runtime() -> Arc<MemoryWorldPersistence> {
    Arc::new(MemoryWorldPersistence::new())
}

fn test_worker_config() -> config::FdbWorkerConfig {
    config::FdbWorkerConfig {
        worker_id: "worker-test".into(),
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
        ..config::FdbWorkerConfig::default()
    }
}

fn text_type() -> TypeExpr {
    TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
        text: EmptyObject {},
    }))
}

fn def_text_record_schema(name: &str, fields: Vec<(&str, TypeExpr)>) -> DefSchema {
    DefSchema {
        name: name.into(),
        ty: TypeExpr::Record(TypeRecord {
            record: IndexMap::from_iter(fields.into_iter().map(|(k, ty)| (k.to_string(), ty))),
        }),
    }
}

fn insert_test_schemas(loaded: &mut aos_kernel::manifest::LoadedManifest, schemas: Vec<DefSchema>) {
    for schema in schemas {
        let name = schema.name.clone();
        loaded.schemas.insert(name.clone(), schema);
        if !loaded
            .manifest
            .schemas
            .iter()
            .any(|existing| existing.name == name)
        {
            loaded.manifest.schemas.push(NamedRef {
                name,
                hash: fixtures::zero_hash(),
            });
        }
    }
}

fn state_bytes<P: WorldStore + 'static>(
    persistence: Arc<P>,
    universe: UniverseId,
    world: WorldId,
    workflow: &str,
) -> Result<Option<Vec<u8>>, WorkerError> {
    let persistence: Arc<dyn WorldStore> = persistence;
    let host = open_hosted_world(
        persistence,
        universe,
        world,
        WorldConfig::default(),
        EffectAdapterConfig::default(),
        KernelConfig::default(),
        None,
    )?;
    Ok(host.state(workflow, None))
}

fn hosted_store<P: WorldStore + 'static>(
    persistence: Arc<P>,
    universe: UniverseId,
) -> Arc<HostedStore> {
    let persistence: Arc<dyn WorldStore> = persistence;
    Arc::new(HostedStore::new(persistence, universe))
}

#[derive(Debug, PartialEq, Eq)]
struct HostedRuntimeFingerprint {
    state: Option<Vec<u8>>,
    journal_head: u64,
    active_baseline_height: Option<u64>,
}

fn runtime_fingerprint(host: &WorldHost<HostedStore>, workflow: &str) -> HostedRuntimeFingerprint {
    HostedRuntimeFingerprint {
        state: host.state(workflow, None),
        journal_head: host.kernel().journal_head(),
        active_baseline_height: host.kernel().get_journal_head().active_baseline_height,
    }
}

fn reopen_host_from_runtime(
    runtime: Arc<MemoryWorldPersistence>,
    universe: UniverseId,
    world: WorldId,
) -> Result<WorldHost<HostedStore>, WorkerError> {
    let persistence: Arc<dyn WorldStore> = runtime;
    Ok(open_hosted_world(
        persistence,
        universe,
        world,
        WorldConfig::default(),
        EffectAdapterConfig::default(),
        KernelConfig::default(),
        None,
    )?)
}

fn seed_hosted_world<P: WorldStore + 'static>(
    persistence: Arc<P>,
    universe: UniverseId,
    world: WorldId,
    loaded: aos_kernel::manifest::LoadedManifest,
) -> Result<(), WorkerError> {
    let persistence_dyn: Arc<dyn WorldStore> = persistence.clone();
    let init_seq = persistence.inbox_enqueue(
        universe,
        world,
        InboxItem::Control(CommandIngress {
            command_id: "seed-world".into(),
            command: "seed-world".into(),
            actor: None,
            payload: CborPayload::inline(Vec::new()),
            submitted_at_ns: 0,
        }),
    )?;
    persistence.inbox_commit_cursor(universe, world, None, init_seq)?;
    let store = hosted_store(persistence.clone(), universe);
    let manifest_hash = store_full_manifest(store.as_ref(), &loaded)
        .map_err(|err| WorkerError::Host(HostError::Manifest(err.to_string())))?;
    let mut host = open_hosted_from_manifest_hash(
        persistence_dyn,
        universe,
        world,
        manifest_hash,
        WorldConfig::default(),
        EffectAdapterConfig::default(),
        KernelConfig::default(),
        None,
    )?;
    let persistence_sync: Arc<dyn WorldStore> = persistence.clone();
    snapshot_hosted_world(&mut host, &persistence_sync, universe, world)?;
    Ok(())
}

fn queued_command_record(command_id: &str, command: &str, submitted_at_ns: u64) -> CommandRecord {
    CommandRecord {
        command_id: command_id.into(),
        command: command.into(),
        status: CommandStatus::Queued,
        submitted_at_ns,
        started_at_ns: None,
        finished_at_ns: None,
        journal_height: None,
        manifest_hash: None,
        result_payload: None,
        error: None,
    }
}

fn submit_control_command(
    runtime: &MemoryWorldPersistence,
    universe: UniverseId,
    world: WorldId,
    command_id: &str,
    command: &str,
    payload_cbor: Vec<u8>,
    submitted_at_ns: u64,
) -> Result<(), WorkerError> {
    runtime.submit_command(
        universe,
        world,
        CommandIngress {
            command_id: command_id.into(),
            command: command.into(),
            actor: Some("tester".into()),
            payload: CborPayload::inline(payload_cbor),
            submitted_at_ns,
        },
        queued_command_record(command_id, command, submitted_at_ns),
    )?;
    Ok(())
}

fn seed_hosted_world_from_snapshot<P>(
    persistence: Arc<P>,
    universe: UniverseId,
    world: WorldId,
    loaded: aos_kernel::manifest::LoadedManifest,
    snapshot_bytes: Vec<u8>,
    height: u64,
) -> Result<(), WorkerError>
where
    P: WorldAdminStore + WorldStore + 'static,
{
    let store = hosted_store(Arc::clone(&persistence), universe);
    let manifest_hash = store_full_manifest(store.as_ref(), &loaded)
        .map_err(|err| WorkerError::Host(HostError::Manifest(err.to_string())))?;
    let snapshot_hash = persistence.cas_put_verified(universe, &snapshot_bytes)?;
    persistence.world_create_from_seed(
        universe,
        CreateWorldSeedRequest {
            world_id: Some(world),
            handle: None,
            seed: WorldSeed {
                baseline: SnapshotRecord {
                    snapshot_ref: snapshot_hash.to_hex(),
                    height,
                    logical_time_ns: height * 10,
                    receipt_horizon_height: Some(height),
                    manifest_hash: Some(manifest_hash.to_hex()),
                },
                seed_kind: SeedKind::Genesis,
                imported_from: None,
            },
            placement_pin: None,
            created_at_ns: now_wallclock_ns(),
        },
    )?;
    Ok(())
}

fn build_runner(
    runtime: Arc<MemoryWorldPersistence>,
    universe: UniverseId,
    world: WorldId,
    loaded: aos_kernel::manifest::LoadedManifest,
    worker_config: config::FdbWorkerConfig,
) -> Result<WorldRunner<MemoryWorldPersistence>, WorkerError> {
    seed_hosted_world(Arc::clone(&runtime), universe, world, loaded)?;
    let worker = FdbWorker::new(worker_config.clone());
    let lease = runtime.acquire_world_lease(
        universe,
        world,
        &worker_config.worker_id,
        now_wallclock_ns(),
        duration_ns(worker_config.lease_ttl),
    )?;
    WorldRunner::open(worker, runtime, universe, world, lease)
}

fn enqueue_start_event<P: HostedRuntimeStore + 'static>(
    runtime: &Arc<P>,
    universe: UniverseId,
    world: WorldId,
    schema: &str,
    start_id: &str,
) -> Result<(), WorkerError> {
    runtime.inbox_enqueue(
        universe,
        world,
        InboxItem::DomainEvent(DomainEventIngress {
            schema: schema.into(),
            value: CborPayload::inline(serde_cbor::to_vec(&serde_json::json!({
                "$tag": "Start",
                "$value": fixtures::start_event(start_id),
            }))?),
            key: None,
            correlation_id: Some(format!("corr-{start_id}")),
        }),
    )?;
    Ok(())
}

fn enqueue_simple_start<P: HostedRuntimeStore + 'static>(
    runtime: &Arc<P>,
    universe: UniverseId,
    world: WorldId,
    start_id: &str,
) -> Result<(), WorkerError> {
    runtime.inbox_enqueue(
        universe,
        world,
        InboxItem::DomainEvent(DomainEventIngress {
            schema: START_SCHEMA.into(),
            value: CborPayload::inline(serde_cbor::to_vec(&fixtures::start_event(start_id))?),
            key: None,
            correlation_id: Some(format!("corr-{start_id}")),
        }),
    )?;
    Ok(())
}

fn run_supervisor_until<F>(
    supervisor: &mut WorkerSupervisor<MemoryWorldPersistence>,
    max_iters: usize,
    mut done: F,
) -> Result<(), WorkerError>
where
    F: FnMut(&WorkerSupervisor<MemoryWorldPersistence>) -> Result<bool, WorkerError>,
{
    for _ in 0..max_iters {
        supervisor.run_once_blocking()?;
        if done(supervisor)? {
            return Ok(());
        }
    }
    Err(WorkerError::Host(HostError::External(
        "condition not reached".into(),
    )))
}

fn decode_owned_entry(bytes: &[u8]) -> OwnedJournalEntry {
    serde_cbor::from_slice(bytes).expect("decode owned journal entry")
}

fn decode_record(entry: &OwnedJournalEntry) -> JournalRecord {
    serde_cbor::from_slice(&entry.payload).expect("decode journal record")
}

#[test]
fn resolve_payload_reads_externalized_cbor_from_cas() {
    let persistence = Arc::new(MemoryWorldPersistence::new());
    let payload = vec![1, 2, 3, 4, 5];
    let hash = persistence.cas_put_verified(universe(), &payload).unwrap();
    let resolved = resolve_payload(
        &*persistence,
        universe(),
        &CborPayload::externalized(hash, payload.len() as u64),
    )
    .unwrap();

    assert_eq!(resolved, payload);
}

#[test]
fn rendezvous_hashing_is_stable_for_same_inputs() {
    let first = rendezvous_score(universe(), world(), "worker-a");
    let second = rendezvous_score(universe(), world(), "worker-a");
    let third = rendezvous_score(universe(), world(), "worker-b");

    assert_eq!(first, second);
    assert_ne!(first, third);
}

#[test]
fn effective_world_pin_defaults_to_default() {
    let info = WorldRuntimeInfo {
        world_id: world(),
        meta: aos_fdb::WorldMeta {
            handle: aos_fdb::default_world_handle(world()),
            manifest_hash: None,
            active_baseline_height: None,
            placement_pin: None,
            created_at_ns: 0,
            lineage: None,
            admin: aos_fdb::WorldAdminLifecycle::default(),
        },
        notify_counter: 0,
        has_pending_inbox: false,
        has_pending_effects: false,
        next_timer_due_at_ns: None,
        has_pending_maintenance: false,
        lease: None,
    };

    assert_eq!(effective_world_pin(&info), "default");
}

#[test]
fn worker_eligibility_checks_advertised_pins() {
    let default_worker = WorkerHeartbeat {
        worker_id: "worker-a".into(),
        pins: vec!["default".into()],
        last_seen_ns: 0,
        expires_at_ns: 1,
    };
    let pinned_worker = WorkerHeartbeat {
        worker_id: "worker-b".into(),
        pins: vec!["gpu".into(), "default".into()],
        last_seen_ns: 0,
        expires_at_ns: 1,
    };

    assert!(worker_is_eligible_for_pin(&default_worker, "default"));
    assert!(!worker_is_eligible_for_pin(&default_worker, "gpu"));
    assert!(worker_is_eligible_for_pin(&pinned_worker, "gpu"));
}

#[test]
fn supervisor_filters_rendezvous_candidates_by_effective_pin() {
    let worker = FdbWorker {
        config: config::FdbWorkerConfig {
            worker_id: "worker-b".into(),
            worker_pins: std::collections::BTreeSet::from(["gpu".to_string()]),
            ..config::FdbWorkerConfig::default()
        },
        world_config: WorldConfig::default(),
        adapter_config: EffectAdapterConfig::default(),
        kernel_config: KernelConfig::default(),
        hosted_blob_cache: SharedBlobCache::new(1024, 8 * 1024 * 1024, 1024 * 1024),
    };
    let runtime = Arc::new(MemoryWorldPersistence::new());
    let supervisor = worker.with_runtime_for_universes(runtime, [universe()]);
    let info = WorldRuntimeInfo {
        world_id: world(),
        meta: aos_fdb::WorldMeta {
            handle: aos_fdb::default_world_handle(world()),
            manifest_hash: None,
            active_baseline_height: Some(0),
            placement_pin: Some("gpu".into()),
            created_at_ns: 0,
            lineage: None,
            admin: aos_fdb::WorldAdminLifecycle::default(),
        },
        notify_counter: 0,
        has_pending_inbox: true,
        has_pending_effects: false,
        next_timer_due_at_ns: None,
        has_pending_maintenance: false,
        lease: None,
    };
    let workers = vec![
        WorkerHeartbeat {
            worker_id: "worker-a".into(),
            pins: vec!["default".into()],
            last_seen_ns: 0,
            expires_at_ns: 1,
        },
        WorkerHeartbeat {
            worker_id: "worker-b".into(),
            pins: vec!["gpu".into()],
            last_seen_ns: 0,
            expires_at_ns: 1,
        },
    ];

    assert!(supervisor.should_own_world(universe(), &info, &workers));
}

#[test]
fn memory_supervisor_runs_world_and_releases_when_idle() {
    let runtime = memory_runtime();
    seed_hosted_world(
        Arc::clone(&runtime),
        universe(),
        world(),
        simple_state_manifest(&hosted_store(Arc::clone(&runtime), universe())),
    )
    .unwrap();
    enqueue_simple_start(&runtime, universe(), world(), "wf-1").unwrap();

    let worker = FdbWorker::new(test_worker_config());
    let mut supervisor = worker.with_runtime_for_universes(Arc::clone(&runtime), [universe()]);
    run_supervisor_until(&mut supervisor, 8, |supervisor| {
        Ok(state_bytes(
            Arc::clone(&runtime),
            universe(),
            world(),
            "com.acme/Simple@1",
        )? == Some(vec![0xAA])
            && runtime.inbox_cursor(universe(), world())?.is_some()
            && supervisor.active_worlds().is_empty())
    })
    .unwrap();
}

#[test]
fn memory_supervisor_isolates_corrupted_world_open_failure() {
    let runtime = memory_runtime();
    seed_hosted_world(
        Arc::clone(&runtime),
        universe(),
        world(),
        simple_state_manifest(&hosted_store(Arc::clone(&runtime), universe())),
    )
    .unwrap();

    let mut corrupted = runtime.snapshot_latest(universe(), world()).unwrap();
    corrupted.manifest_hash = Some(Hash::of_bytes(b"poisoned-manifest").to_hex());
    runtime
        .snapshot_repair_record(universe(), world(), corrupted.clone())
        .unwrap();
    runtime
        .snapshot_promote_baseline(universe(), world(), corrupted)
        .unwrap();
    enqueue_simple_start(&runtime, universe(), world(), "wf-corrupt").unwrap();

    let worker = FdbWorker::new(test_worker_config());
    let mut supervisor = worker.with_runtime_for_universes(Arc::clone(&runtime), [universe()]);

    let first = supervisor.run_once_blocking().unwrap();
    assert_eq!(first.worlds_started, 0);
    assert_eq!(first.worlds_fenced, 1);
    assert!(supervisor.active_worlds().is_empty());
    assert!(supervisor.faulted_worlds.contains_key(&ActiveWorldRef {
        universe_id: universe(),
        world_id: world(),
    }));

    let second = supervisor.run_once_blocking().unwrap();
    assert_eq!(second.worlds_started, 0);
    assert_eq!(second.worlds_fenced, 0);
    assert!(supervisor.active_worlds().is_empty());
}

#[test]
fn memory_supervisor_finalizes_deleting_world_after_corrupted_open_failure() {
    let runtime = memory_runtime();
    seed_hosted_world(
        Arc::clone(&runtime),
        universe(),
        world(),
        simple_state_manifest(&hosted_store(Arc::clone(&runtime), universe())),
    )
    .unwrap();

    let mut corrupted = runtime.snapshot_latest(universe(), world()).unwrap();
    corrupted.manifest_hash = Some(Hash::of_bytes(b"poisoned-manifest").to_hex());
    runtime
        .snapshot_repair_record(universe(), world(), corrupted.clone())
        .unwrap();
    runtime
        .snapshot_promote_baseline(universe(), world(), corrupted)
        .unwrap();
    runtime
        .set_world_admin_lifecycle(
            universe(),
            world(),
            WorldAdminLifecycle {
                status: WorldAdminStatus::Deleting,
                updated_at_ns: now_wallclock_ns(),
                operation_id: Some("delete-op".into()),
                reason: Some("corrupted".into()),
            },
        )
        .unwrap();

    let worker = FdbWorker::new(test_worker_config());
    let mut supervisor = worker.with_runtime_for_universes(Arc::clone(&runtime), [universe()]);

    let first = supervisor.run_once_blocking().unwrap();
    assert_eq!(first.worlds_started, 0);
    assert_eq!(first.worlds_fenced, 0);
    assert!(supervisor.active_worlds().is_empty());
    assert!(!supervisor.faulted_worlds.contains_key(&ActiveWorldRef {
        universe_id: universe(),
        world_id: world(),
    }));
    assert_eq!(
        runtime
            .world_runtime_info(universe(), world(), now_wallclock_ns())
            .unwrap()
            .meta
            .admin
            .status,
        WorldAdminStatus::Deleted
    );
}

#[test]
fn memory_supervisor_keeps_released_world_warm_for_fast_resume() {
    let runtime = memory_runtime();
    seed_hosted_world(
        Arc::clone(&runtime),
        universe(),
        world(),
        simple_state_manifest(&hosted_store(Arc::clone(&runtime), universe())),
    )
    .unwrap();
    enqueue_simple_start(&runtime, universe(), world(), "wf-warm-1").unwrap();

    let worker = FdbWorker::new(test_worker_config());
    let mut supervisor = worker.with_runtime_for_universes(Arc::clone(&runtime), [universe()]);
    run_supervisor_until(&mut supervisor, 8, |supervisor| {
        Ok(state_bytes(
            Arc::clone(&runtime),
            universe(),
            world(),
            "com.acme/Simple@1",
        )? == Some(vec![0xAA])
            && supervisor.active_worlds().is_empty()
            && supervisor.warm_worlds.contains_key(&ActiveWorldRef {
                universe_id: universe(),
                world_id: world(),
            }))
    })
    .unwrap();

    let before_cursor = runtime.inbox_cursor(universe(), world()).unwrap();
    enqueue_simple_start(&runtime, universe(), world(), "wf-warm-2").unwrap();
    run_supervisor_until(&mut supervisor, 8, |supervisor| {
        Ok(runtime.inbox_cursor(universe(), world())? != before_cursor
            && supervisor.active_worlds().is_empty()
            && supervisor.warm_worlds.contains_key(&ActiveWorldRef {
                universe_id: universe(),
                world_id: world(),
            }))
    })
    .unwrap();
}

#[test]
fn worker_materializes_head_and_latest_durable_cell_projections() {
    let runtime = memory_runtime();
    seed_hosted_world(
        Arc::clone(&runtime),
        universe(),
        world(),
        simple_state_manifest(&hosted_store(Arc::clone(&runtime), universe())),
    )
    .unwrap();
    enqueue_simple_start(&runtime, universe(), world(), "wf-projection").unwrap();

    let worker = FdbWorker::new(test_worker_config());
    let mut supervisor = worker.with_runtime_for_universes(Arc::clone(&runtime), [universe()]);
    run_supervisor_until(&mut supervisor, 8, |_| {
        let head = runtime.head_projection(universe(), world())?;
        let mono_key_hash = Hash::of_bytes(b"").as_bytes().to_vec();
        let cell = runtime.cell_state_projection(
            universe(),
            world(),
            "com.acme/Simple@1",
            &mono_key_hash,
        )?;
        Ok(head.is_some() && cell.is_some())
    })
    .unwrap();

    let head = runtime
        .head_projection(universe(), world())
        .unwrap()
        .unwrap();
    let cell = runtime
        .cell_state_projection(
            universe(),
            world(),
            "com.acme/Simple@1",
            &Hash::of_bytes(b"").as_bytes().to_vec(),
        )
        .unwrap()
        .unwrap();
    let state = state_bytes(
        Arc::clone(&runtime),
        universe(),
        world(),
        "com.acme/Simple@1",
    )
    .unwrap()
    .unwrap();

    assert!(head.journal_head > 0);
    assert_eq!(
        head.manifest_hash,
        reopen_host_from_runtime(Arc::clone(&runtime), universe(), world())
            .unwrap()
            .kernel()
            .manifest_hash()
            .to_hex()
    );
    assert_eq!(cell.journal_head, head.journal_head);
    assert_eq!(cell.state_hash, Hash::of_bytes(&state).to_hex());
    assert_eq!(cell.size, state.len() as u64);
}

#[test]
fn worker_materializes_workspace_registry_projection() {
    let runtime = memory_runtime();
    let loaded = workspace_registry_manifest();
    let store = hosted_store(Arc::clone(&runtime), universe());
    let root_v1 = store
        .put_node(&serde_cbor::Value::Null)
        .expect("store workspace root v1")
        .to_hex();
    let root_v2 = store
        .put_node(&serde_cbor::Value::Bool(true))
        .expect("store workspace root v2")
        .to_hex();
    let manifest_hash = store_full_manifest(store.as_ref(), &loaded)
        .expect("store workspace manifest for snapshot");
    let snapshot_bytes = workspace_history_snapshot_bytes(
        "shell",
        2,
        &[(1, &root_v1, "alice", 111), (2, &root_v2, "bob", 222)],
        manifest_hash,
        4,
    );
    seed_hosted_world_from_snapshot(
        Arc::clone(&runtime),
        universe(),
        world(),
        loaded,
        snapshot_bytes,
        4,
    )
    .unwrap();
    let worker_config = test_worker_config();
    let worker = FdbWorker::new(worker_config.clone());
    let lease = runtime
        .acquire_world_lease(
            universe(),
            world(),
            &worker_config.worker_id,
            now_wallclock_ns(),
            duration_ns(worker_config.lease_ttl),
        )
        .unwrap();
    runtime
        .materialize_query_projections_guarded(
            universe(),
            world(),
            &lease,
            now_wallclock_ns(),
            QueryProjectionMaterialization {
                head: runtime
                    .head_projection(universe(), world())
                    .unwrap()
                    .unwrap(),
                workflows: Vec::new(),
                workspaces: Vec::new(),
            },
        )
        .unwrap();
    runtime
        .release_world_lease(universe(), world(), &lease)
        .unwrap();

    let runner_lease = runtime
        .acquire_world_lease(
            universe(),
            world(),
            &worker_config.worker_id,
            now_wallclock_ns(),
            duration_ns(worker_config.lease_ttl),
        )
        .unwrap();
    let mut runner = WorldRunner::open(
        worker,
        Arc::clone(&runtime),
        universe(),
        world(),
        runner_lease,
    )
    .unwrap();
    runner.step_blocking().unwrap();

    let workspace = runtime
        .workspace_projection(universe(), world(), "shell")
        .unwrap()
        .unwrap();
    assert!(workspace.journal_head > 0);
    assert_eq!(workspace.latest_version, 2);
    assert_eq!(workspace.versions.get(&1).unwrap().root_hash, root_v1);
    assert_eq!(workspace.versions.get(&2).unwrap().root_hash, root_v2);
    assert_eq!(workspace.versions.get(&2).unwrap().owner, "bob");
}

#[test]
fn memory_supervisor_snapshots_and_compacts_when_maintenance_is_due() {
    let runtime = Arc::new(MemoryWorldPersistence::with_config(PersistenceConfig {
        snapshot_maintenance: SnapshotMaintenanceConfig {
            snapshot_after_journal_entries: 1,
            segment_target_entries: 1,
            segment_hot_tail_margin: 0,
            segment_delete_chunk_entries: 16,
        },
        ..PersistenceConfig::default()
    }));
    seed_hosted_world(
        Arc::clone(&runtime),
        universe(),
        world(),
        simple_state_manifest(&hosted_store(Arc::clone(&runtime), universe())),
    )
    .unwrap();
    enqueue_simple_start(&runtime, universe(), world(), "wf-maintenance").unwrap();

    let worker = FdbWorker::new(test_worker_config());
    let mut supervisor = worker.with_runtime_for_universes(Arc::clone(&runtime), [universe()]);
    for _ in 0..48 {
        supervisor.run_once_blocking().unwrap();
        let info = runtime
            .world_runtime_info(universe(), world(), now_wallclock_ns())
            .unwrap();
        let baseline = runtime
            .snapshot_active_baseline(universe(), world())
            .unwrap();
        let segments = runtime
            .segment_index_read_from(universe(), world(), 0, 8)
            .unwrap();
        let state = state_bytes(
            Arc::clone(&runtime),
            universe(),
            world(),
            "com.acme/Simple@1",
        )
        .unwrap();
        if state == Some(vec![0xAA])
            && baseline.height > 0
            && !segments.is_empty()
            && !info.has_pending_maintenance
            && supervisor.active_worlds().is_empty()
        {
            return;
        }
    }

    let info = runtime
        .world_runtime_info(universe(), world(), now_wallclock_ns())
        .unwrap();
    let baseline = runtime
        .snapshot_active_baseline(universe(), world())
        .unwrap();
    let segments = runtime
        .segment_index_read_from(universe(), world(), 0, 8)
        .unwrap();
    let state = state_bytes(
        Arc::clone(&runtime),
        universe(),
        world(),
        "com.acme/Simple@1",
    )
    .unwrap();
    panic!(
        "maintenance condition not reached: state={state:?} baseline={} segments={} maintenance={} active_worlds={:?}",
        baseline.height,
        segments.len(),
        info.has_pending_maintenance,
        supervisor.active_worlds()
    );
}

#[test]
fn memory_supervisor_delays_maintenance_until_world_has_been_idle_long_enough() {
    let runtime = Arc::new(MemoryWorldPersistence::with_config(PersistenceConfig {
        snapshot_maintenance: SnapshotMaintenanceConfig {
            snapshot_after_journal_entries: 1,
            segment_target_entries: 1,
            segment_hot_tail_margin: 0,
            segment_delete_chunk_entries: 16,
        },
        ..PersistenceConfig::default()
    }));
    seed_hosted_world(
        Arc::clone(&runtime),
        universe(),
        world(),
        simple_state_manifest(&hosted_store(Arc::clone(&runtime), universe())),
    )
    .unwrap();
    enqueue_simple_start(&runtime, universe(), world(), "wf-maint-delay").unwrap();

    let mut worker_config = test_worker_config();
    worker_config.maintenance_idle_after = Duration::from_millis(50);
    let worker = FdbWorker::new(worker_config);
    let mut supervisor = worker.with_runtime_for_universes(Arc::clone(&runtime), [universe()]);

    supervisor.run_once_blocking().unwrap();

    let info = runtime
        .world_runtime_info(universe(), world(), now_wallclock_ns())
        .unwrap();
    let segments = runtime
        .segment_index_read_from(universe(), world(), 0, 8)
        .unwrap();
    let state = state_bytes(
        Arc::clone(&runtime),
        universe(),
        world(),
        "com.acme/Simple@1",
    )
    .unwrap();
    assert_eq!(state, Some(vec![0xAA]));
    assert!(info.has_pending_maintenance);
    assert!(segments.is_empty());
    assert!(!supervisor.active_worlds().is_empty());

    std::thread::sleep(Duration::from_millis(60));

    run_supervisor_until(&mut supervisor, 16, |supervisor| {
        let info = runtime.world_runtime_info(universe(), world(), now_wallclock_ns())?;
        let segments = runtime.segment_index_read_from(universe(), world(), 0, 8)?;
        Ok(!info.has_pending_maintenance
            && !segments.is_empty()
            && supervisor.active_worlds().is_empty())
    })
    .unwrap();
}

#[test]
fn memory_supervisor_skips_world_without_active_baseline() {
    let runtime = memory_runtime();
    enqueue_simple_start(&runtime, universe(), world(), "wf-2").unwrap();

    let worker = FdbWorker::new(test_worker_config());
    let mut supervisor = worker.with_runtime_for_universes(Arc::clone(&runtime), [universe()]);
    let outcome = supervisor.run_once_blocking().unwrap();

    assert_eq!(outcome.worlds_started, 0);
    assert!(supervisor.active_worlds().is_empty());
    let cursor = runtime.inbox_cursor(universe(), world()).unwrap();
    let pending = runtime
        .inbox_read_after(universe(), world(), cursor, 8)
        .unwrap();
    assert_eq!(pending.len(), 1);
}

#[test]
fn memory_supervisor_respects_ineligible_pins() {
    let runtime = memory_runtime();
    seed_hosted_world(
        Arc::clone(&runtime),
        universe(),
        world(),
        simple_state_manifest(&hosted_store(Arc::clone(&runtime), universe())),
    )
    .unwrap();
    runtime
        .set_world_placement_pin(universe(), world(), Some("gpu".to_string()))
        .unwrap();
    enqueue_simple_start(&runtime, universe(), world(), "wf-3").unwrap();

    let worker = FdbWorker::new(test_worker_config());
    let mut supervisor = worker.with_runtime_for_universes(Arc::clone(&runtime), [universe()]);
    let outcome = supervisor.run_once_blocking().unwrap();

    assert_eq!(outcome.worlds_started, 0);
    assert!(supervisor.active_worlds().is_empty());
    let cursor = runtime.inbox_cursor(universe(), world()).unwrap();
    let pending = runtime
        .inbox_read_after(universe(), world(), cursor, 8)
        .unwrap();
    assert_eq!(pending.len(), 1);
}

#[test]
fn memory_supervisor_pausing_world_transitions_to_paused_and_releases() {
    let runtime = memory_runtime();
    seed_hosted_world(
        Arc::clone(&runtime),
        universe(),
        world(),
        workflow_receipt_manifest(&hosted_store(Arc::clone(&runtime), universe())),
    )
    .unwrap();
    enqueue_start_event(
        &runtime,
        universe(),
        world(),
        "com.acme/WorkflowEvent@1",
        "wf-pause",
    )
    .unwrap();

    let worker = FdbWorker::new(test_worker_config());
    let mut supervisor = worker.with_runtime_for_universes(Arc::clone(&runtime), [universe()]);

    let first = supervisor.run_once_blocking().unwrap();
    assert_eq!(first.worlds_started, 1);
    assert!(!supervisor.active_worlds().is_empty());

    runtime
        .set_world_admin_lifecycle(
            universe(),
            world(),
            aos_fdb::WorldAdminLifecycle {
                status: aos_fdb::WorldAdminStatus::Pausing,
                updated_at_ns: now_wallclock_ns(),
                operation_id: Some("pause-op".into()),
                reason: Some("pause test".into()),
            },
        )
        .unwrap();

    run_supervisor_until(&mut supervisor, 16, |supervisor| {
        let info = runtime
            .world_runtime_info(universe(), world(), now_wallclock_ns())
            .unwrap();
        Ok::<_, WorkerError>(
            info.meta.admin.status == aos_fdb::WorldAdminStatus::Paused
                && info.lease.is_none()
                && supervisor.active_worlds().is_empty(),
        )
    })
    .unwrap();
}

#[test]
fn memory_supervisor_considers_only_ready_worlds() {
    let worker = FdbWorker::new(test_worker_config());
    let runtime = memory_runtime();
    let supervisor = worker.with_runtime_for_universes(runtime, [universe()]);
    let base = WorldRuntimeInfo {
        world_id: world(),
        meta: aos_fdb::WorldMeta {
            handle: aos_fdb::default_world_handle(world()),
            manifest_hash: None,
            active_baseline_height: Some(0),
            placement_pin: None,
            created_at_ns: 0,
            lineage: None,
            admin: aos_fdb::WorldAdminLifecycle::default(),
        },
        notify_counter: 0,
        has_pending_inbox: false,
        has_pending_effects: false,
        next_timer_due_at_ns: None,
        has_pending_maintenance: false,
        lease: None,
    };

    let world_ref = ActiveWorldRef {
        universe_id: universe(),
        world_id: world(),
    };

    assert!(!supervisor.should_consider_world(world_ref, &base, 100));
    assert!(supervisor.should_consider_world(
        world_ref,
        &WorldRuntimeInfo {
            has_pending_inbox: true,
            ..base.clone()
        },
        100
    ));
    assert!(supervisor.should_consider_world(
        world_ref,
        &WorldRuntimeInfo {
            has_pending_effects: true,
            ..base.clone()
        },
        100
    ));
    assert!(supervisor.should_consider_world(
        world_ref,
        &WorldRuntimeInfo {
            next_timer_due_at_ns: Some(99),
            ..base.clone()
        },
        100
    ));
    assert!(supervisor.should_consider_world(
        world_ref,
        &WorldRuntimeInfo {
            has_pending_maintenance: true,
            ..base.clone()
        },
        100
    ));
}

#[test]
fn memory_supervisor_releases_active_world_when_pin_changes() {
    let runtime = memory_runtime();
    seed_hosted_world(
        Arc::clone(&runtime),
        universe(),
        world(),
        simple_state_manifest(&hosted_store(Arc::clone(&runtime), universe())),
    )
    .unwrap();
    enqueue_simple_start(&runtime, universe(), world(), "wf-4").unwrap();
    let mut cfg = test_worker_config();
    cfg.idle_release_after = Duration::from_secs(60);
    let worker = FdbWorker::new(cfg);
    let mut supervisor = worker.with_runtime_for_universes(Arc::clone(&runtime), [universe()]);

    let first = supervisor.run_once_blocking().unwrap();
    assert_eq!(first.worlds_started, 1);
    assert_eq!(
        supervisor.active_worlds(),
        vec![ActiveWorldRef {
            universe_id: universe(),
            world_id: world(),
        }]
    );

    runtime
        .set_world_placement_pin(universe(), world(), Some("gpu".to_string()))
        .unwrap();
    let second = supervisor.run_once_blocking().unwrap();

    assert_eq!(second.worlds_released, 1);
    assert!(supervisor.active_worlds().is_empty());
}

#[test]
fn runner_encodes_domain_event_inbox_items_from_inline_and_cas_payloads() {
    let runtime = memory_runtime();
    let mut runner = build_runner(
        Arc::clone(&runtime),
        universe(),
        world(),
        simple_state_manifest(&hosted_store(Arc::clone(&runtime), universe())),
        test_worker_config(),
    )
    .unwrap();
    let external_payload = serde_cbor::to_vec(&fixtures::start_event("wf-cas")).unwrap();
    let hash = runtime
        .cas_put_verified(universe(), &external_payload)
        .unwrap();

    let inline = runner
        .encode_inbox_item_as_journal_entry(
            0,
            InboxItem::DomainEvent(DomainEventIngress {
                schema: START_SCHEMA.into(),
                value: CborPayload::inline(
                    serde_cbor::to_vec(&fixtures::start_event("wf-inline")).unwrap(),
                ),
                key: Some(vec![1, 2, 3]),
                correlation_id: None,
            }),
        )
        .unwrap();
    let external = runner
        .encode_inbox_item_as_journal_entry(
            1,
            InboxItem::DomainEvent(DomainEventIngress {
                schema: START_SCHEMA.into(),
                value: CborPayload::externalized(hash, external_payload.len() as u64),
                key: None,
                correlation_id: None,
            }),
        )
        .unwrap();

    let inline_record = decode_record(&decode_owned_entry(&inline));
    let external_record = decode_record(&decode_owned_entry(&external));
    match inline_record {
        JournalRecord::DomainEvent(record) => {
            assert_eq!(record.schema, START_SCHEMA);
            assert_eq!(
                record.value,
                serde_cbor::to_vec(&fixtures::start_event("wf-inline")).unwrap()
            );
            assert_eq!(record.key, Some(vec![1, 2, 3]));
        }
        other => panic!("expected domain event record, got {other:?}"),
    }
    match external_record {
        JournalRecord::DomainEvent(record) => {
            assert_eq!(record.schema, START_SCHEMA);
            assert_eq!(record.value, external_payload);
        }
        other => panic!("expected domain event record, got {other:?}"),
    }
}

#[test]
fn runner_encodes_receipt_inbox_items_from_inline_and_cas_payloads() {
    let runtime = memory_runtime();
    let mut runner = build_runner(
        Arc::clone(&runtime),
        universe(),
        world(),
        simple_state_manifest(&hosted_store(Arc::clone(&runtime), universe())),
        test_worker_config(),
    )
    .unwrap();
    let external_payload = serde_cbor::to_vec(&serde_json::json!({ "ok": true })).unwrap();
    let hash = runtime
        .cas_put_verified(universe(), &external_payload)
        .unwrap();
    let intent_hash = vec![7; 32];

    let inline = runner
        .encode_inbox_item_as_journal_entry(
            0,
            InboxItem::Receipt(ReceiptIngress {
                intent_hash: intent_hash.clone(),
                effect_kind: "http.request".into(),
                adapter_id: "stub.http".into(),
                status: ReceiptStatus::Ok,
                payload: CborPayload::inline(vec![1, 2, 3]),
                cost_cents: Some(0),
                signature: vec![9; 64],
                correlation_id: None,
            }),
        )
        .unwrap();
    let external = runner
        .encode_inbox_item_as_journal_entry(
            1,
            InboxItem::Receipt(ReceiptIngress {
                intent_hash: intent_hash.clone(),
                effect_kind: "http.request".into(),
                adapter_id: "stub.http".into(),
                status: ReceiptStatus::Ok,
                payload: CborPayload::externalized(hash, external_payload.len() as u64),
                cost_cents: Some(1),
                signature: vec![8; 64],
                correlation_id: None,
            }),
        )
        .unwrap();

    match decode_record(&decode_owned_entry(&inline)) {
        JournalRecord::EffectReceipt(record) => {
            assert_eq!(record.intent_hash, [7; 32]);
            assert_eq!(record.payload_cbor, vec![1, 2, 3]);
            assert_eq!(record.cost_cents, Some(0));
        }
        other => panic!("expected effect receipt record, got {other:?}"),
    }
    match decode_record(&decode_owned_entry(&external)) {
        JournalRecord::EffectReceipt(record) => {
            assert_eq!(record.payload_cbor, external_payload);
            assert_eq!(record.cost_cents, Some(1));
        }
        other => panic!("expected effect receipt record, got {other:?}"),
    }
}

#[test]
fn runner_encodes_timer_fired_and_rejects_unsupported_inbox_items() {
    let runtime = memory_runtime();
    let mut runner = build_runner(
        Arc::clone(&runtime),
        universe(),
        world(),
        simple_state_manifest(&hosted_store(Arc::clone(&runtime), universe())),
        test_worker_config(),
    )
    .unwrap();

    let timer = runner
        .encode_inbox_item_as_journal_entry(
            0,
            InboxItem::TimerFired(TimerFiredIngress {
                timer_id: "timer-1".into(),
                payload: CborPayload::inline(vec![1]),
                correlation_id: None,
            }),
        )
        .unwrap();
    let inbox = runner
        .encode_inbox_item_as_journal_entry(
            0,
            InboxItem::Inbox(ExternalInboxIngress {
                inbox_name: "demo".into(),
                payload: CborPayload::inline(vec![1]),
                headers: Default::default(),
                correlation_id: None,
            }),
        )
        .unwrap_err();
    let control = runner
        .encode_inbox_item_as_journal_entry(
            0,
            InboxItem::Control(CommandIngress {
                command_id: "cmd-test".into(),
                command: "event-send".into(),
                actor: None,
                payload: CborPayload::inline(vec![1]),
                submitted_at_ns: 0,
            }),
        )
        .unwrap_err();

    match decode_record(&decode_owned_entry(&timer)) {
        JournalRecord::DomainEvent(record) => {
            assert_eq!(record.schema, SYS_TIMER_FIRED_SCHEMA);
            assert_eq!(record.value, vec![1]);
        }
        other => panic!("expected timer_fired domain event record, got {other:?}"),
    }
    assert!(matches!(inbox, WorkerError::UnsupportedInboxItem("inbox")));
    assert!(matches!(
        control,
        WorkerError::UnsupportedInboxItem("control")
    ));
}

#[test]
fn runner_executes_pause_command_and_updates_command_record() {
    let runtime = memory_runtime();
    let mut runner = build_runner(
        Arc::clone(&runtime),
        universe(),
        world(),
        simple_state_manifest(&hosted_store(Arc::clone(&runtime), universe())),
        test_worker_config(),
    )
    .unwrap();

    submit_control_command(
        &runtime,
        universe(),
        world(),
        "cmd-pause",
        CMD_WORLD_PAUSE,
        serde_cbor::to_vec(&LifecycleCommandParams {
            reason: Some("maintenance".into()),
        })
        .unwrap(),
        7,
    )
    .unwrap();

    let _ = runner.step_blocking().unwrap();

    let record = runtime
        .command_record(universe(), world(), "cmd-pause")
        .unwrap()
        .unwrap();
    assert_eq!(record.status, CommandStatus::Succeeded);
    assert!(record.finished_at_ns.is_some());
    let lifecycle: WorldAdminLifecycle =
        serde_cbor::from_slice(record.result_payload.unwrap().inline_cbor.as_ref().unwrap())
            .unwrap();
    assert_eq!(lifecycle.status, WorldAdminStatus::Pausing);
    assert_eq!(
        runtime
            .world_runtime_info(universe(), world(), now_wallclock_ns())
            .unwrap()
            .meta
            .admin
            .status,
        WorldAdminStatus::Paused
    );
}

#[test]
fn runner_executes_governance_propose_command() {
    let runtime = memory_runtime();
    let mut runner = build_runner(
        Arc::clone(&runtime),
        universe(),
        world(),
        simple_state_manifest(&hosted_store(Arc::clone(&runtime), universe())),
        test_worker_config(),
    )
    .unwrap();

    let patch = ManifestPatch {
        manifest: runner
            .host
            .kernel()
            .get_manifest(Consistency::Head)
            .unwrap()
            .value,
        nodes: Vec::new(),
    };
    submit_control_command(
        &runtime,
        universe(),
        world(),
        "cmd-gov-propose",
        CMD_GOV_PROPOSE,
        serde_cbor::to_vec(&GovProposeParams {
            patch: GovPatchInput::PatchCbor(serde_cbor::to_vec(&patch).unwrap()),
            summary: None,
            manifest_base: None,
            description: Some("test proposal".into()),
        })
        .unwrap(),
        9,
    )
    .unwrap();

    let _ = runner.step_blocking().unwrap();

    let record = runtime
        .command_record(universe(), world(), "cmd-gov-propose")
        .unwrap()
        .unwrap();
    assert_eq!(record.status, CommandStatus::Succeeded, "{record:?}");
    let receipt: GovProposeReceipt =
        serde_cbor::from_slice(record.result_payload.unwrap().inline_cbor.as_ref().unwrap())
            .unwrap();
    assert_eq!(receipt.proposal_id, 0);
    assert_eq!(runner.host.kernel().governance().proposals().len(), 1);
}

#[test]
fn runner_publishes_http_effects_and_enqueues_receipts_once() {
    let runtime = memory_runtime();
    let mut runner = build_runner(
        Arc::clone(&runtime),
        universe(),
        world(),
        workflow_receipt_manifest(&hosted_store(Arc::clone(&runtime), universe())),
        test_worker_config(),
    )
    .unwrap();
    enqueue_start_event(
        &runtime,
        universe(),
        world(),
        "com.acme/WorkflowEvent@1",
        "wf-http",
    )
    .unwrap();

    runner.drain_inbox_to_journal().unwrap();
    runner.host.drain().unwrap();
    let (effects, timers) = runner.publish_effects_and_timers().unwrap();
    assert_eq!((effects, timers), (1, 0));
    assert!(
        runtime
            .world_runtime_info(universe(), world(), now_wallclock_ns())
            .unwrap()
            .has_pending_effects
    );

    let claimed = runner.execute_claimed_effects_blocking().unwrap();
    assert_eq!(claimed, 1);
    let after_cursor = runtime.inbox_cursor(universe(), world()).unwrap();
    let receipts = runtime
        .inbox_read_after(universe(), world(), after_cursor, 8)
        .unwrap();
    assert_eq!(receipts.len(), 1);
    assert!(matches!(receipts[0].1, InboxItem::Receipt(_)));
    assert_eq!(runner.publish_effects_and_timers().unwrap(), (0, 0));
}

#[test]
fn runner_publishes_due_timers_and_enqueues_receipts_once() {
    let runtime = memory_runtime();
    let mut runner = build_runner(
        Arc::clone(&runtime),
        universe(),
        world(),
        timer_receipt_workflow_manifest(&hosted_store(Arc::clone(&runtime), universe())),
        test_worker_config(),
    )
    .unwrap();
    enqueue_start_event(
        &runtime,
        universe(),
        world(),
        "com.acme/TimerWorkflowEvent@1",
        "wf-timer",
    )
    .unwrap();

    runner.drain_inbox_to_journal().unwrap();
    runner.host.drain().unwrap();
    let (effects, timers) = runner.publish_effects_and_timers().unwrap();
    assert_eq!((effects, timers), (0, 1));
    assert!(
        runtime
            .world_runtime_info(universe(), world(), now_wallclock_ns())
            .unwrap()
            .next_timer_due_at_ns
            .is_some()
    );

    let fired = runner.fire_due_timers().unwrap();
    assert_eq!(fired, 1);
    let after_cursor = runtime.inbox_cursor(universe(), world()).unwrap();
    let receipts = runtime
        .inbox_read_after(universe(), world(), after_cursor, 8)
        .unwrap();
    assert_eq!(receipts.len(), 1);
    assert!(matches!(receipts[0].1, InboxItem::Receipt(_)));
    assert!(
        runtime
            .world_runtime_info(universe(), world(), now_wallclock_ns())
            .unwrap()
            .next_timer_due_at_ns
            .is_none()
    );
    assert_eq!(runner.publish_effects_and_timers().unwrap(), (0, 0));
}

#[test]
fn runner_reconciliation_prunes_orphan_persisted_effect_dispatches() {
    let runtime = memory_runtime();
    let mut runner = build_runner(
        Arc::clone(&runtime),
        universe(),
        world(),
        workflow_receipt_manifest(&hosted_store(Arc::clone(&runtime), universe())),
        test_worker_config(),
    )
    .unwrap();
    let orphan_hash = [0x6c; 32];
    let dispatch = EffectDispatchItem {
        shard: shard_for_hash(&orphan_hash, runner.worker.config.shard_count),
        universe_id: universe(),
        world_id: world(),
        intent_hash: orphan_hash.to_vec(),
        effect_kind: "llm.generate".into(),
        cap_name: "llm".into(),
        params_inline_cbor: Some(vec![0xa0]),
        params_ref: None,
        params_size: None,
        params_sha256: None,
        idempotency_key: [0x42; 32].to_vec(),
        origin_name: "orphan".into(),
        policy_context_hash: None,
        enqueued_at_ns: now_wallclock_ns(),
    };
    runtime
        .publish_effect_dispatches_guarded(
            universe(),
            world(),
            runner.current_lease().unwrap(),
            now_wallclock_ns(),
            &[dispatch],
        )
        .unwrap();
    assert!(
        runtime
            .world_runtime_info(universe(), world(), now_wallclock_ns())
            .unwrap()
            .has_pending_effects
    );

    let (dropped_receipts, dropped_intents) = runner.reconcile_workflow_runtime_waits().unwrap();
    assert_eq!((dropped_receipts, dropped_intents), (0, 0));
    assert!(
        !runtime
            .world_runtime_info(universe(), world(), now_wallclock_ns())
            .unwrap()
            .has_pending_effects
    );
}

#[test]
fn runner_execute_claimed_effects_discards_orphan_dispatch_without_receipt() {
    let runtime = memory_runtime();
    let mut runner = build_runner(
        Arc::clone(&runtime),
        universe(),
        world(),
        workflow_receipt_manifest(&hosted_store(Arc::clone(&runtime), universe())),
        test_worker_config(),
    )
    .unwrap();
    let orphan_hash = [0x7d; 32];
    let dispatch = EffectDispatchItem {
        shard: shard_for_hash(&orphan_hash, runner.worker.config.shard_count),
        universe_id: universe(),
        world_id: world(),
        intent_hash: orphan_hash.to_vec(),
        effect_kind: "llm.generate".into(),
        cap_name: "llm".into(),
        params_inline_cbor: Some(vec![0xa0]),
        params_ref: None,
        params_size: None,
        params_sha256: None,
        idempotency_key: [0x24; 32].to_vec(),
        origin_name: "orphan".into(),
        policy_context_hash: None,
        enqueued_at_ns: now_wallclock_ns(),
    };
    runtime
        .publish_effect_dispatches_guarded(
            universe(),
            world(),
            runner.current_lease().unwrap(),
            now_wallclock_ns(),
            &[dispatch],
        )
        .unwrap();

    let claimed = runner.execute_claimed_effects_blocking().unwrap();
    assert_eq!(claimed, 0);
    let after_cursor = runtime.inbox_cursor(universe(), world()).unwrap();
    let receipts = runtime
        .inbox_read_after(universe(), world(), after_cursor, 8)
        .unwrap();
    assert!(receipts.is_empty());
    assert!(
        !runtime
            .world_runtime_info(universe(), world(), now_wallclock_ns())
            .unwrap()
            .has_pending_effects
    );
}

#[test]
fn runner_reopen_after_drain_before_effect_publish_matches_reference_and_completes() {
    let runtime_ref = memory_runtime();
    let runtime_crash = memory_runtime();
    let mut reference = build_runner(
        Arc::clone(&runtime_ref),
        universe(),
        world(),
        workflow_receipt_manifest(&hosted_store(Arc::clone(&runtime_ref), universe())),
        test_worker_config(),
    )
    .unwrap();
    let mut crashed = build_runner(
        Arc::clone(&runtime_crash),
        universe(),
        world(),
        workflow_receipt_manifest(&hosted_store(Arc::clone(&runtime_crash), universe())),
        test_worker_config(),
    )
    .unwrap();
    enqueue_start_event(
        &runtime_ref,
        universe(),
        world(),
        "com.acme/WorkflowEvent@1",
        "wf-prepublish-ref",
    )
    .unwrap();
    enqueue_start_event(
        &runtime_crash,
        universe(),
        world(),
        "com.acme/WorkflowEvent@1",
        "wf-prepublish-crash",
    )
    .unwrap();

    reference.drain_inbox_to_journal().unwrap();
    reference.host.drain().unwrap();
    crashed.drain_inbox_to_journal().unwrap();
    crashed.host.drain().unwrap();

    let expected = runtime_fingerprint(&reference.host, "com.acme/Workflow@1");
    drop(crashed);

    let reopened =
        reopen_host_from_runtime(Arc::clone(&runtime_crash), universe(), world()).unwrap();
    assert_eq!(
        runtime_fingerprint(&reopened, "com.acme/Workflow@1"),
        expected
    );

    let worker = FdbWorker::new(test_worker_config());
    let mut supervisor =
        worker.with_runtime_for_universes(Arc::clone(&runtime_crash), [universe()]);
    run_supervisor_until(&mut supervisor, 12, |supervisor| {
        Ok(state_bytes(
            Arc::clone(&runtime_crash),
            universe(),
            world(),
            "com.acme/Workflow@1",
        )? == Some(vec![0x02])
            && state_bytes(
                Arc::clone(&runtime_crash),
                universe(),
                world(),
                "com.acme/ResultWorkflow@1",
            )? == Some(vec![0xEE])
            && supervisor.active_worlds().is_empty())
    })
    .unwrap();
}

#[test]
fn runner_reopen_after_drain_before_timer_publish_matches_reference_and_completes() {
    let runtime_ref = memory_runtime();
    let runtime_crash = memory_runtime();
    let mut reference = build_runner(
        Arc::clone(&runtime_ref),
        universe(),
        world(),
        timer_receipt_workflow_manifest(&hosted_store(Arc::clone(&runtime_ref), universe())),
        test_worker_config(),
    )
    .unwrap();
    let mut crashed = build_runner(
        Arc::clone(&runtime_crash),
        universe(),
        world(),
        timer_receipt_workflow_manifest(&hosted_store(Arc::clone(&runtime_crash), universe())),
        test_worker_config(),
    )
    .unwrap();
    enqueue_start_event(
        &runtime_ref,
        universe(),
        world(),
        "com.acme/TimerWorkflowEvent@1",
        "wf-prepublish-timer-ref",
    )
    .unwrap();
    enqueue_start_event(
        &runtime_crash,
        universe(),
        world(),
        "com.acme/TimerWorkflowEvent@1",
        "wf-prepublish-timer-crash",
    )
    .unwrap();

    reference.drain_inbox_to_journal().unwrap();
    reference.host.drain().unwrap();
    crashed.drain_inbox_to_journal().unwrap();
    crashed.host.drain().unwrap();

    let expected = runtime_fingerprint(&reference.host, "com.acme/TimerWorkflow@1");
    drop(crashed);

    let reopened =
        reopen_host_from_runtime(Arc::clone(&runtime_crash), universe(), world()).unwrap();
    assert_eq!(
        runtime_fingerprint(&reopened, "com.acme/TimerWorkflow@1"),
        expected
    );

    let worker = FdbWorker::new(test_worker_config());
    let mut supervisor =
        worker.with_runtime_for_universes(Arc::clone(&runtime_crash), [universe()]);
    run_supervisor_until(&mut supervisor, 12, |supervisor| {
        Ok(state_bytes(
            Arc::clone(&runtime_crash),
            universe(),
            world(),
            "com.acme/TimerWorkflow@1",
        )? == Some(vec![0xCC])
            && supervisor.active_worlds().is_empty())
    })
    .unwrap();
}

#[test]
fn supervisor_executes_portal_send_and_delivers_destination_event() {
    let runtime = memory_runtime();
    seed_hosted_world(
        Arc::clone(&runtime),
        universe(),
        world(),
        portal_sender_manifest(
            &hosted_store(Arc::clone(&runtime), universe()),
            second_world(),
        ),
    )
    .unwrap();
    seed_hosted_world(
        Arc::clone(&runtime),
        universe(),
        second_world(),
        portal_receiver_manifest(&hosted_store(Arc::clone(&runtime), universe())),
    )
    .unwrap();
    enqueue_start_event(
        &runtime,
        universe(),
        world(),
        "com.acme/PortalWorkflowEvent@1",
        "wf-portal",
    )
    .unwrap();

    let mut supervisor = FdbWorker::new(test_worker_config())
        .with_runtime_for_universes(Arc::clone(&runtime), [universe()]);
    for _ in 0..16 {
        supervisor.run_once_blocking().unwrap();
    }

    assert_eq!(
        state_bytes(
            Arc::clone(&runtime),
            universe(),
            world(),
            "com.acme/PortalWorkflow@1"
        )
        .unwrap(),
        Some(vec![0xC2])
    );
    assert_eq!(
        state_bytes(
            Arc::clone(&runtime),
            universe(),
            second_world(),
            "com.acme/PortalReceiver@1"
        )
        .unwrap(),
        Some(vec![0xDD])
    );
    assert!(supervisor.active_worlds().is_empty());
}

#[test]
fn seed_helper_stores_manifest_graph_and_reopens_hosted_world() {
    let runtime = memory_runtime();
    seed_hosted_world(
        Arc::clone(&runtime),
        universe(),
        second_world(),
        workflow_receipt_manifest(&hosted_store(Arc::clone(&runtime), universe())),
    )
    .unwrap();

    let reopened = state_bytes(
        Arc::clone(&runtime),
        universe(),
        second_world(),
        "com.acme/Workflow@1",
    )
    .unwrap();

    assert_eq!(reopened, None);
}

fn store_full_manifest<S: Store + ?Sized>(
    store: &S,
    loaded: &aos_kernel::manifest::LoadedManifest,
) -> Result<Hash, Box<dyn std::error::Error>> {
    let mut manifest = loaded.manifest.clone();
    patch_named_refs(
        "schema",
        &mut manifest.schemas,
        &store_defs(store, loaded.schemas.values(), AirNode::Defschema)?,
    )?;
    patch_named_refs(
        "module",
        &mut manifest.modules,
        &store_defs(store, loaded.modules.values(), AirNode::Defmodule)?,
    )?;
    patch_named_refs("cap", &mut manifest.caps, &HashMap::new())?;
    patch_named_refs("effect", &mut manifest.effects, &HashMap::new())?;
    patch_named_refs(
        "policy",
        &mut manifest.policies,
        &store_defs(store, loaded.policies.values(), AirNode::Defpolicy)?,
    )?;
    Ok(store.put_node(&AirNode::Manifest(manifest))?)
}

fn store_defs<'a, T, S, F>(
    store: &S,
    defs: impl IntoIterator<Item = &'a T>,
    to_node: F,
) -> Result<HashMap<String, HashRef>, Box<dyn std::error::Error>>
where
    T: Clone + HasName + 'a,
    S: Store + ?Sized,
    F: Fn(T) -> AirNode,
{
    let mut hashes = HashMap::new();
    for def in defs {
        let def = def.clone();
        let hash = store.put_node(&to_node(def.clone()))?;
        hashes.insert(
            def.name().to_string(),
            HashRef::new(hash.to_hex()).expect("hash ref"),
        );
    }
    Ok(hashes)
}

fn patch_named_refs(
    kind: &str,
    refs: &mut Vec<NamedRef>,
    hashes: &HashMap<String, HashRef>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in refs {
        let actual = if let Some(hash) = hashes.get(entry.name.as_str()) {
            hash.clone()
        } else if let Some(builtin) =
            aos_air_types::builtins::find_builtin_schema(entry.name.as_str())
        {
            builtin.hash_ref.clone()
        } else if kind == "effect" {
            aos_air_types::builtins::find_builtin_effect(entry.name.as_str())
                .map(|builtin| builtin.hash_ref.clone())
                .ok_or_else(|| format!("manifest references unknown effect '{}'", entry.name))?
        } else if kind == "module" {
            aos_air_types::builtins::find_builtin_module(entry.name.as_str())
                .map(|builtin| builtin.hash_ref.clone())
                .ok_or_else(|| format!("manifest references unknown module '{}'", entry.name))?
        } else if kind == "cap" {
            aos_air_types::builtins::find_builtin_cap(entry.name.as_str())
                .map(|builtin| builtin.hash_ref.clone())
                .ok_or_else(|| format!("manifest references unknown cap '{}'", entry.name))?
        } else {
            return Err(format!("manifest references unknown {kind} '{}'", entry.name).into());
        };
        entry.hash = actual;
    }
    Ok(())
}

trait HasName {
    fn name(&self) -> &str;
}

impl HasName for DefSchema {
    fn name(&self) -> &str {
        &self.name
    }
}

impl HasName for DefModule {
    fn name(&self) -> &str {
        &self.name
    }
}

impl HasName for DefPolicy {
    fn name(&self) -> &str {
        &self.name
    }
}

fn allow_http_enforcer<S: Store + ?Sized>(store: &Arc<S>) -> DefModule {
    let allow_output = aos_kernel::cap_enforcer::CapCheckOutput {
        constraints_ok: true,
        deny: None,
    };
    let output_bytes = serde_cbor::to_vec(&allow_output).expect("encode cap output");
    let pure_output = PureOutput {
        output: output_bytes,
    };
    fixtures::stub_pure_module(
        store,
        "sys/CapEnforceHttpOut@1",
        &pure_output,
        "sys/CapCheckInput@1",
        "sys/CapCheckOutput@1",
    )
}

fn simple_state_manifest<S: Store + ?Sized>(
    store: &Arc<S>,
) -> aos_kernel::manifest::LoadedManifest {
    let mut workflow = fixtures::stub_workflow_module(
        store,
        "com.acme/Simple@1",
        &WorkflowOutput {
            state: Some(vec![0xAA]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/SimpleState@1"),
        event: fixtures::schema(START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow],
        vec![fixtures::routing_event(START_SCHEMA, "com.acme/Simple@1")],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/SimpleState@1".into(),
                ty: text_type(),
            },
        ],
    );
    loaded
}

fn builtin_schema_ref(name: &str) -> NamedRef {
    let builtin = aos_air_types::builtins::find_builtin_schema(name).expect("builtin schema ref");
    NamedRef {
        name: name.to_string(),
        hash: builtin.hash_ref.clone(),
    }
}

fn builtin_module_ref(name: &str) -> NamedRef {
    let builtin = aos_air_types::builtins::find_builtin_module(name).expect("builtin module ref");
    NamedRef {
        name: name.to_string(),
        hash: builtin.hash_ref.clone(),
    }
}

fn workspace_registry_manifest() -> aos_kernel::manifest::LoadedManifest {
    let mut loaded = fixtures::build_loaded_manifest(
        Vec::new(),
        vec![aos_air_types::RoutingEvent {
            event: SchemaRef::new("sys/WorkspaceCommit@1").expect("workspace commit schema"),
            module: "sys/Workspace@1".into(),
            key_field: Some("workspace".into()),
        }],
    );
    loaded.manifest.schemas.extend([
        builtin_schema_ref("sys/WorkspaceName@1"),
        builtin_schema_ref("sys/WorkspaceCommitMeta@1"),
        builtin_schema_ref("sys/WorkspaceHistory@1"),
        builtin_schema_ref("sys/WorkspaceCommit@1"),
    ]);
    loaded
        .manifest
        .modules
        .push(builtin_module_ref("sys/Workspace@1"));
    loaded
}

fn workspace_history_snapshot_bytes(
    workspace: &str,
    latest: u64,
    versions: &[(u64, &str, &str, u64)],
    manifest_hash: Hash,
    height: u64,
) -> Vec<u8> {
    #[derive(serde::Serialize)]
    struct WorkspaceCommitMeta<'a> {
        root_hash: &'a str,
        owner: &'a str,
        created_at: u64,
    }
    #[derive(serde::Serialize)]
    struct WorkspaceHistory<'a> {
        latest: u64,
        versions: BTreeMap<u64, WorkspaceCommitMeta<'a>>,
    }

    let versions = versions
        .iter()
        .map(|(version, root_hash, owner, created_at)| {
            (
                *version,
                WorkspaceCommitMeta {
                    root_hash,
                    owner,
                    created_at: *created_at,
                },
            )
        })
        .collect();
    let history_bytes = serde_cbor::to_vec(&WorkspaceHistory { latest, versions })
        .expect("encode workspace history");
    let state_hash = *Hash::of_bytes(&history_bytes).as_bytes();
    let mut snapshot = aos_kernel::snapshot::KernelSnapshot::new(
        height,
        vec![aos_kernel::snapshot::WorkflowStateEntry {
            workflow: "sys/Workspace@1".into(),
            key: Some(serde_cbor::to_vec(&workspace.to_string()).expect("workspace key")),
            state: history_bytes.clone(),
            state_hash,
            last_active_ns: 999,
        }],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        height * 10,
        Some(*manifest_hash.as_bytes()),
    );
    snapshot.set_root_completeness(aos_kernel::snapshot::SnapshotRootCompleteness {
        manifest_hash: Some(manifest_hash.as_bytes().to_vec()),
        workflow_state_roots: vec![state_hash],
        cell_index_roots: Vec::new(),
        workspace_roots: Vec::new(),
        pinned_roots: Vec::new(),
    });
    serde_cbor::to_vec(&snapshot).expect("encode workspace snapshot")
}

fn build_loaded_manifest_with_http_enforcer<S: Store + ?Sized>(
    store: &Arc<S>,
    mut modules: Vec<DefModule>,
    routing: Vec<aos_air_types::RoutingEvent>,
) -> aos_kernel::manifest::LoadedManifest {
    if !modules
        .iter()
        .any(|module| module.name == "sys/CapEnforceHttpOut@1")
    {
        modules.push(allow_http_enforcer(store));
    }
    fixtures::build_loaded_manifest(modules, routing)
}

fn workflow_receipt_manifest<S: Store + ?Sized>(
    store: &Arc<S>,
) -> aos_kernel::manifest::LoadedManifest {
    let start_output = WorkflowOutput {
        state: Some(vec![0x01]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::HTTP_REQUEST,
            serde_cbor::to_vec(&HttpRequestParams {
                method: "GET".into(),
                url: "https://example.com/workflow".into(),
                headers: Default::default(),
                body_ref: None,
            })
            .expect("encode http params"),
            "http",
        )],
        ann: None,
    };
    let receipt_output = WorkflowOutput {
        state: Some(vec![0x02]),
        domain_events: vec![DomainEvent::new(
            "com.acme/WorkflowDone@1".to_string(),
            serde_cbor::to_vec(&serde_json::json!({ "id": "wf-http" }))
                .expect("encode completion event"),
        )],
        effects: vec![],
        ann: None,
    };

    let mut workflow =
        sequenced_workflow_module(store, "com.acme/Workflow@1", &start_output, &receipt_output);
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/WorkflowState@1"),
        event: fixtures::schema("com.acme/WorkflowEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::HTTP_REQUEST.into()],
        cap_slots: Default::default(),
    });

    let mut result_module = fixtures::stub_workflow_module(
        store,
        "com.acme/ResultWorkflow@1",
        &WorkflowOutput {
            state: Some(vec![0xEE]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    result_module.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/ResultState@1"),
        event: fixtures::schema("com.acme/WorkflowDone@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: IndexMap::new(),
    });

    let mut loaded = build_loaded_manifest_with_http_enforcer(
        store,
        vec![workflow, result_module],
        vec![
            fixtures::routing_event("com.acme/WorkflowEvent@1", "com.acme/Workflow@1"),
            fixtures::routing_event("com.acme/WorkflowDone@1", "com.acme/ResultWorkflow@1"),
        ],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/WorkflowEvent@1".into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(START_SCHEMA),
                            }),
                        ),
                        (
                            "Receipt".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("sys/EffectReceiptEnvelope@1"),
                            }),
                        ),
                    ]),
                }),
            },
            def_text_record_schema("com.acme/WorkflowDone@1", vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/WorkflowState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            DefSchema {
                name: "com.acme/ResultState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );
    loaded
        .manifest
        .module_bindings
        .get_mut("com.acme/Workflow@1")
        .expect("workflow binding")
        .slots
        .insert("http".into(), "cap_http".into());
    loaded
}

fn timer_receipt_workflow_manifest<S: Store + ?Sized>(
    store: &Arc<S>,
) -> aos_kernel::manifest::LoadedManifest {
    let start_output = WorkflowOutput {
        state: Some(vec![0x01]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::new(
            aos_effects::EffectKind::TIMER_SET,
            serde_cbor::to_vec(&TimerSetParams {
                deliver_at_ns: 10,
                key: Some("retry".into()),
            })
            .expect("encode timer params"),
        )],
        ann: None,
    };
    let receipt_output = WorkflowOutput {
        state: Some(vec![0xCC]),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };
    let mut workflow = sequenced_workflow_module(
        store,
        "com.acme/TimerWorkflow@1",
        &start_output,
        &receipt_output,
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/TimerWorkflowState@1"),
        event: fixtures::schema("com.acme/TimerWorkflowEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::TIMER_SET.into()],
        cap_slots: Default::default(),
    });
    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow],
        vec![fixtures::routing_event(
            "com.acme/TimerWorkflowEvent@1",
            "com.acme/TimerWorkflow@1",
        )],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/TimerWorkflowEvent@1".into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(START_SCHEMA),
                            }),
                        ),
                        (
                            "Receipt".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("sys/EffectReceiptEnvelope@1"),
                            }),
                        ),
                    ]),
                }),
            },
            DefSchema {
                name: "com.acme/TimerWorkflowState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );
    loaded
}

fn portal_sender_manifest<S: Store + ?Sized>(
    store: &Arc<S>,
    dest_world: WorldId,
) -> aos_kernel::manifest::LoadedManifest {
    let start_output = WorkflowOutput {
        state: Some(vec![0xC1]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::PORTAL_SEND,
            serde_cbor::to_vec(&PortalSendParams {
                dest_universe: None,
                dest_world: dest_world.to_string(),
                mode: PortalSendMode::TypedEvent,
                schema: Some("com.acme/PortalEvent@1".into()),
                value_cbor: Some(
                    serde_cbor::to_vec(&serde_json::json!({ "id": "ported" }))
                        .expect("encode portal event"),
                ),
                inbox: None,
                payload_cbor: None,
                headers: None,
                correlation_id: Some("portal-corr".into()),
            })
            .expect("encode portal params"),
            "portal",
        )],
        ann: None,
    };
    let receipt_output = WorkflowOutput {
        state: Some(vec![0xC2]),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };

    let mut workflow = sequenced_workflow_module(
        store,
        "com.acme/PortalWorkflow@1",
        &start_output,
        &receipt_output,
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/PortalWorkflowState@1"),
        event: fixtures::schema("com.acme/PortalWorkflowEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::PORTAL_SEND.into()],
        cap_slots: Default::default(),
    });

    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow],
        vec![fixtures::routing_event(
            "com.acme/PortalWorkflowEvent@1",
            "com.acme/PortalWorkflow@1",
        )],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/PortalWorkflowEvent@1".into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(START_SCHEMA),
                            }),
                        ),
                        (
                            "Receipt".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("sys/EffectReceiptEnvelope@1"),
                            }),
                        ),
                    ]),
                }),
            },
            DefSchema {
                name: "com.acme/PortalWorkflowState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            def_text_record_schema("com.acme/PortalEvent@1", vec![("id", text_type())]),
        ],
    );
    loaded.manifest.caps.push(NamedRef {
        name: "sys/portal@1".into(),
        hash: fixtures::zero_hash(),
    });
    if let Some(defaults) = loaded.manifest.defaults.as_mut() {
        defaults.cap_grants.push(aos_air_types::CapGrant {
            name: "cap_portal".into(),
            cap: "sys/portal@1".into(),
            params: fixtures::empty_value_literal(),
            expiry_ns: None,
        });
    } else {
        loaded.manifest.defaults = Some(aos_air_types::ManifestDefaults {
            policy: None,
            cap_grants: vec![aos_air_types::CapGrant {
                name: "cap_portal".into(),
                cap: "sys/portal@1".into(),
                params: fixtures::empty_value_literal(),
                expiry_ns: None,
            }],
        });
    }
    loaded
        .manifest
        .module_bindings
        .get_mut("com.acme/PortalWorkflow@1")
        .expect("portal workflow binding")
        .slots
        .insert("portal".into(), "cap_portal".into());
    loaded
}

fn portal_receiver_manifest<S: Store + ?Sized>(
    store: &Arc<S>,
) -> aos_kernel::manifest::LoadedManifest {
    let mut workflow = fixtures::stub_workflow_module(
        store,
        "com.acme/PortalReceiver@1",
        &WorkflowOutput {
            state: Some(vec![0xDD]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/PortalReceiverState@1"),
        event: fixtures::schema("com.acme/PortalEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow],
        vec![fixtures::routing_event(
            "com.acme/PortalEvent@1",
            "com.acme/PortalReceiver@1",
        )],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema("com.acme/PortalEvent@1", vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/PortalReceiverState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );
    loaded
}

fn sequenced_workflow_module<S: Store + ?Sized>(
    store: &Arc<S>,
    name: impl Into<String>,
    first: &WorkflowOutput,
    then: &WorkflowOutput,
) -> DefModule {
    let first_bytes = first.encode().expect("encode first workflow output");
    let then_bytes = then.encode().expect("encode second workflow output");
    let first_literal = first_bytes
        .iter()
        .map(|byte| format!("\\{:02x}", byte))
        .collect::<String>();
    let then_literal = then_bytes
        .iter()
        .map(|byte| format!("\\{:02x}", byte))
        .collect::<String>();
    let first_len = first_bytes.len();
    let then_len = then_bytes.len();
    let second_offset = first_len;
    let heap_start = first_len + then_len;
    let wat = format!(
        r#"(module
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const {heap_start}))
  (data (i32.const 0) "{first_literal}")
  (data (i32.const {second_offset}) "{then_literal}")
  (func (export "alloc") (param i32) (result i32)
    (local $old i32)
    global.get $heap
    local.tee $old
    local.get 0
    i32.add
    global.set $heap
    local.get $old)
  (func $is_receipt_event (param $ptr i32) (param $len i32) (result i32)
    (local $i i32)
    (block $not_found
      (loop $search
        local.get $i
        i32.const 6
        i32.add
        local.get $len
        i32.ge_u
        br_if $not_found

        local.get $ptr
        local.get $i
        i32.add
        i32.load8_u
        i32.const 82
        i32.eq
        if
          local.get $ptr
          local.get $i
          i32.add
          i32.const 1
          i32.add
          i32.load8_u
          i32.const 101
          i32.eq
          if
            local.get $ptr
            local.get $i
            i32.add
            i32.const 2
            i32.add
            i32.load8_u
            i32.const 99
            i32.eq
            if
              local.get $ptr
              local.get $i
              i32.add
              i32.const 3
              i32.add
              i32.load8_u
              i32.const 101
              i32.eq
              if
                local.get $ptr
                local.get $i
                i32.add
                i32.const 4
                i32.add
                i32.load8_u
                i32.const 105
                i32.eq
                if
                  local.get $ptr
                  local.get $i
                  i32.add
                  i32.const 5
                  i32.add
                  i32.load8_u
                  i32.const 112
                  i32.eq
                  if
                    local.get $ptr
                    local.get $i
                    i32.add
                    i32.const 6
                    i32.add
                    i32.load8_u
                    i32.const 116
                    i32.eq
                    if
                      i32.const 1
                      return
                    end
                  end
                end
              end
            end
          end
        end

        local.get $i
        i32.const 1
        i32.add
        local.set $i
        br $search
      )
    )
    i32.const 0
  )
  (func (export "step") (param i32 i32) (result i32 i32)
    local.get 0
    local.get 1
    call $is_receipt_event
    if (result i32 i32)
      (i32.const {second_offset})
      (i32.const {then_len})
    else
      (i32.const 0)
      (i32.const {first_len})
    end)
)"#
    );
    let wasm_bytes = parse_str(&wat).expect("wat compile");
    let wasm_hash = store.put_blob(&wasm_bytes).expect("store wasm");

    DefModule {
        name: name.into(),
        module_kind: aos_air_types::ModuleKind::Workflow,
        wasm_hash: HashRef::new(wasm_hash.to_hex()).expect("hash ref"),
        key_schema: None,
        abi: aos_air_types::ModuleAbi {
            workflow: None,
            pure: None,
        },
    }
}
