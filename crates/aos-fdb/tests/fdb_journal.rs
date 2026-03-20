#![cfg(feature = "foundationdb-backend")]

mod common;

use aos_fdb::{PersistConflict, PersistError, WorldStore};

#[test]
fn journal_append_and_read_range_support_windowing() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    ctx.persistence.journal_append_batch(
        ctx.universe,
        ctx.world,
        0,
        &[b"entry-0".to_vec(), b"entry-1".to_vec()],
    )?;
    ctx.persistence
        .journal_append_batch(ctx.universe, ctx.world, 2, &[b"entry-2".to_vec()])?;

    assert_eq!(ctx.persistence.journal_head(ctx.universe, ctx.world)?, 3);
    assert_eq!(
        ctx.persistence
            .journal_read_range(ctx.universe, ctx.world, 1, 2)?,
        vec![(1, b"entry-1".to_vec()), (2, b"entry-2".to_vec())]
    );

    Ok(())
}

#[test]
fn journal_append_conflicts_on_stale_expected_head() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    ctx.persistence
        .journal_append_batch(ctx.universe, ctx.world, 0, &[b"first".to_vec()])?;

    let err = ctx
        .persistence
        .journal_append_batch(ctx.universe, ctx.world, 0, &[b"stale".to_vec()])
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::HeadAdvanced {
            expected: 0,
            actual: 1
        })
    ));
    assert_eq!(ctx.persistence.journal_head(ctx.universe, ctx.world)?, 1);

    Ok(())
}

#[test]
fn failed_journal_append_does_not_advance_head() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    ctx.persistence
        .journal_append_batch(ctx.universe, ctx.world, 0, &[b"first".to_vec()])?;

    let _ = ctx
        .persistence
        .journal_append_batch(ctx.universe, ctx.world, 0, &[b"stale".to_vec()])
        .unwrap_err();

    assert_eq!(ctx.persistence.journal_head(ctx.universe, ctx.world)?, 1);
    assert_eq!(
        ctx.persistence
            .journal_read_range(ctx.universe, ctx.world, 0, 8)?,
        vec![(0, b"first".to_vec())]
    );

    Ok(())
}
