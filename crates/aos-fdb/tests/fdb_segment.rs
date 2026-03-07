#![cfg(feature = "foundationdb-backend")]

mod common;

use aos_fdb::{SegmentExportRequest, SegmentId, SnapshotRecord, WorldPersistence};

fn baseline(height: u64) -> SnapshotRecord {
    SnapshotRecord {
        snapshot_ref: "sha256:snapshot".into(),
        height,
        logical_time_ns: height * 10,
        receipt_horizon_height: Some(height),
        manifest_hash: Some("sha256:manifest".into()),
    }
}

#[test]
fn segment_export_indexes_object_and_deletes_hot_entries() -> Result<(), Box<dyn std::error::Error>>
{
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    ctx.persistence.journal_append_batch(
        ctx.universe,
        ctx.world,
        0,
        &[
            b"j0".to_vec(),
            b"j1".to_vec(),
            b"j2".to_vec(),
            b"j3".to_vec(),
        ],
    )?;
    let baseline = baseline(4);
    ctx.persistence
        .snapshot_index(ctx.universe, ctx.world, baseline.clone())?;
    ctx.persistence
        .snapshot_promote_baseline(ctx.universe, ctx.world, baseline)?;

    let result = ctx.persistence.segment_export(
        ctx.universe,
        ctx.world,
        SegmentExportRequest {
            segment: SegmentId::new(0, 1)?,
            hot_tail_margin: 1,
            delete_chunk_entries: 1,
        },
    )?;

    assert_eq!(result.exported_entries, 2);
    assert_eq!(result.deleted_entries, 2);
    assert_eq!(
        ctx.persistence
            .segment_read_entries(ctx.universe, ctx.world, SegmentId::new(0, 1)?)?,
        vec![(0, b"j0".to_vec()), (1, b"j1".to_vec())]
    );
    assert_eq!(
        ctx.persistence
            .journal_read_range(ctx.universe, ctx.world, 2, 8)?,
        vec![(2, b"j2".to_vec()), (3, b"j3".to_vec())]
    );

    Ok(())
}

#[test]
fn segment_export_rejects_ranges_not_strictly_older_than_baseline()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    ctx.persistence.journal_append_batch(
        ctx.universe,
        ctx.world,
        0,
        &[b"j0".to_vec(), b"j1".to_vec(), b"j2".to_vec()],
    )?;
    let baseline = baseline(2);
    ctx.persistence
        .snapshot_index(ctx.universe, ctx.world, baseline.clone())?;
    ctx.persistence
        .snapshot_promote_baseline(ctx.universe, ctx.world, baseline)?;

    let err = ctx
        .persistence
        .segment_export(
            ctx.universe,
            ctx.world,
            SegmentExportRequest {
                segment: SegmentId::new(0, 2)?,
                hot_tail_margin: 0,
                delete_chunk_entries: 2,
            },
        )
        .unwrap_err();
    assert!(matches!(err, aos_fdb::PersistError::Validation(_)));

    Ok(())
}

#[test]
fn segment_export_preserves_restore_equivalence_with_hot_tail()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    ctx.persistence.journal_append_batch(
        ctx.universe,
        ctx.world,
        0,
        &[
            b"j0".to_vec(),
            b"j1".to_vec(),
            b"j2".to_vec(),
            b"j3".to_vec(),
            b"j4".to_vec(),
        ],
    )?;
    let expected = ctx
        .persistence
        .journal_read_range(ctx.universe, ctx.world, 0, 8)?;
    let baseline = baseline(5);
    ctx.persistence
        .snapshot_index(ctx.universe, ctx.world, baseline.clone())?;
    ctx.persistence
        .snapshot_promote_baseline(ctx.universe, ctx.world, baseline)?;

    let segment = SegmentId::new(0, 2)?;
    ctx.persistence.segment_export(
        ctx.universe,
        ctx.world,
        SegmentExportRequest {
            segment,
            hot_tail_margin: 1,
            delete_chunk_entries: 2,
        },
    )?;

    let mut reconstructed =
        ctx.persistence
            .segment_read_entries(ctx.universe, ctx.world, segment)?;
    reconstructed.extend(
        ctx.persistence
            .journal_read_range(ctx.universe, ctx.world, 3, 8)?,
    );
    assert_eq!(reconstructed, expected);

    Ok(())
}
