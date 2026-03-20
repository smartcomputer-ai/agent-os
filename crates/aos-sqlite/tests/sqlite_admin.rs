mod common;

use aos_cbor::Hash;
use aos_kernel::snapshot::KernelSnapshot;
use aos_node::{
    CborPayload, CommandIngress, CommandRecord, CommandStatus, CommandStore,
    CreateWorldSeedRequest, ForkPendingEffectPolicy, ForkWorldRequest, NodeCatalog,
    PersistConflict, PersistError, SeedKind, SnapshotRecord, SnapshotSelector, UniverseId,
    UniverseStore, WorldAdminLifecycle, WorldAdminStatus, WorldAdminStore, WorldId, WorldSeed,
    WorldStore,
};
use aos_sqlite::SqliteNodeStore;
use uuid::Uuid;

use common::{open_store, temp_state_root, universe};

fn second_universe() -> UniverseId {
    UniverseId::from(Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap())
}

fn world() -> WorldId {
    WorldId::from(Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap())
}

fn second_world() -> WorldId {
    WorldId::from(Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap())
}

fn third_world() -> WorldId {
    WorldId::from(Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap())
}

fn create_universe(
    store: &SqliteNodeStore,
    universe: UniverseId,
    handle: &str,
    created_at_ns: u64,
) {
    let _ = created_at_ns;
    assert_eq!(universe, common::universe());
    store
        .set_universe_handle(universe, handle.into())
        .expect("set singleton universe handle");
}

fn kernel_snapshot_bytes(height: u64, manifest_hash: Option<Hash>) -> Vec<u8> {
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

fn store_snapshot_record(
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

fn seed_request(
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

fn seed_world(
    store: &SqliteNodeStore,
    universe: UniverseId,
    world_id: WorldId,
    height: u64,
) -> aos_node::WorldCreateResult {
    store
        .world_create_from_seed(universe, seed_request(store, universe, world_id, height))
        .expect("seed world")
}

fn queued_command(command_id: &str, command: &str, submitted_at_ns: u64) -> CommandRecord {
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

fn command_ingress(command_id: &str, payload: Vec<u8>, submitted_at_ns: u64) -> CommandIngress {
    CommandIngress {
        command_id: command_id.into(),
        command: "event-send".into(),
        actor: Some("tester".into()),
        payload: CborPayload::inline(payload),
        submitted_at_ns,
    }
}

#[test]
fn singleton_universe_and_world_handle_collisions_match_local_node_semantics() {
    let (_temp, paths) = temp_state_root();
    let store = open_store(&paths);
    create_universe(&store, universe(), "alpha", 1);

    let err = store
        .create_universe(aos_node::CreateUniverseRequest {
            universe_id: Some(second_universe()),
            handle: Some("beta".into()),
            created_at_ns: 2,
        })
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::UniverseExists { .. })
    ));

    let listed = store.list_universes(None, 10).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].universe_id, universe());
    assert_eq!(
        store.get_universe_by_handle("alpha").unwrap().universe_id,
        universe()
    );
    assert!(matches!(
        store.get_universe(second_universe()),
        Err(PersistError::NotFound(_))
    ));

    seed_world(&store, universe(), world(), 3);
    seed_world(&store, universe(), second_world(), 4);
    store
        .set_world_handle(universe(), world(), "ops".into())
        .unwrap();
    store
        .set_world_handle(universe(), second_world(), "ops-2".into())
        .unwrap();

    let err = store
        .set_world_handle(universe(), second_world(), "ops".into())
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::WorldHandleExists { .. })
    ));
}

#[test]
fn singleton_universe_keeps_world_handle_release_and_archived_world_blocking() {
    let (_temp, paths) = temp_state_root();
    let store = open_store(&paths);
    create_universe(&store, universe(), "alpha", 1);
    assert!(matches!(
        store.delete_universe(universe(), 2),
        Err(PersistError::Validation(_))
    ));

    seed_world(&store, universe(), world(), 4);
    store
        .set_world_handle(universe(), world(), "ops".into())
        .unwrap();
    store
        .set_world_admin_lifecycle(
            universe(),
            world(),
            WorldAdminLifecycle {
                status: WorldAdminStatus::Deleted,
                updated_at_ns: 5,
                operation_id: Some("delete-op".into()),
                reason: Some("cleanup".into()),
            },
        )
        .unwrap();
    assert!(matches!(
        store.world_runtime_info_by_handle(universe(), "ops", 0),
        Err(PersistError::NotFound(_))
    ));
    seed_world(&store, universe(), second_world(), 5);
    store
        .set_world_handle(universe(), second_world(), "ops".into())
        .unwrap();

    seed_world(&store, universe(), third_world(), 6);
    store
        .set_world_admin_lifecycle(
            universe(),
            third_world(),
            WorldAdminLifecycle {
                status: WorldAdminStatus::Archived,
                updated_at_ns: 7,
                operation_id: Some("archive-op".into()),
                reason: Some("archive".into()),
            },
        )
        .unwrap();

    assert!(matches!(
        store.set_world_placement_pin(universe(), third_world(), Some("cpu".into())),
        Err(PersistError::Conflict(
            PersistConflict::WorldAdminBlocked { .. }
        ))
    ));
    assert!(matches!(
        store.inbox_enqueue(
            universe(),
            third_world(),
            aos_node::InboxItem::Control(command_ingress("archived-ingress", vec![], 7))
        ),
        Err(PersistError::Conflict(
            PersistConflict::WorldAdminBlocked { .. }
        ))
    ));
    assert!(matches!(
        store.submit_command(
            universe(),
            third_world(),
            command_ingress("archived-command", vec![], 7),
            queued_command("archived-command", "event-send", 7),
        ),
        Err(PersistError::Conflict(
            PersistConflict::WorldAdminBlocked { .. }
        ))
    ));
}

#[test]
fn world_create_from_seed_requires_existing_snapshot_and_manifest_cas() {
    let (_temp, paths) = temp_state_root();
    let store = open_store(&paths);

    let err = store
        .world_create_from_seed(
            universe(),
            CreateWorldSeedRequest {
                world_id: Some(world()),
                handle: Some("broken".into()),
                seed: aos_node::WorldSeed {
                    baseline: aos_node::SnapshotRecord {
                        snapshot_ref: aos_cbor::Hash::of_bytes(b"missing-snapshot").to_hex(),
                        height: 1,
                        logical_time_ns: 10,
                        receipt_horizon_height: Some(1),
                        manifest_hash: Some(aos_cbor::Hash::of_bytes(b"missing-manifest").to_hex()),
                    },
                    seed_kind: aos_node::SeedKind::Genesis,
                    imported_from: None,
                },
                placement_pin: None,
                created_at_ns: 10,
            },
        )
        .unwrap_err();
    assert!(matches!(err, PersistError::NotFound(_)));
}

#[test]
fn world_fork_can_select_snapshot_by_ref_and_records_lineage() {
    let (_temp, paths) = temp_state_root();
    let store = open_store(&paths);
    seed_world(&store, universe(), world(), 1);

    let second_snapshot = store_snapshot_record(
        &store,
        universe(),
        7,
        b"fork-manifest",
        &kernel_snapshot_bytes(7, None),
    );
    store
        .snapshot_index(universe(), world(), second_snapshot.clone())
        .unwrap();

    let fork = store
        .world_fork(
            universe(),
            ForkWorldRequest {
                src_world_id: world(),
                src_snapshot: SnapshotSelector::ByRef {
                    snapshot_ref: second_snapshot.snapshot_ref.clone(),
                },
                new_world_id: Some(second_world()),
                handle: Some("forked".into()),
                placement_pin: Some("cpu".into()),
                forked_at_ns: 777,
                pending_effect_policy: ForkPendingEffectPolicy::ClearAllPendingExternalState,
            },
        )
        .unwrap();

    assert_eq!(
        fork.record.active_baseline.snapshot_ref,
        second_snapshot.snapshot_ref
    );
    match fork.record.meta.lineage.expect("fork lineage") {
        aos_node::WorldLineage::Fork {
            src_world_id,
            src_snapshot_ref,
            src_height,
            ..
        } => {
            assert_eq!(src_world_id, world());
            assert_eq!(src_snapshot_ref, second_snapshot.snapshot_ref);
            assert_eq!(src_height, 7);
        }
        other => panic!("expected fork lineage, got {other:?}"),
    }
}
