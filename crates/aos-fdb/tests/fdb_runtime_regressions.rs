#![cfg(feature = "foundationdb-backend")]

mod common;

use aos_effects::builtins::TimerSetParams;
use aos_effects::{EffectKind, ReceiptStatus};
use aos_fdb::{
    CborPayload, CreateWorldSeedRequest, EffectDispatchItem, ForkPendingEffectPolicy,
    ForkWorldRequest, HostedCoordinationStore, HostedEffectQueueStore, HostedPortalStore,
    HostedTimerQueueStore, InboxItem, NodeCatalog, ReceiptIngress, SeedKind, SnapshotRecord,
    SnapshotSelector, TimerDueItem, WorkerHeartbeat, WorldAdminStore, WorldId, WorldIngressStore,
    WorldLineage, WorldSeed, WorldStore,
};
use aos_kernel::snapshot::KernelSnapshot;
use uuid::Uuid;

fn seed_request(
    ctx: &common::TestContext,
    world_id: WorldId,
    height: u64,
) -> Result<CreateWorldSeedRequest, Box<dyn std::error::Error>> {
    let snapshot_bytes = serde_cbor::to_vec(&KernelSnapshot::new(
        height,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        height * 10,
        None,
    ))?;
    let manifest_hash = ctx
        .persistence
        .cas_put_verified(ctx.universe, b"manifest")?;
    let snapshot_hash = ctx
        .persistence
        .cas_put_verified(ctx.universe, &snapshot_bytes)?;
    Ok(CreateWorldSeedRequest {
        world_id: Some(world_id),
        handle: None,
        seed: WorldSeed {
            baseline: SnapshotRecord {
                snapshot_ref: snapshot_hash.to_hex(),
                height,
                logical_time_ns: height * 10,
                receipt_horizon_height: Some(height),
                manifest_hash: Some(manifest_hash.to_hex()),
            },
            seed_kind: SeedKind::Genesis,
            imported_from: None,
        },
        placement_pin: Some("gpu".into()),
        created_at_ns: 123,
    })
}

fn seed_world(
    ctx: &common::TestContext,
    world_id: WorldId,
    height: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    ctx.persistence
        .world_create_from_seed(ctx.universe, seed_request(ctx, world_id, height)?)?;
    Ok(())
}

#[test]
fn guarded_snapshot_index_succeeds_with_live_lease() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let now_ns = 1_000;
    let lease =
        ctx.persistence
            .acquire_world_lease(ctx.universe, ctx.world, "worker-a", now_ns, 5_000)?;
    let record = SnapshotRecord {
        snapshot_ref: "cas:seed".into(),
        height: 0,
        logical_time_ns: now_ns,
        receipt_horizon_height: Some(0),
        manifest_hash: Some("sha256:manifest".into()),
    };

    ctx.persistence.snapshot_index_guarded(
        ctx.universe,
        ctx.world,
        &lease,
        now_ns,
        record.clone(),
    )?;

    assert_eq!(
        ctx.persistence
            .snapshot_at_height(ctx.universe, ctx.world, 0)?,
        record
    );
    Ok(())
}

#[test]
fn effect_ack_enqueues_tagged_receipt_inbox_item() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let now_ns = 2_000;
    let lease =
        ctx.persistence
            .acquire_world_lease(ctx.universe, ctx.world, "worker-a", now_ns, 5_000)?;
    let item = EffectDispatchItem {
        shard: 0,
        universe_id: ctx.universe,
        world_id: ctx.world,
        intent_hash: vec![1; 32],
        effect_kind: EffectKind::HTTP_REQUEST.to_string(),
        cap_name: "http".into(),
        params_inline_cbor: Some(vec![0xA0]),
        params_ref: None,
        params_size: None,
        params_sha256: None,
        idempotency_key: vec![2; 32],
        origin_name: "test".into(),
        policy_context_hash: None,
        enqueued_at_ns: now_ns,
    };

    assert_eq!(
        ctx.persistence.publish_effect_dispatches_guarded(
            ctx.universe,
            ctx.world,
            &lease,
            now_ns,
            &[item.clone()],
        )?,
        1
    );
    let claimed = ctx.persistence.claim_pending_effects_for_world(
        ctx.universe,
        ctx.world,
        "worker-a",
        now_ns,
        5_000,
        8,
    )?;
    assert_eq!(claimed.len(), 1);
    assert!(
        ctx.persistence
            .world_runtime_info(ctx.universe, ctx.world, now_ns)?
            .has_pending_effects
    );

    ctx.persistence.ack_effect_dispatch_with_receipt(
        ctx.universe,
        ctx.world,
        "worker-a",
        item.shard,
        claimed[0].0.clone(),
        now_ns,
        ReceiptIngress {
            intent_hash: item.intent_hash.clone(),
            effect_kind: item.effect_kind.clone(),
            adapter_id: "stub.http".into(),
            status: ReceiptStatus::Ok,
            payload: CborPayload::inline(vec![1, 2, 3]),
            cost_cents: Some(0),
            signature: vec![9; 64],
            correlation_id: Some("effect-receipt".into()),
        },
    )?;

    let inbox = ctx
        .persistence
        .inbox_read_after(ctx.universe, ctx.world, None, 8)?;
    assert_eq!(inbox.len(), 1);
    match &inbox[0].1 {
        InboxItem::Receipt(receipt) => {
            assert_eq!(receipt.intent_hash, item.intent_hash);
            assert_eq!(receipt.adapter_id, "stub.http");
            assert_eq!(receipt.payload.inline_cbor.as_deref(), Some(&[1, 2, 3][..]));
        }
        other => panic!("expected receipt inbox item, got {other:?}"),
    }
    assert!(
        !ctx.persistence
            .world_runtime_info(ctx.universe, ctx.world, now_ns)?
            .has_pending_effects
    );

    Ok(())
}

#[test]
fn timer_ack_enqueues_tagged_receipt_inbox_item() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    seed_world(&ctx, ctx.world, 0)?;
    let now_ns = 3_000;
    let lease =
        ctx.persistence
            .acquire_world_lease(ctx.universe, ctx.world, "worker-a", now_ns, 5_000)?;
    let item = TimerDueItem {
        shard: 0,
        universe_id: ctx.universe,
        world_id: ctx.world,
        intent_hash: vec![3; 32],
        time_bucket: 0,
        deliver_at_ns: 0,
        payload_cbor: serde_cbor::to_vec(&TimerSetParams {
            deliver_at_ns: 0,
            key: Some("retry".into()),
        })?,
        enqueued_at_ns: now_ns,
    };

    assert_eq!(
        ctx.persistence.publish_due_timers_guarded(
            ctx.universe,
            ctx.world,
            &lease,
            now_ns,
            &[item.clone()],
        )?,
        1
    );
    let claimed = ctx.persistence.claim_due_timers_for_world(
        ctx.universe,
        ctx.world,
        "worker-a",
        now_ns,
        5_000,
        8,
    )?;
    assert_eq!(claimed.len(), 1);

    ctx.persistence.ack_timer_delivery_with_receipt(
        ctx.universe,
        ctx.world,
        "worker-a",
        &item.intent_hash,
        now_ns,
        ReceiptIngress {
            intent_hash: item.intent_hash.clone(),
            effect_kind: EffectKind::TIMER_SET.to_string(),
            adapter_id: EffectKind::TIMER_SET.to_string(),
            status: ReceiptStatus::Ok,
            payload: CborPayload::inline(vec![4, 5, 6]),
            cost_cents: Some(0),
            signature: vec![8; 64],
            correlation_id: Some("timer-receipt".into()),
        },
    )?;

    let inbox = ctx
        .persistence
        .inbox_read_after(ctx.universe, ctx.world, None, 8)?;
    assert_eq!(inbox.len(), 1);
    match &inbox[0].1 {
        InboxItem::Receipt(receipt) => {
            assert_eq!(receipt.intent_hash, item.intent_hash);
            assert_eq!(receipt.adapter_id, EffectKind::TIMER_SET.to_string());
            assert_eq!(receipt.payload.inline_cbor.as_deref(), Some(&[4, 5, 6][..]));
        }
        other => panic!("expected receipt inbox item, got {other:?}"),
    }

    Ok(())
}

#[test]
fn effect_claim_honors_batch_limit() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let now_ns = 4_000;
    let lease =
        ctx.persistence
            .acquire_world_lease(ctx.universe, ctx.world, "worker-a", now_ns, 5_000)?;

    let items: Vec<_> = (0..3u8)
        .map(|idx| EffectDispatchItem {
            shard: 0,
            universe_id: ctx.universe,
            world_id: ctx.world,
            intent_hash: vec![idx + 1; 32],
            effect_kind: EffectKind::HTTP_REQUEST.to_string(),
            cap_name: "http".into(),
            params_inline_cbor: Some(vec![0xA0]),
            params_ref: None,
            params_size: None,
            params_sha256: None,
            idempotency_key: vec![idx + 11; 32],
            origin_name: format!("test-{idx}"),
            policy_context_hash: None,
            enqueued_at_ns: now_ns,
        })
        .collect();

    assert_eq!(
        ctx.persistence.publish_effect_dispatches_guarded(
            ctx.universe,
            ctx.world,
            &lease,
            now_ns,
            &items,
        )?,
        items.len() as u32
    );

    let claimed = ctx.persistence.claim_pending_effects_for_world(
        ctx.universe,
        ctx.world,
        "worker-a",
        now_ns,
        5_000,
        3,
    )?;
    assert_eq!(claimed.len(), 3);
    let info = ctx
        .persistence
        .world_runtime_info(ctx.universe, ctx.world, now_ns)?;
    assert!(info.has_pending_effects);
    Ok(())
}

#[test]
fn timer_claim_honors_batch_limit() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let now_ns = 5_000;
    let lease =
        ctx.persistence
            .acquire_world_lease(ctx.universe, ctx.world, "worker-a", now_ns, 5_000)?;

    let items: Vec<_> = (0..3u8)
        .map(|idx| TimerDueItem {
            shard: 0,
            universe_id: ctx.universe,
            world_id: ctx.world,
            intent_hash: vec![idx + 21; 32],
            time_bucket: 0,
            deliver_at_ns: 0,
            payload_cbor: serde_cbor::to_vec(&TimerSetParams {
                deliver_at_ns: 0,
                key: Some(format!("retry-{idx}")),
            })
            .expect("encode timer params"),
            enqueued_at_ns: now_ns,
        })
        .collect();

    assert_eq!(
        ctx.persistence.publish_due_timers_guarded(
            ctx.universe,
            ctx.world,
            &lease,
            now_ns,
            &items,
        )?,
        items.len() as u32
    );

    let claimed = ctx.persistence.claim_due_timers_for_world(
        ctx.universe,
        ctx.world,
        "worker-a",
        now_ns,
        5_000,
        3,
    )?;
    assert_eq!(claimed.len(), 3);
    Ok(())
}

#[test]
fn snapshot_promote_baseline_preserves_world_placement_pin()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    ctx.persistence
        .set_world_placement_pin(ctx.universe, ctx.world, Some("gpu".into()))?;

    let record = SnapshotRecord {
        snapshot_ref: "cas:seed".into(),
        height: 7,
        logical_time_ns: 4_000,
        receipt_horizon_height: Some(7),
        manifest_hash: Some("sha256:manifest".into()),
    };
    ctx.persistence
        .snapshot_index(ctx.universe, ctx.world, record.clone())?;
    ctx.persistence
        .snapshot_promote_baseline(ctx.universe, ctx.world, record)?;

    let info = ctx
        .persistence
        .world_runtime_info(ctx.universe, ctx.world, 4_000)?;
    assert_eq!(info.meta.placement_pin.as_deref(), Some("gpu"));
    assert_eq!(info.meta.active_baseline_height, Some(7));
    Ok(())
}

#[test]
fn acquire_world_lease_reclaims_holder_without_live_heartbeat()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    ctx.persistence.heartbeat_worker(WorkerHeartbeat {
        worker_id: "worker-a".into(),
        pins: vec!["default".into()],
        last_seen_ns: 10,
        expires_at_ns: 15,
    })?;

    let first =
        ctx.persistence
            .acquire_world_lease(ctx.universe, ctx.world, "worker-a", 10, 100)?;
    let second =
        ctx.persistence
            .acquire_world_lease(ctx.universe, ctx.world, "worker-b", 20, 100)?;

    assert_eq!(first.epoch, 1);
    assert_eq!(second.epoch, 2);
    assert_eq!(second.holder_worker_id, "worker-b");
    Ok(())
}

#[test]
fn world_create_from_seed_persists_complete_seeded_world_shape()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let request = seed_request(&ctx, ctx.world, 9)?;
    let result = ctx
        .persistence
        .world_create_from_seed(ctx.universe, request)?;

    assert_eq!(result.record.world_id, ctx.world);
    assert_eq!(result.record.journal_head, 10);
    assert_eq!(result.record.meta.active_baseline_height, Some(9));
    assert_eq!(result.record.meta.placement_pin.as_deref(), Some("gpu"));
    assert_eq!(
        result.record.meta.lineage,
        Some(WorldLineage::Genesis { created_at_ns: 123 })
    );
    assert_eq!(ctx.persistence.journal_head(ctx.universe, ctx.world)?, 10);
    assert_eq!(
        ctx.persistence
            .snapshot_active_baseline(ctx.universe, ctx.world)?,
        result.record.active_baseline
    );
    Ok(())
}

#[test]
fn world_fork_copies_active_baseline_and_records_lineage() -> Result<(), Box<dyn std::error::Error>>
{
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let src_world = ctx.world;
    let fork_world = WorldId::from(Uuid::new_v4());
    ctx.persistence
        .world_create_from_seed(ctx.universe, seed_request(&ctx, src_world, 4)?)?;

    let result = ctx.persistence.world_fork(
        ctx.universe,
        ForkWorldRequest {
            src_world_id: src_world,
            src_snapshot: SnapshotSelector::ActiveBaseline,
            new_world_id: Some(fork_world),
            handle: None,
            placement_pin: None,
            forked_at_ns: 456,
            pending_effect_policy: ForkPendingEffectPolicy::default(),
        },
    )?;

    assert_eq!(result.record.world_id, fork_world);
    assert_eq!(result.record.journal_head, 5);
    assert_eq!(result.record.meta.placement_pin.as_deref(), Some("gpu"));
    assert_eq!(
        result.record.meta.lineage,
        Some(WorldLineage::Fork {
            forked_at_ns: 456,
            src_universe_id: ctx.universe,
            src_world_id: src_world,
            src_snapshot_ref: result.record.active_baseline.snapshot_ref.clone(),
            src_height: 4,
        })
    );
    Ok(())
}

#[test]
fn ready_queue_and_worker_lease_index_track_runtime_state() -> Result<(), Box<dyn std::error::Error>>
{
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    seed_world(&ctx, ctx.world, 0)?;
    let now_ns = 4_000;
    ctx.persistence.enqueue_ingress(
        ctx.universe,
        ctx.world,
        InboxItem::Control(aos_fdb::CommandIngress {
            command_id: "cmd-ready".into(),
            command: "event-send".into(),
            actor: None,
            payload: CborPayload::inline(vec![1, 2, 3]),
            submitted_at_ns: now_ns,
        }),
    )?;
    let lease =
        ctx.persistence
            .acquire_world_lease(ctx.universe, ctx.world, "worker-a", now_ns, 5_000)?;

    let ready = ctx
        .persistence
        .list_ready_worlds(now_ns, 8, Some(&[ctx.universe]))?;
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].universe_id, ctx.universe);
    assert_eq!(ready[0].info.world_id, ctx.world);
    assert!(ready[0].info.has_pending_inbox);
    assert_eq!(ready[0].info.lease, Some(lease.clone()));

    let leased =
        ctx.persistence
            .list_worker_worlds("worker-a", now_ns, 8, Some(&[ctx.universe]))?;
    assert_eq!(leased.len(), 1);
    assert_eq!(leased[0].universe_id, ctx.universe);
    assert_eq!(leased[0].info.world_id, ctx.world);

    Ok(())
}

#[test]
fn portal_send_is_idempotent_for_same_message_id() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let dest_world = WorldId::from(Uuid::new_v4());
    ctx.persistence.snapshot_index(
        ctx.universe,
        dest_world,
        SnapshotRecord {
            snapshot_ref: "cas:seed".into(),
            height: 0,
            logical_time_ns: 5_000,
            receipt_horizon_height: Some(0),
            manifest_hash: Some("sha256:manifest".into()),
        },
    )?;
    ctx.persistence.snapshot_promote_baseline(
        ctx.universe,
        dest_world,
        SnapshotRecord {
            snapshot_ref: "cas:seed".into(),
            height: 0,
            logical_time_ns: 5_000,
            receipt_horizon_height: Some(0),
            manifest_hash: Some("sha256:manifest".into()),
        },
    )?;

    let first = ctx.persistence.portal_send(
        ctx.universe,
        dest_world,
        7_000,
        &[7; 32],
        InboxItem::DomainEvent(aos_fdb::DomainEventIngress {
            schema: "com.acme/Event@1".into(),
            value: CborPayload::inline(vec![0xA1]),
            key: None,
            correlation_id: Some("portal-first".into()),
        }),
    )?;
    let second = ctx.persistence.portal_send(
        ctx.universe,
        dest_world,
        7_001,
        &[7; 32],
        InboxItem::DomainEvent(aos_fdb::DomainEventIngress {
            schema: "com.acme/Event@1".into(),
            value: CborPayload::inline(vec![0xA1]),
            key: None,
            correlation_id: Some("portal-second".into()),
        }),
    )?;

    assert_eq!(first.status, aos_fdb::PortalSendStatus::Enqueued);
    assert_eq!(second.status, aos_fdb::PortalSendStatus::AlreadyEnqueued);
    let inbox = ctx
        .persistence
        .inbox_read_after(ctx.universe, dest_world, None, 8)?;
    assert_eq!(inbox.len(), 1);
    Ok(())
}

#[test]
fn dedupe_gc_releases_expired_effect_timer_and_portal_records()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let mut config = common::test_config();
    config.dedupe_gc = aos_fdb::DedupeGcConfig {
        effect_retention_ns: 5,
        timer_retention_ns: 5,
        portal_retention_ns: 5,
        bucket_width_ns: 5,
    };
    let ctx = common::open_test_context(config)?;
    seed_world(&ctx, ctx.world, 0)?;
    let now_ns = 10_000;
    let lease =
        ctx.persistence
            .acquire_world_lease(ctx.universe, ctx.world, "worker-a", now_ns, 5_000)?;

    let effect = EffectDispatchItem {
        shard: 0,
        universe_id: ctx.universe,
        world_id: ctx.world,
        intent_hash: vec![1; 32],
        effect_kind: EffectKind::HTTP_REQUEST.to_string(),
        cap_name: "http".into(),
        params_inline_cbor: Some(vec![0xA0]),
        params_ref: None,
        params_size: None,
        params_sha256: None,
        idempotency_key: vec![2; 32],
        origin_name: "test".into(),
        policy_context_hash: None,
        enqueued_at_ns: now_ns,
    };
    assert_eq!(
        ctx.persistence.publish_effect_dispatches_guarded(
            ctx.universe,
            ctx.world,
            &lease,
            now_ns,
            &[effect.clone()],
        )?,
        1
    );
    let claimed = ctx.persistence.claim_pending_effects_for_world(
        ctx.universe,
        ctx.world,
        "worker-a",
        now_ns,
        5_000,
        8,
    )?;
    ctx.persistence.ack_effect_dispatch_with_receipt(
        ctx.universe,
        ctx.world,
        "worker-a",
        effect.shard,
        claimed[0].0.clone(),
        now_ns,
        ReceiptIngress {
            intent_hash: effect.intent_hash.clone(),
            effect_kind: effect.effect_kind.clone(),
            adapter_id: "stub.http".into(),
            status: ReceiptStatus::Ok,
            payload: CborPayload::inline(vec![1]),
            cost_cents: Some(0),
            signature: vec![0; 64],
            correlation_id: None,
        },
    )?;

    let timer = TimerDueItem {
        shard: 0,
        universe_id: ctx.universe,
        world_id: ctx.world,
        intent_hash: vec![3; 32],
        time_bucket: 0,
        deliver_at_ns: now_ns,
        payload_cbor: serde_cbor::to_vec(&TimerSetParams {
            deliver_at_ns: now_ns,
            key: None,
        })?,
        enqueued_at_ns: now_ns,
    };
    assert_eq!(
        ctx.persistence.publish_due_timers_guarded(
            ctx.universe,
            ctx.world,
            &lease,
            now_ns,
            &[timer.clone()],
        )?,
        1
    );
    let claimed_timers = ctx.persistence.claim_due_timers_for_world(
        ctx.universe,
        ctx.world,
        "worker-a",
        now_ns,
        5_000,
        8,
    )?;
    assert_eq!(claimed_timers.len(), 1);
    ctx.persistence.ack_timer_delivery_with_receipt(
        ctx.universe,
        ctx.world,
        "worker-a",
        &timer.intent_hash,
        now_ns,
        ReceiptIngress {
            intent_hash: timer.intent_hash.clone(),
            effect_kind: EffectKind::TIMER_SET.to_string(),
            adapter_id: EffectKind::TIMER_SET.to_string(),
            status: ReceiptStatus::Ok,
            payload: CborPayload::inline(vec![2]),
            cost_cents: Some(0),
            signature: vec![0; 64],
            correlation_id: None,
        },
    )?;

    let dest_world = WorldId::from(Uuid::new_v4());
    seed_world(&ctx, dest_world, 0)?;
    assert_eq!(
        ctx.persistence
            .portal_send(
                ctx.universe,
                dest_world,
                now_ns,
                b"portal-dedupe",
                InboxItem::DomainEvent(aos_fdb::DomainEventIngress {
                    schema: "com.acme/Event@1".into(),
                    value: CborPayload::inline(vec![3]),
                    key: None,
                    correlation_id: None,
                }),
            )?
            .status,
        aos_fdb::PortalSendStatus::Enqueued
    );

    let sweep_now = now_ns + 100;
    assert_eq!(
        ctx.persistence
            .sweep_effect_dedupe_gc(ctx.universe, sweep_now, 8)?,
        1
    );
    assert_eq!(
        ctx.persistence
            .sweep_timer_dedupe_gc(ctx.universe, sweep_now, 8)?,
        1
    );
    assert_eq!(
        ctx.persistence
            .sweep_portal_dedupe_gc(ctx.universe, sweep_now, 8)?,
        1
    );

    assert_eq!(
        ctx.persistence.publish_effect_dispatches_guarded(
            ctx.universe,
            ctx.world,
            &lease,
            sweep_now,
            &[effect.clone()],
        )?,
        1
    );
    assert_eq!(
        ctx.persistence.publish_due_timers_guarded(
            ctx.universe,
            ctx.world,
            &lease,
            sweep_now,
            &[timer.clone()],
        )?,
        1
    );
    assert_eq!(
        ctx.persistence
            .portal_send(
                ctx.universe,
                dest_world,
                sweep_now,
                b"portal-dedupe",
                InboxItem::DomainEvent(aos_fdb::DomainEventIngress {
                    schema: "com.acme/Event@1".into(),
                    value: CborPayload::inline(vec![3]),
                    key: None,
                    correlation_id: None,
                }),
            )?
            .status,
        aos_fdb::PortalSendStatus::Enqueued
    );

    Ok(())
}
