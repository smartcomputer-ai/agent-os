#![cfg(feature = "foundationdb-backend")]

mod common;

use aos_fdb::{
    CborPayload, CommandIngress, InboxItem, PersistConflict, PersistError, TimerFiredIngress,
    WorldStore,
};

fn timer(seed: u8) -> InboxItem {
    InboxItem::TimerFired(TimerFiredIngress {
        timer_id: format!("timer-{seed}"),
        payload: CborPayload::inline(vec![seed]),
        correlation_id: Some(format!("corr-{seed}")),
    })
}

#[test]
fn inbox_enqueue_orders_items_and_supports_pagination() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let first = timer(1);
    let second = timer(2);
    let third = InboxItem::Control(CommandIngress {
        command_id: "cmd-3".into(),
        command: "event-send".into(),
        actor: None,
        payload: CborPayload::inline(b"this payload is externalized".to_vec()),
        submitted_at_ns: 3,
    });

    let seq1 = ctx
        .persistence
        .inbox_enqueue(ctx.universe, ctx.world, first.clone())?;
    let seq2 = ctx
        .persistence
        .inbox_enqueue(ctx.universe, ctx.world, second.clone())?;
    let seq3 = ctx
        .persistence
        .inbox_enqueue(ctx.universe, ctx.world, third)?;

    let first_page = ctx
        .persistence
        .inbox_read_after(ctx.universe, ctx.world, None, 2)?;
    assert_eq!(first_page.len(), 2);
    assert_eq!(first_page[0], (seq1, first));
    assert_eq!(first_page[1], (seq2.clone(), second));

    let second_page = ctx
        .persistence
        .inbox_read_after(ctx.universe, ctx.world, Some(seq2), 2)?;
    assert_eq!(second_page.len(), 1);
    assert_eq!(second_page[0].0, seq3);

    Ok(())
}

#[test]
fn inbox_commit_cursor_uses_compare_and_swap() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let first = ctx
        .persistence
        .inbox_enqueue(ctx.universe, ctx.world, timer(1))?;
    let second = ctx
        .persistence
        .inbox_enqueue(ctx.universe, ctx.world, timer(2))?;

    ctx.persistence
        .inbox_commit_cursor(ctx.universe, ctx.world, None, first.clone())?;

    let err = ctx
        .persistence
        .inbox_commit_cursor(ctx.universe, ctx.world, None, second)
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::InboxCursorAdvanced {
            expected: None,
            actual: Some(actual)
        }) if actual == first
    ));

    Ok(())
}

#[test]
fn drain_inbox_to_journal_is_atomic_on_head_conflict() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let seq = ctx
        .persistence
        .inbox_enqueue(ctx.universe, ctx.world, timer(1))?;
    ctx.persistence
        .journal_append_batch(ctx.universe, ctx.world, 0, &[b"existing".to_vec()])?;

    let err = ctx
        .persistence
        .drain_inbox_to_journal(
            ctx.universe,
            ctx.world,
            None,
            seq,
            0,
            &[b"new-entry".to_vec()],
        )
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::HeadAdvanced {
            expected: 0,
            actual: 1
        })
    ));
    assert_eq!(ctx.persistence.inbox_cursor(ctx.universe, ctx.world)?, None);
    assert_eq!(ctx.persistence.journal_head(ctx.universe, ctx.world)?, 1);
    assert_eq!(
        ctx.persistence
            .journal_read_range(ctx.universe, ctx.world, 0, 8)?,
        vec![(0, b"existing".to_vec())]
    );

    Ok(())
}
