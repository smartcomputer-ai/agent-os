#![cfg(feature = "foundationdb-backend")]

mod common;

use aos_cbor::Hash;
use aos_fdb::{SegmentExportRequest, SegmentId, SnapshotRecord, WorldStore};

fn baseline_record(snapshot_hash: Hash, height: u64) -> SnapshotRecord {
    SnapshotRecord {
        snapshot_ref: snapshot_hash.to_hex(),
        height,
        logical_time_ns: height * 10,
        receipt_horizon_height: Some(height),
        manifest_hash: Some("sha256:manifest".into()),
    }
}

#[test]
fn reopen_preserves_journal_cursor_baseline_and_segments() -> Result<(), Box<dyn std::error::Error>>
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

    let seq1 = ctx.persistence.inbox_enqueue(
        ctx.universe,
        ctx.world,
        aos_fdb::InboxItem::TimerFired(aos_fdb::TimerFiredIngress {
            timer_id: "timer-1".into(),
            payload: aos_fdb::CborPayload::inline(vec![1]),
            correlation_id: Some("corr-1".into()),
        }),
    )?;
    let seq2 = ctx.persistence.inbox_enqueue(
        ctx.universe,
        ctx.world,
        aos_fdb::InboxItem::TimerFired(aos_fdb::TimerFiredIngress {
            timer_id: "timer-2".into(),
            payload: aos_fdb::CborPayload::inline(vec![2]),
            correlation_id: Some("corr-2".into()),
        }),
    )?;
    ctx.persistence
        .inbox_commit_cursor(ctx.universe, ctx.world, None, seq1.clone())?;
    ctx.persistence
        .inbox_commit_cursor(ctx.universe, ctx.world, Some(seq1), seq2.clone())?;

    let snapshot_bytes = b"snapshot-for-reopen";
    let snapshot_hash = ctx
        .persistence
        .cas_put_verified(ctx.universe, snapshot_bytes)?;
    let baseline = baseline_record(snapshot_hash, 4);
    ctx.persistence
        .snapshot_index(ctx.universe, ctx.world, baseline.clone())?;
    ctx.persistence
        .snapshot_promote_baseline(ctx.universe, ctx.world, baseline.clone())?;

    let segment = SegmentId::new(0, 1)?;
    ctx.persistence.segment_export(
        ctx.universe,
        ctx.world,
        SegmentExportRequest {
            segment,
            hot_tail_margin: 1,
            delete_chunk_entries: 1,
        },
    )?;

    let reopened = common::open_persistence(common::test_config())?;
    assert_eq!(reopened.journal_head(ctx.universe, ctx.world)?, 4);
    assert_eq!(
        reopened.journal_read_range(ctx.universe, ctx.world, 2, 8)?,
        vec![(2, b"j2".to_vec()), (3, b"j3".to_vec())]
    );
    assert_eq!(
        reopened.inbox_cursor(ctx.universe, ctx.world)?,
        Some(seq2.clone())
    );
    assert_eq!(
        reopened.inbox_read_after(ctx.universe, ctx.world, Some(seq2), 8)?,
        Vec::new()
    );
    assert_eq!(
        reopened.snapshot_active_baseline(ctx.universe, ctx.world)?,
        baseline
    );
    assert_eq!(
        reopened
            .segment_index_read_from(ctx.universe, ctx.world, 0, 8)?
            .into_iter()
            .map(|record| record.segment)
            .collect::<Vec<_>>(),
        vec![segment]
    );
    assert_eq!(
        reopened.segment_read_entries(ctx.universe, ctx.world, segment)?,
        vec![(0, b"j0".to_vec()), (1, b"j1".to_vec())]
    );

    Ok(())
}
