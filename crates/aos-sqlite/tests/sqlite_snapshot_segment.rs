mod common;

use aos_cbor::Hash;
use aos_kernel::snapshot::KernelSnapshot;
use aos_node::{
    SegmentExportRequest, SegmentId, SnapshotCommitRequest, SnapshotRecord, UniverseId,
    WorldAdminStore, WorldId, WorldLineage, WorldStore,
};
use aos_sqlite::SqliteNodeStore;
use uuid::Uuid;

use common::{open_store, temp_state_root, universe};

fn world() -> WorldId {
    WorldId::from(Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap())
}

fn second_world() -> WorldId {
    WorldId::from(Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap())
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

fn prepare_bootstrap_world(
    store: &SqliteNodeStore,
    universe: UniverseId,
    world: WorldId,
    handle: &str,
) -> Hash {
    assert_eq!(universe, common::universe());
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

fn snapshot_record(height: u64, snapshot_ref: String) -> aos_node::SnapshotRecord {
    aos_node::SnapshotRecord {
        snapshot_ref,
        height,
        logical_time_ns: height * 10,
        receipt_horizon_height: Some(height),
        manifest_hash: Some("sha256:manifest".into()),
    }
}

#[test]
fn snapshot_commit_indexes_snapshot_and_promotes_baseline_atomically() {
    let (_temp, paths) = temp_state_root();
    let store = open_store(&paths);
    prepare_bootstrap_world(&store, universe(), world(), "hello-world");

    let snapshot_bytes = kernel_snapshot_bytes(0, None);
    let snapshot_hash = Hash::of_bytes(&snapshot_bytes);
    let record = snapshot_record(0, snapshot_hash.to_hex());

    let result = store
        .snapshot_commit(
            universe(),
            world(),
            SnapshotCommitRequest {
                expected_head: 0,
                snapshot_bytes,
                record: record.clone(),
                snapshot_journal_entry: b"journal:snapshot".to_vec(),
                baseline_journal_entry: Some(b"journal:baseline".to_vec()),
                promote_baseline: true,
            },
        )
        .unwrap();

    assert_eq!(result.snapshot_hash, snapshot_hash);
    assert_eq!(result.first_height, 0);
    assert_eq!(result.next_head, 2);
    assert!(result.baseline_promoted);
    assert_eq!(
        store.snapshot_at_height(universe(), world(), 0).unwrap(),
        record
    );
    assert_eq!(
        store.snapshot_active_baseline(universe(), world()).unwrap(),
        record
    );
    assert_eq!(
        store.journal_read_range(universe(), world(), 0, 8).unwrap(),
        vec![
            (0, b"journal:snapshot".to_vec()),
            (1, b"journal:baseline".to_vec())
        ]
    );
}

#[test]
fn segment_export_is_world_scoped_and_survives_reopen() {
    let (_temp, paths) = temp_state_root();
    let store = open_store(&paths);
    prepare_bootstrap_world(&store, universe(), world(), "alpha");
    prepare_bootstrap_world(&store, universe(), second_world(), "beta");

    store
        .journal_append_batch(
            universe(),
            world(),
            0,
            &[
                b"a0".to_vec(),
                b"a1".to_vec(),
                b"a2".to_vec(),
                b"a3".to_vec(),
            ],
        )
        .unwrap();
    store
        .journal_append_batch(
            universe(),
            second_world(),
            0,
            &[
                b"b0".to_vec(),
                b"b1".to_vec(),
                b"b2".to_vec(),
                b"b3".to_vec(),
            ],
        )
        .unwrap();

    let world_baseline = store_snapshot_record(
        &store,
        universe(),
        4,
        b"manifest-a",
        &kernel_snapshot_bytes(4, None),
    );
    let second_baseline = store_snapshot_record(
        &store,
        universe(),
        4,
        b"manifest-b",
        &kernel_snapshot_bytes(4, None),
    );
    store
        .snapshot_index(universe(), world(), world_baseline.clone())
        .unwrap();
    store
        .snapshot_promote_baseline(universe(), world(), world_baseline)
        .unwrap();
    store
        .snapshot_index(universe(), second_world(), second_baseline.clone())
        .unwrap();
    store
        .snapshot_promote_baseline(universe(), second_world(), second_baseline)
        .unwrap();

    let segment = SegmentId::new(0, 1).unwrap();
    let first = store
        .segment_export(
            universe(),
            world(),
            SegmentExportRequest {
                segment,
                hot_tail_margin: 1,
                delete_chunk_entries: 1,
            },
        )
        .unwrap();
    let second = store
        .segment_export(
            universe(),
            second_world(),
            SegmentExportRequest {
                segment,
                hot_tail_margin: 1,
                delete_chunk_entries: 1,
            },
        )
        .unwrap();

    assert_eq!(first.exported_entries, 2);
    assert_eq!(second.exported_entries, 2);
    assert_eq!(
        store
            .segment_read_entries(universe(), world(), segment)
            .unwrap(),
        vec![(0, b"a0".to_vec()), (1, b"a1".to_vec())]
    );
    assert_eq!(
        store
            .segment_read_entries(universe(), second_world(), segment)
            .unwrap(),
        vec![(0, b"b0".to_vec()), (1, b"b1".to_vec())]
    );

    drop(store);
    let reopened = open_store(&paths);
    assert_eq!(
        reopened
            .journal_read_range(universe(), world(), 2, 8)
            .unwrap(),
        vec![(2, b"a2".to_vec()), (3, b"a3".to_vec())]
    );
    assert_eq!(
        reopened
            .segment_index_read_from(universe(), world(), 1, 8)
            .unwrap()
            .len(),
        1
    );
}
