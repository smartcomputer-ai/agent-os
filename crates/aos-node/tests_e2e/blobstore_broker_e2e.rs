#[path = "../tests/common/mod.rs"]
mod common;

use aos_node::FsCas;
use aos_node::blobstore::{HostedBlobMetaStore, HostedCas, RemoteCasStore};
use aos_node::{
    BlobBackend, CborPayload, CheckpointBackend, CommandRecord, CommandStatus,
    PromotableBaselineRef, UniverseId, WorldCheckpointRef, WorldId, WorldJournalCursor,
};
use serial_test::serial;

use common::{blobstore_bucket_enabled, broker_blobstore_config};

#[test]
#[serial]
fn object_store_blob_roundtrip_and_prefix_isolation() {
    if !blobstore_bucket_enabled() {
        return;
    }

    let universe_id = UniverseId::from(uuid::Uuid::new_v4());
    let config_a = broker_blobstore_config("blob-roundtrip-a").unwrap();
    let config_b = broker_blobstore_config("blob-roundtrip-b").unwrap();
    let blobstore_a = RemoteCasStore::new(config_a).unwrap();
    let blobstore_b = RemoteCasStore::new(config_b).unwrap();

    let payload = br#"{"hello":"blobstore"}"#;
    let hash = blobstore_a.put_blob(universe_id, payload).unwrap();

    assert!(blobstore_a.has_blob(universe_id, hash).unwrap());
    assert_eq!(blobstore_a.get_blob(universe_id, hash).unwrap(), payload);
    assert!(!blobstore_b.has_blob(universe_id, hash).unwrap());
}

#[test]
#[serial]
fn object_store_checkpoints_and_command_records_roundtrip() {
    if !blobstore_bucket_enabled() {
        return;
    }

    let config = broker_blobstore_config("checkpoint-command-roundtrip").unwrap();
    let cas = RemoteCasStore::new(config.clone()).unwrap();
    let mut blobstore = HostedBlobMetaStore::new(config.clone()).unwrap();
    let universe_id = UniverseId::from(uuid::Uuid::new_v4());
    let world_id = WorldId::from(uuid::Uuid::new_v4());

    let snapshot_bytes = b"snapshot-bytes";
    let snapshot_hash = cas.put_blob(universe_id, snapshot_bytes).unwrap();

    let checkpoint = WorldCheckpointRef {
        universe_id,
        world_id,
        world_epoch: 7,
        checkpointed_at_ns: 123,
        world_seq: 3,
        baseline: PromotableBaselineRef {
            snapshot_ref: snapshot_hash.to_hex(),
            snapshot_manifest_ref: None,
            manifest_hash: "manifest-123".into(),
            universe_id: UniverseId::nil(),
            height: 3,
            logical_time_ns: 55,
            receipt_horizon_height: 3,
        },
        journal_cursor: Some(WorldJournalCursor::Kafka {
            journal_topic: "aos-journal".into(),
            partition: 0,
            journal_offset: 11,
        }),
    };
    blobstore
        .commit_world_checkpoint(checkpoint.clone())
        .unwrap();

    let command_record = CommandRecord {
        command_id: "cmd-1".into(),
        command: "governance/apply".into(),
        status: CommandStatus::Succeeded,
        submitted_at_ns: 100,
        started_at_ns: Some(110),
        finished_at_ns: Some(120),
        journal_height: Some(3),
        manifest_hash: Some("manifest-123".into()),
        result_payload: Some(CborPayload::inline(vec![0x01])),
        error: None,
    };
    blobstore
        .put_command_record(world_id, command_record.clone())
        .unwrap();

    let mut reopened = HostedBlobMetaStore::new(config).unwrap();
    reopened.prime_latest_checkpoints().unwrap();

    let latest = reopened.latest_world_checkpoint(world_id).unwrap().unwrap();
    assert_eq!(latest, checkpoint);
    assert_eq!(
        reopened
            .get_command_record(world_id, "cmd-1")
            .unwrap()
            .unwrap(),
        command_record
    );
}

#[test]
#[serial]
fn hosted_cas_hydrates_snapshot_blob_into_fresh_cache() {
    if !blobstore_bucket_enabled() {
        return;
    }

    let config = broker_blobstore_config("hydrate-snapshot").unwrap();
    let remote = RemoteCasStore::new(config).unwrap();
    let universe_id = UniverseId::from(uuid::Uuid::new_v4());
    let snapshot_bytes = b"snapshot-payload";
    let snapshot_hash = remote.put_blob(universe_id, snapshot_bytes).unwrap();

    let local_root = tempfile::tempdir().unwrap();
    let local = std::sync::Arc::new(FsCas::open_cas_root(local_root.path()).unwrap());
    let hosted = HostedCas::new(local.clone(), std::sync::Arc::new(remote));
    assert!(!local.has(snapshot_hash));

    assert_eq!(hosted.get(snapshot_hash).unwrap(), snapshot_bytes);
    assert!(local.has(snapshot_hash));
}
