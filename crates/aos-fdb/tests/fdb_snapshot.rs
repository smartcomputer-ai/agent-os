#![cfg(feature = "foundationdb-backend")]

mod common;

use aos_cbor::Hash;
use aos_fdb::{PersistError, SnapshotCommitRequest, SnapshotRecord, WorldStore};

fn snapshot_record(
    height: u64,
    snapshot_ref: String,
    receipt_horizon_height: Option<u64>,
) -> SnapshotRecord {
    SnapshotRecord {
        snapshot_ref,
        height,
        logical_time_ns: height * 10,
        receipt_horizon_height,
        manifest_hash: Some("sha256:manifest".into()),
    }
}

#[test]
fn snapshot_commit_indexes_snapshot_and_promotes_baseline_atomically()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let snapshot_bytes = b"kernel-snapshot-v1".to_vec();
    let snapshot_hash = Hash::of_bytes(&snapshot_bytes);
    let record = snapshot_record(0, snapshot_hash.to_hex(), Some(0));

    let result = ctx.persistence.snapshot_commit(
        ctx.universe,
        ctx.world,
        SnapshotCommitRequest {
            expected_head: 0,
            snapshot_bytes,
            record: record.clone(),
            snapshot_journal_entry: b"journal:snapshot".to_vec(),
            baseline_journal_entry: Some(b"journal:baseline".to_vec()),
            promote_baseline: true,
        },
    )?;

    assert_eq!(result.snapshot_hash, snapshot_hash);
    assert_eq!(result.first_height, 0);
    assert_eq!(result.next_head, 2);
    assert!(result.baseline_promoted);
    assert_eq!(
        ctx.persistence
            .snapshot_at_height(ctx.universe, ctx.world, 0)?,
        record
    );
    assert_eq!(
        ctx.persistence
            .snapshot_active_baseline(ctx.universe, ctx.world)?,
        record
    );
    assert_eq!(
        ctx.persistence
            .journal_read_range(ctx.universe, ctx.world, 0, 8)?,
        vec![
            (0, b"journal:snapshot".to_vec()),
            (1, b"journal:baseline".to_vec()),
        ]
    );

    Ok(())
}

#[test]
fn snapshot_index_is_idempotent_for_identical_record() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let snapshot_bytes = b"kernel-snapshot-v2";
    let snapshot_hash = ctx
        .persistence
        .cas_put_verified(ctx.universe, snapshot_bytes)?;
    let record = snapshot_record(3, snapshot_hash.to_hex(), Some(3));

    ctx.persistence
        .snapshot_index(ctx.universe, ctx.world, record.clone())?;
    ctx.persistence
        .snapshot_index(ctx.universe, ctx.world, record.clone())?;

    assert_eq!(
        ctx.persistence
            .snapshot_at_height(ctx.universe, ctx.world, 3)?,
        record
    );

    Ok(())
}

#[test]
fn snapshot_index_upgrades_legacy_record_to_canonical_receipt_horizon()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let snapshot_bytes = b"kernel-snapshot-upgrade";
    let snapshot_hash = ctx
        .persistence
        .cas_put_verified(ctx.universe, snapshot_bytes)?;
    let legacy = snapshot_record(9, snapshot_hash.to_hex(), None);
    let mut canonical = snapshot_record(9, snapshot_hash.to_hex(), Some(9));
    canonical.logical_time_ns = 999;
    canonical.manifest_hash = Some("sha256:manifest-upgraded".into());

    ctx.persistence
        .snapshot_index(ctx.universe, ctx.world, legacy)?;
    ctx.persistence
        .snapshot_index(ctx.universe, ctx.world, canonical.clone())?;

    assert_eq!(
        ctx.persistence
            .snapshot_at_height(ctx.universe, ctx.world, 9)?,
        canonical
    );

    Ok(())
}

#[test]
fn baseline_promotion_requires_receipt_horizon_equal_height()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let snapshot_bytes = b"kernel-snapshot-v3";
    let snapshot_hash = ctx
        .persistence
        .cas_put_verified(ctx.universe, snapshot_bytes)?;
    let record = snapshot_record(5, snapshot_hash.to_hex(), None);
    ctx.persistence
        .snapshot_index(ctx.universe, ctx.world, record.clone())?;

    let err = ctx
        .persistence
        .snapshot_promote_baseline(ctx.universe, ctx.world, record)
        .unwrap_err();
    assert!(matches!(err, PersistError::Validation(_)));

    Ok(())
}

#[test]
fn baseline_promotion_cannot_regress_after_advancing() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let older_hash = ctx
        .persistence
        .cas_put_verified(ctx.universe, b"kernel-snapshot-old")?;
    let newer_hash = ctx
        .persistence
        .cas_put_verified(ctx.universe, b"kernel-snapshot-new")?;
    let older = snapshot_record(2, older_hash.to_hex(), Some(2));
    let newer = snapshot_record(5, newer_hash.to_hex(), Some(5));

    ctx.persistence
        .snapshot_index(ctx.universe, ctx.world, older.clone())?;
    ctx.persistence
        .snapshot_index(ctx.universe, ctx.world, newer.clone())?;
    ctx.persistence
        .snapshot_promote_baseline(ctx.universe, ctx.world, older)?;
    ctx.persistence
        .snapshot_promote_baseline(ctx.universe, ctx.world, newer.clone())?;

    let err = ctx
        .persistence
        .snapshot_promote_baseline(
            ctx.universe,
            ctx.world,
            snapshot_record(2, older_hash.to_hex(), Some(2)),
        )
        .unwrap_err();
    assert!(matches!(err, PersistError::Validation(_)));
    assert_eq!(
        ctx.persistence
            .snapshot_active_baseline(ctx.universe, ctx.world)?,
        newer
    );

    Ok(())
}

#[test]
fn snapshot_latest_returns_highest_indexed_height() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let older_hash = ctx
        .persistence
        .cas_put_verified(ctx.universe, b"kernel-snapshot-latest-old")?;
    let newer_hash = ctx
        .persistence
        .cas_put_verified(ctx.universe, b"kernel-snapshot-latest-new")?;
    let older = snapshot_record(7, older_hash.to_hex(), Some(7));
    let newer = snapshot_record(42, newer_hash.to_hex(), Some(42));

    ctx.persistence
        .snapshot_index(ctx.universe, ctx.world, older)?;
    ctx.persistence
        .snapshot_index(ctx.universe, ctx.world, newer.clone())?;

    assert_eq!(
        ctx.persistence.snapshot_latest(ctx.universe, ctx.world)?,
        newer
    );

    Ok(())
}
