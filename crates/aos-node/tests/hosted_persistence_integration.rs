#![cfg(feature = "foundationdb-hosted")]

#[path = "../../aos-runtime/tests/helpers.rs"]
mod helpers;

use std::env;
use std::fs;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use aos_air_types::{DefSchema, TypeExpr, TypeRef, TypeVariant, WorkflowAbi};
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_fdb::{
    CasConfig, CreateWorldSeedRequest, FdbRuntime, FdbWorldPersistence, ForkPendingEffectPolicy,
    ForkWorldRequest, InboxConfig, PersistenceConfig, SegmentExportRequest, SegmentId,
    SnapshotRecord, SnapshotSelector, UniverseId, WorldAdminStore, WorldId, WorldSeed, WorldStore,
};
use aos_kernel::Store;
use aos_kernel::snapshot::WorkflowStatusSnapshot;
use aos_kernel::{KernelConfig, StateReader};
use aos_node::{
    HostedStore, open_hosted_from_manifest_hash, open_hosted_world, snapshot_hosted_world,
};
use aos_runtime::manifest_loader::store_loaded_manifest;
use aos_runtime::{ExternalEvent, WorldConfig, WorldHost};
use aos_wasm_abi::{WorkflowEffect, WorkflowOutput};
use helpers::fixtures;
use helpers::{def_text_record_schema, insert_test_schemas, text_type};
use indexmap::IndexMap;
use uuid::Uuid;

static RUNTIME: OnceLock<Result<Arc<FdbRuntime>, String>> = OnceLock::new();

fn cluster_is_reachable() -> bool {
    let cluster_file = env::var_os("FDB_CLUSTER_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/local/etc/foundationdb/fdb.cluster"));
    let cluster_line = match fs::read_to_string(&cluster_file) {
        Ok(contents) => contents
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string(),
        Err(_) => return false,
    };
    let Some(coord_part) = cluster_line.split('@').nth(1) else {
        return false;
    };
    let Some(first_coord) = coord_part.split(',').next() else {
        return false;
    };
    let addresses: Vec<SocketAddr> = match first_coord.to_socket_addrs() {
        Ok(addresses) => addresses.collect(),
        Err(_) => return false,
    };
    addresses
        .into_iter()
        .any(|address| TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok())
}

fn runtime() -> Result<Arc<FdbRuntime>, Box<dyn std::error::Error>> {
    let runtime = RUNTIME
        .get_or_init(|| {
            FdbRuntime::boot()
                .map(Arc::new)
                .map_err(|err| err.to_string())
        })
        .as_ref()
        .map_err(|err| err.clone())?;
    Ok(Arc::clone(runtime))
}

fn test_config() -> PersistenceConfig {
    PersistenceConfig {
        cas: CasConfig {
            verify_reads: true,
            ..CasConfig::default()
        },
        inbox: InboxConfig {
            inline_payload_threshold_bytes: 8,
        },
        ..PersistenceConfig::default()
    }
}

fn open_persistence() -> Result<Arc<FdbWorldPersistence>, Box<dyn std::error::Error>> {
    let runtime = runtime()?;
    let persistence = match env::var_os("FDB_CLUSTER_FILE") {
        Some(cluster_file) => {
            FdbWorldPersistence::open(runtime, Some(PathBuf::from(cluster_file)), test_config())?
        }
        None => FdbWorldPersistence::open_default(runtime, test_config())?,
    };
    Ok(Arc::new(persistence))
}

fn hosted_persistence(persistence: &Arc<FdbWorldPersistence>) -> Arc<dyn WorldStore> {
    persistence.clone()
}

fn open_hosted(
    persistence: &Arc<FdbWorldPersistence>,
    universe: UniverseId,
    world: WorldId,
) -> Result<WorldHost<HostedStore>, Box<dyn std::error::Error>> {
    Ok(open_hosted_world(
        hosted_persistence(persistence),
        universe,
        world,
        WorldConfig::default(),
        EffectAdapterConfig::default(),
        KernelConfig::default(),
        None,
    )?)
}

fn open_hosted_from_hash(
    persistence: &Arc<FdbWorldPersistence>,
    universe: UniverseId,
    world: WorldId,
    manifest_hash: aos_cbor::Hash,
) -> Result<WorldHost<HostedStore>, Box<dyn std::error::Error>> {
    Ok(open_hosted_from_manifest_hash(
        hosted_persistence(persistence),
        universe,
        world,
        manifest_hash,
        WorldConfig::default(),
        EffectAdapterConfig::default(),
        KernelConfig::default(),
        None,
    )?)
}

fn snapshot_hosted(
    persistence: &Arc<FdbWorldPersistence>,
    universe: UniverseId,
    world: WorldId,
    host: &mut WorldHost<HostedStore>,
) -> Result<(), Box<dyn std::error::Error>> {
    let persistence: Arc<dyn WorldStore> = persistence.clone();
    snapshot_hosted_world(host, &persistence, universe, world)?;
    Ok(())
}

fn build_timer_manifest<S: Store + ?Sized>(store: &Arc<S>) -> aos_kernel::LoadedManifest {
    let effect = WorkflowEffect::with_cap_slot(
        aos_effects::EffectKind::TIMER_SET,
        serde_cbor::to_vec(&serde_json::json!({
            "deliver_at_ns": 99u64,
            "key": "demo"
        }))
        .unwrap(),
        "default",
    );
    let output = WorkflowOutput {
        state: Some(vec![0xAA]),
        domain_events: vec![],
        effects: vec![effect],
        ann: None,
    };
    let mut module = fixtures::stub_workflow_module(store, "demo/TimerWorkflow@1", &output);
    module.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("demo/TimerState@1"),
        event: fixtures::schema("demo/TimerEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::TIMER_SET.into()],
        cap_slots: Default::default(),
    });
    let mut loaded = fixtures::build_loaded_manifest(
        vec![module],
        vec![fixtures::routing_event(
            "demo/TimerEvent@1",
            "demo/TimerWorkflow@1",
        )],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema("demo/TimerStart@1", vec![]),
            DefSchema {
                name: "demo/TimerEvent@1".into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("demo/TimerStart@1"),
                            }),
                        ),
                        (
                            "Fired".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(fixtures::SYS_TIMER_FIRED),
                            }),
                        ),
                    ]),
                }),
            },
            DefSchema {
                name: "demo/TimerState@1".into(),
                ty: text_type(),
            },
        ],
    );
    loaded
}

fn build_pending_timer_manifest<S: Store + ?Sized>(store: &Arc<S>) -> aos_kernel::LoadedManifest {
    let effect = WorkflowEffect::new(
        aos_effects::EffectKind::TIMER_SET,
        serde_cbor::to_vec(&serde_json::json!({
            "deliver_at_ns": 99u64,
            "key": "fork"
        }))
        .unwrap(),
    );
    let output = WorkflowOutput {
        state: Some(vec![0xAA]),
        domain_events: vec![],
        effects: vec![effect],
        ann: None,
    };
    let mut module = fixtures::stub_workflow_module(store, "demo/ForkWorkflow@1", &output);
    module.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("demo/ForkState@1"),
        event: fixtures::schema(helpers::fixtures::START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::TIMER_SET.into()],
        cap_slots: Default::default(),
    });
    let mut loaded = fixtures::build_loaded_manifest(
        vec![module],
        vec![fixtures::routing_event(
            helpers::fixtures::START_SCHEMA,
            "demo/ForkWorkflow@1",
        )],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(helpers::fixtures::START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "demo/ForkState@1".into(),
                ty: text_type(),
            },
        ],
    );
    loaded
}

#[test]
fn hosted_world_reopens_from_persistence_identity_and_restores_queue()
-> Result<(), Box<dyn std::error::Error>> {
    if !cluster_is_reachable() {
        eprintln!(
            "skipping hosted persistence integration test because no local cluster is reachable"
        );
        return Ok(());
    }

    let persistence = open_persistence()?;
    let universe = UniverseId::from(Uuid::new_v4());
    let world = WorldId::from(Uuid::new_v4());

    let store = Arc::new(HostedStore::new(hosted_persistence(&persistence), universe));
    let loaded = build_timer_manifest(&store);
    let manifest_hash = store_loaded_manifest(store.as_ref(), &loaded)?;

    let mut host = open_hosted_from_hash(&persistence, universe, world, manifest_hash)?;

    host.enqueue_external(ExternalEvent::DomainEvent {
        schema: "demo/TimerStart@1".into(),
        value: serde_cbor::to_vec(&serde_json::json!({})).unwrap(),
        key: None,
    })?;
    host.drain()?;
    assert_eq!(host.kernel().queued_effects_snapshot().len(), 1);

    snapshot_hosted(&persistence, universe, world, &mut host)?;
    let baseline_height = host
        .kernel()
        .get_journal_head()
        .active_baseline_height
        .expect("active baseline height");
    assert_eq!(
        persistence
            .snapshot_active_baseline(universe, world)?
            .height,
        baseline_height
    );

    host.enqueue_external(ExternalEvent::DomainEvent {
        schema: "demo/TimerStart@1".into(),
        value: serde_cbor::to_vec(&serde_json::json!({})).unwrap(),
        key: None,
    })?;
    host.drain()?;
    assert_eq!(host.kernel().queued_effects_snapshot().len(), 2);

    persistence.segment_export(
        universe,
        world,
        SegmentExportRequest {
            segment: SegmentId::new(0, baseline_height.saturating_sub(1))?,
            hot_tail_margin: 0,
            delete_chunk_entries: 2,
        },
    )?;
    drop(host);

    let reopened = open_hosted(&persistence, universe, world)?;
    assert_eq!(reopened.kernel().queued_effects_snapshot().len(), 2);
    assert_eq!(
        reopened.kernel().get_journal_head().active_baseline_height,
        Some(baseline_height)
    );

    Ok(())
}

#[test]
fn hosted_persistence_snapshot_latest_returns_highest_indexed_height()
-> Result<(), Box<dyn std::error::Error>> {
    if !cluster_is_reachable() {
        eprintln!(
            "skipping hosted persistence integration test because no local cluster is reachable"
        );
        return Ok(());
    }

    let persistence = open_persistence()?;
    let universe = UniverseId::from(Uuid::new_v4());
    let world = WorldId::from(Uuid::new_v4());

    for height in [1_u64, 5, 9] {
        persistence.snapshot_index(
            universe,
            world,
            SnapshotRecord {
                snapshot_ref: format!("sha256:{:064x}", height),
                height,
                logical_time_ns: height,
                receipt_horizon_height: None,
                manifest_hash: Some(format!("sha256:{:064x}", height + 100)),
            },
        )?;
    }

    let latest = persistence.snapshot_latest(universe, world)?;
    assert_eq!(latest.height, 9);
    assert_eq!(latest.snapshot_ref, format!("sha256:{:064x}", 9_u64));
    Ok(())
}

#[test]
fn hosted_world_reopens_from_latest_unsafe_snapshot_seed() -> Result<(), Box<dyn std::error::Error>>
{
    if !cluster_is_reachable() {
        eprintln!(
            "skipping hosted persistence integration test because no local cluster is reachable"
        );
        return Ok(());
    }

    let persistence = open_persistence()?;
    let universe = UniverseId::from(Uuid::new_v4());
    let world = WorldId::from(Uuid::new_v4());

    let store = Arc::new(HostedStore::new(hosted_persistence(&persistence), universe));
    let loaded = build_pending_timer_manifest(&store);
    let manifest_hash = store_loaded_manifest(store.as_ref(), &loaded)?;

    let mut host = open_hosted_from_hash(&persistence, universe, world, manifest_hash)?;
    let initial_baseline_height = host
        .kernel()
        .get_journal_head()
        .active_baseline_height
        .expect("active baseline height");

    host.enqueue_external(ExternalEvent::DomainEvent {
        schema: helpers::fixtures::START_SCHEMA.into(),
        value: serde_cbor::to_vec(&helpers::fixtures::start_event("unsafe-seed"))?,
        key: None,
    })?;
    host.drain()?;
    snapshot_hosted(&persistence, universe, world, &mut host)?;

    let unsafe_snapshot_height = host.kernel().heights().snapshot.expect("snapshot height");
    assert!(unsafe_snapshot_height > initial_baseline_height);
    let latest = persistence.snapshot_latest(universe, world)?;
    assert_eq!(latest.height, unsafe_snapshot_height);
    assert_eq!(latest.receipt_horizon_height, Some(unsafe_snapshot_height));
    drop(host);

    let reopened = open_hosted(&persistence, universe, world)?;
    assert_eq!(
        reopened.kernel().heights().snapshot,
        Some(unsafe_snapshot_height)
    );
    assert_eq!(
        reopened.kernel().get_journal_head().active_baseline_height,
        Some(unsafe_snapshot_height)
    );

    Ok(())
}

#[test]
fn hosted_world_fork_clears_pending_external_state_from_baseline_snapshot()
-> Result<(), Box<dyn std::error::Error>> {
    if !cluster_is_reachable() {
        eprintln!(
            "skipping hosted persistence integration test because no local cluster is reachable"
        );
        return Ok(());
    }

    let persistence = open_persistence()?;
    let universe = UniverseId::from(Uuid::new_v4());
    let staging_world = WorldId::from(Uuid::new_v4());
    let src_world = WorldId::from(Uuid::new_v4());
    let fork_world = WorldId::from(Uuid::new_v4());

    let store = Arc::new(HostedStore::new(hosted_persistence(&persistence), universe));
    let loaded = build_pending_timer_manifest(&store);
    let manifest_hash = store_loaded_manifest(store.as_ref(), &loaded)?;

    let mut staging = open_hosted_from_hash(&persistence, universe, staging_world, manifest_hash)?;
    staging.enqueue_external(ExternalEvent::DomainEvent {
        schema: helpers::fixtures::START_SCHEMA.into(),
        value: serde_cbor::to_vec(&helpers::fixtures::start_event("fork-src"))?,
        key: None,
    })?;
    staging.drain()?;
    assert_eq!(staging.state("demo/ForkWorkflow@1", None), Some(vec![0xAA]));
    assert_eq!(staging.kernel().queued_effects_snapshot().len(), 1);
    assert_eq!(
        staging.kernel().pending_workflow_receipts_snapshot().len(),
        1
    );
    assert_eq!(
        staging.kernel().workflow_instances_snapshot()[0].status,
        WorkflowStatusSnapshot::Waiting
    );
    snapshot_hosted(&persistence, universe, staging_world, &mut staging)?;

    let snapshot_height = staging
        .kernel()
        .heights()
        .snapshot
        .expect("snapshot height");
    let baseline = persistence.snapshot_at_height(universe, staging_world, snapshot_height)?;
    assert_eq!(baseline.receipt_horizon_height, Some(baseline.height));

    persistence.world_create_from_seed(
        universe,
        CreateWorldSeedRequest {
            world_id: Some(src_world),
            handle: None,
            seed: WorldSeed {
                baseline: baseline.clone(),
                seed_kind: aos_fdb::SeedKind::Genesis,
                imported_from: None,
            },
            placement_pin: None,
            created_at_ns: 10,
        },
    )?;

    let source = open_hosted(&persistence, universe, src_world)?;
    assert_eq!(source.state("demo/ForkWorkflow@1", None), Some(vec![0xAA]));
    assert_eq!(source.kernel().queued_effects_snapshot().len(), 1);
    assert_eq!(
        source.kernel().pending_workflow_receipts_snapshot().len(),
        1
    );
    assert_eq!(
        source.kernel().workflow_instances_snapshot()[0].status,
        WorkflowStatusSnapshot::Waiting
    );
    assert_eq!(
        source.kernel().workflow_instances_snapshot()[0]
            .inflight_intents
            .len(),
        1
    );

    let fork = persistence.world_fork(
        universe,
        ForkWorldRequest {
            src_world_id: src_world,
            src_snapshot: SnapshotSelector::ActiveBaseline,
            new_world_id: Some(fork_world),
            handle: None,
            placement_pin: None,
            forked_at_ns: 20,
            pending_effect_policy: ForkPendingEffectPolicy::default(),
        },
    )?;
    assert_ne!(
        fork.record.active_baseline.snapshot_ref, baseline.snapshot_ref,
        "fork should rewrite snapshot bytes when clearing pending external state"
    );

    let forked = open_hosted(&persistence, universe, fork_world)?;
    assert_eq!(forked.state("demo/ForkWorkflow@1", None), Some(vec![0xAA]));
    assert!(forked.kernel().queued_effects_snapshot().is_empty());
    assert!(
        forked
            .kernel()
            .pending_workflow_receipts_snapshot()
            .is_empty()
    );
    let forked_instances = forked.kernel().workflow_instances_snapshot();
    assert_eq!(forked_instances.len(), 1);
    assert!(forked_instances[0].inflight_intents.is_empty());
    assert_eq!(forked_instances[0].status, WorkflowStatusSnapshot::Running);

    Ok(())
}
