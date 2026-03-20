mod common;

#[path = "../../aos-runtime/tests/helpers.rs"]
mod runtime_helpers;

use std::sync::Arc;

use aos_cbor::{Hash, to_canonical_cbor};
use aos_kernel::Store;
use aos_node::control::NodeControl;
use aos_node::{
    CborPayload, CreateWorldRequest, CreateWorldSource, DomainEventIngress, HostedStore,
};
use aos_node_local::{LocalControl, SqliteNodeStore};
use aos_runtime::manifest_loader::store_loaded_manifest;
use aos_sqlite::LocalStatePaths;
use runtime_helpers::{fixtures, simple_state_manifest};

use common::world;

fn copy_manifest_module_blobs(
    source: &Arc<fixtures::TestStore>,
    target: &HostedStore,
    loaded: &aos_kernel::LoadedManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    for module in loaded.modules.values() {
        let hash = Hash::from_hex_str(module.wasm_hash.as_str())?;
        let bytes = source.get_blob(hash)?;
        let stored = target.put_blob(&bytes)?;
        assert_eq!(stored, hash, "copied wasm blob hash mismatch");
    }
    Ok(())
}

#[test]
fn local_control_runs_a_real_world_end_to_end() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let paths = LocalStatePaths::new(temp.path().join(".aos"));
    let control = LocalControl::open_batch(paths.root())?;
    let universe = control.local_universe_id();

    let persistence: Arc<dyn aos_node::WorldStore> =
        Arc::new(SqliteNodeStore::open_with_paths(&paths)?);
    let hosted = HostedStore::new(Arc::clone(&persistence), universe);
    let fixture_store = fixtures::new_mem_store();
    let loaded = simple_state_manifest(&fixture_store);
    copy_manifest_module_blobs(&fixture_store, &hosted, &loaded)?;
    let manifest_hash = store_loaded_manifest(&hosted, &loaded)?;

    let created = control.create_world(
        universe,
        CreateWorldRequest {
            world_id: Some(world()),
            handle: Some("demo".into()),
            placement_pin: None,
            created_at_ns: 123,
            source: CreateWorldSource::Manifest {
                manifest_hash: manifest_hash.to_hex(),
            },
        },
    )?;
    assert_eq!(created.record.world_id, world());

    let seq = control.enqueue_event(
        universe,
        world(),
        DomainEventIngress {
            schema: fixtures::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&fixtures::start_event("e2e-1"))?),
            key: None,
            correlation_id: Some("corr-e2e-1".into()),
        },
    )?;
    assert_eq!(seq.to_string(), "0000000000000000");

    let runtime = control.runtime(universe, world())?;
    assert!(!runtime.has_pending_inbox);
    assert!(!runtime.has_pending_effects);
    assert_eq!(runtime.world_id, world());

    let workers = control.workers(universe, 10)?;
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0].worker_id, LocalControl::WORKER_ID);

    let worker_worlds = control.worker_worlds(universe, LocalControl::WORKER_ID, 10)?;
    assert_eq!(worker_worlds.len(), 1);
    assert_eq!(worker_worlds[0].world_id, world());
    assert_eq!(
        worker_worlds[0]
            .lease
            .as_ref()
            .map(|lease| lease.holder_worker_id.as_str()),
        Some(LocalControl::WORKER_ID)
    );

    let head = control.journal_head(universe, world())?;
    assert!(head.journal_head > 0);
    let manifest = control.manifest(universe, world())?;
    assert_eq!(head.manifest_hash, Some(manifest.manifest_hash.clone()));

    let state = control.state_get(universe, world(), "com.acme/Simple@1", None, None)?;
    assert_eq!(state.workflow, "com.acme/Simple@1");
    assert_eq!(state.state_b64.as_deref(), Some("qg=="));
    assert_eq!(
        state.cell.as_ref().map(|cell| cell.journal_head),
        Some(head.journal_head)
    );

    Ok(())
}
