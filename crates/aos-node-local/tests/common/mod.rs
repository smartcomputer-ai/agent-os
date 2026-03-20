#![allow(dead_code)]

use aos_cbor::Hash;
use aos_kernel::snapshot::KernelSnapshot;
use aos_node::{
    CborPayload, CommandIngress, CommandRecord, CommandStatus, CreateWorldSeedRequest, SeedKind,
    SnapshotRecord, UniverseId, UniverseStore, WorldAdminStore, WorldId, WorldLineage, WorldSeed,
    WorldStore,
};
use aos_node_local::SqliteNodeStore;
use aos_sqlite::LocalStatePaths;
use tempfile::TempDir;
use uuid::Uuid;

pub fn temp_state_root() -> (TempDir, LocalStatePaths) {
    let temp = TempDir::new().expect("create temp dir");
    let paths = LocalStatePaths::new(temp.path().join(".aos"));
    (temp, paths)
}

pub fn open_store(paths: &LocalStatePaths) -> SqliteNodeStore {
    SqliteNodeStore::open_with_paths(paths).expect("open sqlite node store")
}

pub fn universe() -> UniverseId {
    UniverseId::from(Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap())
}

pub fn second_universe() -> UniverseId {
    UniverseId::from(Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap())
}

pub fn world() -> WorldId {
    WorldId::from(Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap())
}

pub fn second_world() -> WorldId {
    WorldId::from(Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap())
}

pub fn third_world() -> WorldId {
    WorldId::from(Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap())
}

pub fn create_universe(
    store: &SqliteNodeStore,
    universe: UniverseId,
    handle: &str,
    created_at_ns: u64,
) {
    let _ = created_at_ns;
    assert_eq!(universe, crate::common::universe());
    store
        .set_universe_handle(universe, handle.into())
        .expect("set singleton universe handle");
}

pub fn kernel_snapshot_bytes(height: u64, manifest_hash: Option<Hash>) -> Vec<u8> {
    serde_cbor::to_vec(&KernelSnapshot::new(
        height,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        height * 10,
        manifest_hash.map(|hash| *hash.as_bytes()),
    ))
    .expect("encode kernel snapshot")
}

pub fn store_snapshot_record(
    store: &SqliteNodeStore,
    universe: UniverseId,
    height: u64,
    manifest_bytes: &[u8],
    snapshot_bytes: &[u8],
) -> SnapshotRecord {
    let manifest_hash = store
        .cas_put_verified(universe, manifest_bytes)
        .expect("store manifest");
    let snapshot_hash = store
        .cas_put_verified(universe, snapshot_bytes)
        .expect("store snapshot");
    SnapshotRecord {
        snapshot_ref: snapshot_hash.to_hex(),
        height,
        logical_time_ns: height * 10,
        receipt_horizon_height: Some(height),
        manifest_hash: Some(manifest_hash.to_hex()),
    }
}

pub fn seed_request(
    store: &SqliteNodeStore,
    universe: UniverseId,
    world_id: WorldId,
    height: u64,
) -> CreateWorldSeedRequest {
    let manifest_bytes = format!("manifest-{height}").into_bytes();
    let snapshot = store_snapshot_record(
        store,
        universe,
        height,
        &manifest_bytes,
        &kernel_snapshot_bytes(height, None),
    );
    CreateWorldSeedRequest {
        world_id: Some(world_id),
        handle: None,
        seed: WorldSeed {
            baseline: snapshot,
            seed_kind: SeedKind::Genesis,
            imported_from: None,
        },
        placement_pin: Some("gpu".into()),
        created_at_ns: 123 + height,
    }
}

pub fn seed_world(
    store: &SqliteNodeStore,
    universe: UniverseId,
    world_id: WorldId,
    height: u64,
) -> aos_node::WorldCreateResult {
    store
        .world_create_from_seed(universe, seed_request(store, universe, world_id, height))
        .expect("seed world")
}

pub fn prepare_bootstrap_world(
    store: &SqliteNodeStore,
    universe: UniverseId,
    world: WorldId,
    handle: &str,
) -> Hash {
    assert_eq!(universe, crate::common::universe());
    let manifest_hash = store
        .cas_put_verified(universe, b"bootstrap-manifest")
        .expect("store manifest");
    store
        .world_prepare_manifest_bootstrap(
            universe,
            world,
            manifest_hash,
            handle.into(),
            None,
            22,
            WorldLineage::Genesis { created_at_ns: 22 },
        )
        .expect("prepare bootstrap world");
    manifest_hash
}

pub fn queued_command(command_id: &str, command: &str, submitted_at_ns: u64) -> CommandRecord {
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

pub fn command_ingress(command_id: &str, payload: Vec<u8>, submitted_at_ns: u64) -> CommandIngress {
    CommandIngress {
        command_id: command_id.into(),
        command: "event-send".into(),
        actor: Some("tester".into()),
        payload: CborPayload::inline(payload),
        submitted_at_ns,
    }
}
