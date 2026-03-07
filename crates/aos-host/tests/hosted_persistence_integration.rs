#![cfg(feature = "foundationdb-hosted")]

#[path = "helpers.rs"]
mod helpers;

use std::env;
use std::fs;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use aos_air_types::{DefSchema, TypeExpr, TypeRef, TypeVariant, WorkflowAbi};
use aos_fdb::{
    CasConfig, FdbRuntime, FdbWorldPersistence, InboxConfig, PersistenceConfig,
    SegmentExportRequest, SegmentId, UniverseId, WorldId, WorldPersistence,
};
use aos_host::config::HostConfig;
use aos_host::manifest_loader::store_loaded_manifest;
use aos_host::{ExternalEvent, HostedStore, WorldHost};
use aos_kernel::{KernelConfig, StateReader};
use aos_store::Store;
use aos_wasm_abi::{WorkflowEffect, WorkflowOutput};
use helpers::fixtures;
use helpers::{def_text_record_schema, insert_test_schemas, text_type};
use indexmap::IndexMap;
use tempfile::TempDir;
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
            inline_threshold_bytes: 8,
            verify_reads: true,
        },
        inbox: InboxConfig {
            inline_payload_threshold_bytes: 8,
        },
        ..PersistenceConfig::default()
    }
}

fn open_persistence(
    object_store_root: &std::path::Path,
) -> Result<Arc<dyn WorldPersistence>, Box<dyn std::error::Error>> {
    let runtime = runtime()?;
    let persistence = match env::var_os("FDB_CLUSTER_FILE") {
        Some(cluster_file) => FdbWorldPersistence::open(
            runtime,
            Some(PathBuf::from(cluster_file)),
            object_store_root,
            test_config(),
        )?,
        None => FdbWorldPersistence::open_default(runtime, object_store_root, test_config())?,
    };
    Ok(Arc::new(persistence))
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

#[test]
fn hosted_world_reopens_from_persistence_identity_and_restores_queue()
-> Result<(), Box<dyn std::error::Error>> {
    if !cluster_is_reachable() {
        eprintln!(
            "skipping hosted persistence integration test because no local cluster is reachable"
        );
        return Ok(());
    }

    let object_store = TempDir::new()?;
    let persistence = open_persistence(object_store.path())?;
    let universe = UniverseId::from(Uuid::new_v4());
    let world = WorldId::from(Uuid::new_v4());

    let store = Arc::new(HostedStore::new(Arc::clone(&persistence), universe));
    let loaded = build_timer_manifest(&store);
    let manifest_hash = store_loaded_manifest(store.as_ref(), &loaded)?;

    let mut host = WorldHost::open_hosted_from_manifest_hash(
        Arc::clone(&persistence),
        universe,
        world,
        manifest_hash,
        HostConfig::default(),
        KernelConfig::default(),
    )?;

    host.enqueue_external(ExternalEvent::DomainEvent {
        schema: "demo/TimerStart@1".into(),
        value: serde_cbor::to_vec(&serde_json::json!({})).unwrap(),
        key: None,
    })?;
    host.drain()?;
    assert_eq!(host.kernel().queued_effects_snapshot().len(), 1);

    host.snapshot()?;
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

    let reopened = WorldHost::open_hosted(
        Arc::clone(&persistence),
        universe,
        world,
        HostConfig::default(),
        KernelConfig::default(),
    )?;
    assert_eq!(reopened.kernel().queued_effects_snapshot().len(), 2);
    assert_eq!(
        reopened.kernel().get_journal_head().active_baseline_height,
        Some(baseline_height)
    );

    Ok(())
}
