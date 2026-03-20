mod common;

#[path = "../../aos-runtime/tests/helpers.rs"]
mod helpers;

use std::sync::Arc;
use std::time::Duration;

use aos_air_types::{
    AirNode, DefModule, DefSchema, HashRef, NamedRef, TypeExpr, TypeRecord, TypeRef, TypeVariant,
    WorkflowAbi,
};
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_effects::builtins::{
    BlobPutParams, BlobPutReceipt, HttpRequestParams, PortalSendMode, PortalSendParams,
    TimerSetParams, TimerSetReceipt,
};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_fdb::{
    CborPayload, CommandIngress, CommandRecord, CommandStatus, CommandStore, DomainEventIngress,
    FdbWorldPersistence, HostedCoordinationStore, HostedEffectQueueStore, HostedTimerQueueStore,
    InboxItem, NodeCatalog, PersistConflict, PersistenceConfig, SegmentExportRequest, SegmentId,
    SnapshotMaintenanceConfig, WorldAdminLifecycle, WorldAdminStatus, WorldId, WorldIngressStore,
    WorldStore,
};
use aos_kernel::KernelConfig;
use aos_kernel::Store;
use aos_kernel::journal::{EffectReceiptRecord, JournalRecord, OwnedJournalEntry};
use aos_kernel::snapshot::WorkflowStatusSnapshot;
use aos_node::{
    HostedStore, open_hosted_from_manifest_hash, open_hosted_world, snapshot_hosted_world,
};
use aos_node_hosted::config::FdbWorkerConfig;
use aos_node_hosted::{FdbWorker, WorkerSupervisor};
use aos_runtime::{HostError, WorldConfig, WorldHost, now_wallclock_ns};
use aos_wasm_abi::{DomainEvent, PureOutput, WorkflowEffect, WorkflowOutput};
use helpers::fixtures::{self, START_SCHEMA};
use helpers::{def_text_record_schema, insert_test_schemas, text_type};
use indexmap::IndexMap;
use uuid::Uuid;
use wat::parse_str;

#[test]
fn worker_drains_domain_event_inbox_to_journal_and_advances_world()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let store = hosted_store(&ctx);
    seed_hosted_world(&ctx, simple_state_manifest(&store))?;

    ctx.persistence.inbox_enqueue(
        ctx.universe,
        ctx.world,
        InboxItem::DomainEvent(DomainEventIngress {
            schema: START_SCHEMA.into(),
            value: CborPayload::inline(serde_cbor::to_vec(&fixtures::start_event("wf-1"))?),
            key: None,
            correlation_id: Some("test-start".into()),
        }),
    )?;

    let mut supervisor = test_supervisor(&ctx);
    run_until(&mut supervisor, 8, |supervisor| {
        Ok(state_bytes(&ctx, "com.acme/Simple@1")? == Some(vec![0xAA])
            && ctx
                .persistence
                .inbox_cursor(ctx.universe, ctx.world)?
                .is_some()
            && supervisor.active_worlds().is_empty())
    })?;

    Ok(())
}

#[test]
fn worker_executes_http_effect_and_applies_receipt() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let store = hosted_store(&ctx);
    seed_hosted_world(&ctx, workflow_receipt_manifest(&store))?;

    ctx.persistence.inbox_enqueue(
        ctx.universe,
        ctx.world,
        InboxItem::DomainEvent(DomainEventIngress {
            schema: "com.acme/WorkflowEvent@1".into(),
            value: CborPayload::inline(serde_cbor::to_vec(&serde_json::json!({
                "$tag": "Start",
                "$value": fixtures::start_event("wf-1"),
            }))?),
            key: None,
            correlation_id: Some("workflow-start".into()),
        }),
    )?;

    let mut supervisor = test_supervisor(&ctx);
    run_until(&mut supervisor, 12, |supervisor| {
        let workflow_state = state_bytes(&ctx, "com.acme/Workflow@1")?;
        let result_state = state_bytes(&ctx, "com.acme/ResultWorkflow@1")?;
        let info =
            ctx.persistence
                .world_runtime_info(ctx.universe, ctx.world, now_wallclock_ns())?;
        Ok(workflow_state == Some(vec![0x02])
            && result_state == Some(vec![0xEE])
            && !info.has_pending_effects
            && supervisor.active_worlds().is_empty())
    })?;

    Ok(())
}

#[test]
fn worker_persists_and_fires_due_timer() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let store = hosted_store(&ctx);
    seed_hosted_world(&ctx, timer_receipt_workflow_manifest(&store))?;

    ctx.persistence.inbox_enqueue(
        ctx.universe,
        ctx.world,
        InboxItem::DomainEvent(DomainEventIngress {
            schema: "com.acme/TimerWorkflowEvent@1".into(),
            value: CborPayload::inline(serde_cbor::to_vec(&serde_json::json!({
                "$tag": "Start",
                "$value": fixtures::start_event("timer"),
            }))?),
            key: None,
            correlation_id: Some("timer-start".into()),
        }),
    )?;

    let mut supervisor = test_supervisor(&ctx);
    run_until(&mut supervisor, 12, |supervisor| {
        let state = state_bytes(&ctx, "com.acme/TimerWorkflow@1")?;
        let info =
            ctx.persistence
                .world_runtime_info(ctx.universe, ctx.world, now_wallclock_ns())?;
        Ok(state == Some(vec![0xCC])
            && info.next_timer_due_at_ns.is_none()
            && supervisor.active_worlds().is_empty())
    })?;

    Ok(())
}

#[test]
fn worker_runs_snapshot_maintenance_and_exports_cold_segments()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(PersistenceConfig {
        snapshot_maintenance: SnapshotMaintenanceConfig {
            snapshot_after_journal_entries: 1,
            segment_target_entries: 1,
            segment_hot_tail_margin: 0,
            segment_delete_chunk_entries: 16,
        },
        ..common::test_config()
    })?;
    let store = hosted_store(&ctx);
    seed_hosted_world(&ctx, simple_state_manifest(&store))?;

    ctx.persistence.inbox_enqueue(
        ctx.universe,
        ctx.world,
        InboxItem::DomainEvent(DomainEventIngress {
            schema: START_SCHEMA.into(),
            value: CborPayload::inline(serde_cbor::to_vec(&fixtures::start_event("wf-maint"))?),
            key: None,
            correlation_id: Some("snapshot-maintenance".into()),
        }),
    )?;

    let mut supervisor = test_supervisor(&ctx);
    run_until(&mut supervisor, 48, |supervisor| {
        let state = state_bytes(&ctx, "com.acme/Simple@1")?;
        let info =
            ctx.persistence
                .world_runtime_info(ctx.universe, ctx.world, now_wallclock_ns())?;
        let baseline = ctx
            .persistence
            .snapshot_active_baseline(ctx.universe, ctx.world)?;
        let segments = ctx
            .persistence
            .segment_index_read_from(ctx.universe, ctx.world, 0, 8)?;
        Ok(state == Some(vec![0xAA])
            && baseline.height > 0
            && !segments.is_empty()
            && !info.has_pending_maintenance
            && supervisor.active_worlds().is_empty())
    })?;

    Ok(())
}

#[test]
fn hosted_reopen_after_completion_from_stale_baseline_does_not_restore_pending_runtime_work()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let store = hosted_store(&ctx);
    let manifest_hash =
        store_full_manifest(store.as_ref(), &multi_effect_completion_manifest(&store))?;
    let mut host = open_hosted_from_hash_at(&ctx, ctx.world, manifest_hash)?;

    snapshot_hosted(&ctx, ctx.world, &mut host)?;
    host.enqueue_external(aos_runtime::ExternalEvent::DomainEvent {
        schema: "com.acme/MultiEffectEvent@1".into(),
        value: serde_cbor::to_vec(&serde_json::json!({
            "$tag": "Start",
            "$value": fixtures::start_event("wf-reopen"),
        }))?,
        key: None,
    })?;
    host.drain()?;

    let mut intents = host.kernel_mut().drain_effects()?;
    intents.sort_by(|a, b| a.kind.as_str().cmp(b.kind.as_str()));
    assert_eq!(intents.len(), 2);
    assert_eq!(host.kernel().pending_workflow_receipts_snapshot().len(), 2);
    assert_eq!(
        host.kernel().workflow_instances_snapshot()[0].status,
        WorkflowStatusSnapshot::Waiting
    );

    snapshot_hosted(&ctx, ctx.world, &mut host)?;
    let stale_baseline_height = ctx
        .persistence
        .snapshot_active_baseline(ctx.universe, ctx.world)?
        .height;

    for intent in intents {
        let receipt = match intent.kind.as_str() {
            aos_effects::EffectKind::BLOB_PUT => EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: "adapter.blob".into(),
                status: ReceiptStatus::Ok,
                payload_cbor: serde_cbor::to_vec(&BlobPutReceipt {
                    blob_ref: fixtures::fake_hash(0x21),
                    edge_ref: fixtures::fake_hash(0x22),
                    size: 8,
                })?,
                cost_cents: Some(1),
                signature: vec![1, 2, 3],
            },
            aos_effects::EffectKind::TIMER_SET => EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: "adapter.timer".into(),
                status: ReceiptStatus::Ok,
                payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
                    delivered_at_ns: 42,
                    key: Some("wf".into()),
                })?,
                cost_cents: Some(1),
                signature: vec![4, 5, 6],
            },
            other => panic!("unexpected effect kind: {other}"),
        };
        host.enqueue_external(aos_runtime::ExternalEvent::Receipt(receipt))?;
        host.drain()?;
    }

    assert_eq!(
        host.state("com.acme/MultiEffectResult@1", None),
        Some(vec![0xFA])
    );
    assert!(
        host.kernel()
            .pending_workflow_receipts_snapshot()
            .is_empty()
    );
    assert!(host.kernel().queued_effects_snapshot().is_empty());
    let instances = host.kernel().workflow_instances_snapshot();
    let workflow = instances
        .iter()
        .find(|instance| {
            instance
                .instance_id
                .starts_with("com.acme/MultiEffectWorkflow@1::")
        })
        .expect("source workflow instance");
    assert!(workflow.inflight_intents.is_empty());
    assert_eq!(workflow.status, WorkflowStatusSnapshot::Completed);

    ctx.persistence.segment_export(
        ctx.universe,
        ctx.world,
        SegmentExportRequest {
            segment: SegmentId::new(0, stale_baseline_height.saturating_sub(1))?,
            hot_tail_margin: 0,
            delete_chunk_entries: 16,
        },
    )?;
    drop(host);

    let reopened = open_hosted_at(&ctx, ctx.world)?;
    assert_eq!(
        reopened.state("com.acme/MultiEffectResult@1", None),
        Some(vec![0xFA])
    );
    assert!(
        reopened
            .kernel()
            .pending_workflow_receipts_snapshot()
            .is_empty()
    );
    assert!(reopened.kernel().queued_effects_snapshot().is_empty());
    let reopened_instances = reopened.kernel().workflow_instances_snapshot();
    let reopened_workflow = reopened_instances
        .iter()
        .find(|instance| {
            instance
                .instance_id
                .starts_with("com.acme/MultiEffectWorkflow@1::")
        })
        .expect("reopened source workflow instance");
    assert!(reopened_workflow.inflight_intents.is_empty());
    assert_eq!(reopened_workflow.status, WorkflowStatusSnapshot::Completed);

    Ok(())
}

#[test]
#[ignore = "repro for post-segment pending-work resurrection after a duplicate late receipt"]
fn hosted_reopen_after_duplicate_late_receipt_does_not_restore_pending_runtime_work()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let store = hosted_store(&ctx);
    let manifest_hash =
        store_full_manifest(store.as_ref(), &multi_effect_completion_manifest(&store))?;
    let mut host = open_hosted_from_hash_at(&ctx, ctx.world, manifest_hash)?;

    snapshot_hosted(&ctx, ctx.world, &mut host)?;
    host.enqueue_external(aos_runtime::ExternalEvent::DomainEvent {
        schema: "com.acme/MultiEffectEvent@1".into(),
        value: serde_cbor::to_vec(&serde_json::json!({
            "$tag": "Start",
            "$value": fixtures::start_event("wf-late-duplicate"),
        }))?,
        key: None,
    })?;
    host.drain()?;

    let mut intents = host.kernel_mut().drain_effects()?;
    intents.sort_by(|a, b| a.kind.as_str().cmp(b.kind.as_str()));
    snapshot_hosted(&ctx, ctx.world, &mut host)?;
    let stale_baseline_height = ctx
        .persistence
        .snapshot_active_baseline(ctx.universe, ctx.world)?
        .height;

    let mut duplicate_blob_record = None;
    for intent in intents {
        let receipt = match intent.kind.as_str() {
            aos_effects::EffectKind::BLOB_PUT => {
                let receipt = EffectReceipt {
                    intent_hash: intent.intent_hash,
                    adapter_id: "adapter.blob".into(),
                    status: ReceiptStatus::Ok,
                    payload_cbor: serde_cbor::to_vec(&BlobPutReceipt {
                        blob_ref: fixtures::fake_hash(0x31),
                        edge_ref: fixtures::fake_hash(0x32),
                        size: 8,
                    })?,
                    cost_cents: Some(1),
                    signature: vec![1, 2, 3],
                };
                duplicate_blob_record = Some(EffectReceiptRecord {
                    intent_hash: receipt.intent_hash,
                    adapter_id: receipt.adapter_id.clone(),
                    status: receipt.status.clone(),
                    payload_cbor: receipt.payload_cbor.clone(),
                    payload_ref: None,
                    payload_size: None,
                    payload_sha256: None,
                    cost_cents: receipt.cost_cents,
                    signature: receipt.signature.clone(),
                    now_ns: now_wallclock_ns(),
                    logical_now_ns: host.kernel().logical_time_now_ns(),
                    journal_height: 0,
                    entropy: vec![0; 64],
                    manifest_hash: manifest_hash.to_hex(),
                });
                receipt
            }
            aos_effects::EffectKind::TIMER_SET => EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: "adapter.timer".into(),
                status: ReceiptStatus::Ok,
                payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
                    delivered_at_ns: 42,
                    key: Some("wf".into()),
                })?,
                cost_cents: Some(1),
                signature: vec![4, 5, 6],
            },
            other => panic!("unexpected effect kind: {other}"),
        };
        host.enqueue_external(aos_runtime::ExternalEvent::Receipt(receipt))?;
        host.drain()?;
    }

    assert_eq!(
        host.state("com.acme/MultiEffectResult@1", None),
        Some(vec![0xFA])
    );
    assert!(
        host.kernel()
            .pending_workflow_receipts_snapshot()
            .is_empty()
    );
    assert!(host.kernel().queued_effects_snapshot().is_empty());

    let mut duplicate_blob_record = duplicate_blob_record.expect("blob receipt");
    let expected_head = ctx.persistence.journal_head(ctx.universe, ctx.world)?;
    duplicate_blob_record.journal_height = expected_head;
    let duplicate_entry = OwnedJournalEntry {
        seq: expected_head,
        kind: JournalRecord::EffectReceipt(duplicate_blob_record.clone()).kind(),
        payload: serde_cbor::to_vec(&JournalRecord::EffectReceipt(duplicate_blob_record))?,
    };
    ctx.persistence.journal_append_batch(
        ctx.universe,
        ctx.world,
        expected_head,
        &[serde_cbor::to_vec(&duplicate_entry)?],
    )?;

    ctx.persistence.segment_export(
        ctx.universe,
        ctx.world,
        SegmentExportRequest {
            segment: SegmentId::new(0, stale_baseline_height.saturating_sub(1))?,
            hot_tail_margin: 0,
            delete_chunk_entries: 16,
        },
    )?;
    drop(host);

    let reopened = open_hosted_at(&ctx, ctx.world)?;
    assert_eq!(
        reopened.state("com.acme/MultiEffectResult@1", None),
        Some(vec![0xFA])
    );
    assert!(
        reopened
            .kernel()
            .pending_workflow_receipts_snapshot()
            .is_empty()
    );
    assert!(reopened.kernel().queued_effects_snapshot().is_empty());

    Ok(())
}

#[test]
#[ignore = "repro for supervisor maintenance reopening post-finish worlds with duplicate late receipts"]
fn worker_maintenance_reopen_does_not_resurrect_pending_runtime_work_from_duplicate_late_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(PersistenceConfig {
        snapshot_maintenance: SnapshotMaintenanceConfig {
            snapshot_after_journal_entries: 1,
            segment_target_entries: 1,
            segment_hot_tail_margin: 0,
            segment_delete_chunk_entries: 16,
        },
        ..common::test_config()
    })?;
    let store = hosted_store(&ctx);
    let manifest_hash =
        store_full_manifest(store.as_ref(), &multi_effect_completion_manifest(&store))?;
    let mut host = open_hosted_from_hash_at(&ctx, ctx.world, manifest_hash)?;

    snapshot_hosted(&ctx, ctx.world, &mut host)?;
    host.enqueue_external(aos_runtime::ExternalEvent::DomainEvent {
        schema: "com.acme/MultiEffectEvent@1".into(),
        value: serde_cbor::to_vec(&serde_json::json!({
            "$tag": "Start",
            "$value": fixtures::start_event("wf-worker-maint"),
        }))?,
        key: None,
    })?;
    host.drain()?;

    let mut intents = host.kernel_mut().drain_effects()?;
    intents.sort_by(|a, b| a.kind.as_str().cmp(b.kind.as_str()));
    snapshot_hosted(&ctx, ctx.world, &mut host)?;

    let mut duplicate_blob_record = None;
    for intent in intents {
        let receipt = match intent.kind.as_str() {
            aos_effects::EffectKind::BLOB_PUT => {
                let receipt = EffectReceipt {
                    intent_hash: intent.intent_hash,
                    adapter_id: "adapter.blob".into(),
                    status: ReceiptStatus::Ok,
                    payload_cbor: serde_cbor::to_vec(&BlobPutReceipt {
                        blob_ref: fixtures::fake_hash(0x41),
                        edge_ref: fixtures::fake_hash(0x42),
                        size: 8,
                    })?,
                    cost_cents: Some(1),
                    signature: vec![1, 2, 3],
                };
                duplicate_blob_record = Some(EffectReceiptRecord {
                    intent_hash: receipt.intent_hash,
                    adapter_id: receipt.adapter_id.clone(),
                    status: receipt.status.clone(),
                    payload_cbor: receipt.payload_cbor.clone(),
                    payload_ref: None,
                    payload_size: None,
                    payload_sha256: None,
                    cost_cents: receipt.cost_cents,
                    signature: receipt.signature.clone(),
                    now_ns: now_wallclock_ns(),
                    logical_now_ns: host.kernel().logical_time_now_ns(),
                    journal_height: 0,
                    entropy: vec![0; 64],
                    manifest_hash: manifest_hash.to_hex(),
                });
                receipt
            }
            aos_effects::EffectKind::TIMER_SET => EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: "adapter.timer".into(),
                status: ReceiptStatus::Ok,
                payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
                    delivered_at_ns: 42,
                    key: Some("wf".into()),
                })?,
                cost_cents: Some(1),
                signature: vec![4, 5, 6],
            },
            other => panic!("unexpected effect kind: {other}"),
        };
        host.enqueue_external(aos_runtime::ExternalEvent::Receipt(receipt))?;
        host.drain()?;
    }

    assert_eq!(
        host.state("com.acme/MultiEffectResult@1", None),
        Some(vec![0xFA])
    );
    let mut duplicate_blob_record = duplicate_blob_record.expect("blob receipt");
    let expected_head = ctx.persistence.journal_head(ctx.universe, ctx.world)?;
    duplicate_blob_record.journal_height = expected_head;
    let duplicate_entry = OwnedJournalEntry {
        seq: expected_head,
        kind: JournalRecord::EffectReceipt(duplicate_blob_record.clone()).kind(),
        payload: serde_cbor::to_vec(&JournalRecord::EffectReceipt(duplicate_blob_record))?,
    };
    ctx.persistence.journal_append_batch(
        ctx.universe,
        ctx.world,
        expected_head,
        &[serde_cbor::to_vec(&duplicate_entry)?],
    )?;
    drop(host);

    let initial_segments = ctx
        .persistence
        .segment_index_read_from(ctx.universe, ctx.world, 0, 8)?
        .len();
    let mut supervisor = test_supervisor(&ctx);
    let mut saw_segment_export = false;

    for _ in 0..64 {
        supervisor.run_once_blocking()?;
        let debug = supervisor.active_world_debug_state(aos_node_hosted::ActiveWorldRef {
            universe_id: ctx.universe,
            world_id: ctx.world,
        });
        let segment_count = ctx
            .persistence
            .segment_index_read_from(ctx.universe, ctx.world, 0, 8)?
            .len();
        if segment_count > initial_segments {
            saw_segment_export = true;
            if let Some(debug) = debug {
                assert!(
                    debug.pending_receipt_intent_hashes.is_empty(),
                    "pending receipts resurrected after maintenance reopen: {:?}",
                    debug.pending_receipt_intent_hashes
                );
                assert!(
                    debug.queued_effect_intent_hashes.is_empty(),
                    "queued effects resurrected after maintenance reopen: {:?}",
                    debug.queued_effect_intent_hashes
                );
                assert!(
                    debug
                        .workflow_instances
                        .iter()
                        .all(|instance| instance.inflight_intent_hashes.is_empty()),
                    "inflight intents resurrected after maintenance reopen: {:?}",
                    debug.workflow_instances
                );
            }
            break;
        }
    }

    assert!(saw_segment_export, "maintenance did not export a segment");
    Ok(())
}

#[test]
#[ignore = "repro for replaying a late duplicate stage-1 receipt after completion"]
fn hosted_reopen_after_late_stage_one_duplicate_receipt_does_not_restore_followup_effect()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let store = hosted_store(&ctx);
    let manifest_hash = store_full_manifest(store.as_ref(), &late_receipt_repro_manifest(&store))?;
    let mut host = open_hosted_from_hash_at(&ctx, ctx.world, manifest_hash)?;

    snapshot_hosted(&ctx, ctx.world, &mut host)?;
    host.enqueue_external(aos_runtime::ExternalEvent::DomainEvent {
        schema: "com.acme/LateReceiptEvent@1".into(),
        value: serde_cbor::to_vec(&serde_json::json!({
            "$tag": "Start",
            "$value": fixtures::start_event("late"),
        }))?,
        key: None,
    })?;
    host.drain()?;

    let intents = host.kernel_mut().drain_effects()?;
    assert_eq!(intents.len(), 1);
    let stage_one = intents[0].clone();
    snapshot_hosted(&ctx, ctx.world, &mut host)?;
    let stale_baseline_height = ctx
        .persistence
        .snapshot_active_baseline(ctx.universe, ctx.world)?
        .height;

    let receipt = EffectReceipt {
        intent_hash: stage_one.intent_hash,
        adapter_id: "adapter.timer".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
            delivered_at_ns: 42,
            key: Some("late".into()),
        })?,
        cost_cents: Some(1),
        signature: vec![1, 2, 3],
    };
    host.enqueue_external(aos_runtime::ExternalEvent::Receipt(receipt.clone()))?;
    host.drain()?;

    assert_eq!(
        host.state("com.acme/LateReceiptResult@1", None),
        Some(vec![0xFB])
    );
    assert!(
        host.kernel()
            .pending_workflow_receipts_snapshot()
            .is_empty()
    );
    assert!(host.kernel().queued_effects_snapshot().is_empty());

    let expected_head = ctx.persistence.journal_head(ctx.universe, ctx.world)?;
    let late_record = EffectReceiptRecord {
        intent_hash: receipt.intent_hash,
        adapter_id: receipt.adapter_id,
        status: receipt.status,
        payload_cbor: receipt.payload_cbor,
        payload_ref: None,
        payload_size: None,
        payload_sha256: None,
        cost_cents: receipt.cost_cents,
        signature: receipt.signature,
        now_ns: now_wallclock_ns(),
        logical_now_ns: host.kernel().logical_time_now_ns(),
        journal_height: expected_head,
        entropy: vec![0; 64],
        manifest_hash: manifest_hash.to_hex(),
    };
    let late_entry = OwnedJournalEntry {
        seq: expected_head,
        kind: JournalRecord::EffectReceipt(late_record.clone()).kind(),
        payload: serde_cbor::to_vec(&JournalRecord::EffectReceipt(late_record))?,
    };
    ctx.persistence.journal_append_batch(
        ctx.universe,
        ctx.world,
        expected_head,
        &[serde_cbor::to_vec(&late_entry)?],
    )?;
    ctx.persistence.segment_export(
        ctx.universe,
        ctx.world,
        SegmentExportRequest {
            segment: SegmentId::new(0, stale_baseline_height.saturating_sub(1))?,
            hot_tail_margin: 0,
            delete_chunk_entries: 16,
        },
    )?;
    drop(host);

    let reopened = open_hosted_at(&ctx, ctx.world)?;
    assert_eq!(
        reopened.state("com.acme/LateReceiptResult@1", None),
        Some(vec![0xFB])
    );
    assert!(
        reopened
            .kernel()
            .pending_workflow_receipts_snapshot()
            .is_empty()
    );
    assert!(reopened.kernel().queued_effects_snapshot().is_empty());
    let workflow = reopened
        .kernel()
        .workflow_instances_snapshot()
        .into_iter()
        .find(|instance| {
            instance
                .instance_id
                .starts_with("com.acme/LateReceiptWorkflow@1::")
        })
        .expect("reopened source workflow instance");
    assert!(workflow.inflight_intents.is_empty());
    assert_eq!(workflow.status, WorkflowStatusSnapshot::Completed);

    Ok(())
}

#[test]
#[ignore = "repro for supervisor maintenance reopening after a late duplicate stage-1 receipt"]
fn worker_maintenance_reopen_after_late_stage_one_duplicate_receipt_does_not_restore_followup_effect()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(PersistenceConfig {
        snapshot_maintenance: SnapshotMaintenanceConfig {
            snapshot_after_journal_entries: 1,
            segment_target_entries: 1,
            segment_hot_tail_margin: 0,
            segment_delete_chunk_entries: 16,
        },
        ..common::test_config()
    })?;
    let store = hosted_store(&ctx);
    let manifest_hash = store_full_manifest(store.as_ref(), &late_receipt_repro_manifest(&store))?;
    let mut host = open_hosted_from_hash_at(&ctx, ctx.world, manifest_hash)?;

    snapshot_hosted(&ctx, ctx.world, &mut host)?;
    host.enqueue_external(aos_runtime::ExternalEvent::DomainEvent {
        schema: "com.acme/LateReceiptEvent@1".into(),
        value: serde_cbor::to_vec(&serde_json::json!({
            "$tag": "Start",
            "$value": fixtures::start_event("late-worker"),
        }))?,
        key: None,
    })?;
    host.drain()?;

    let intents = host.kernel_mut().drain_effects()?;
    let stage_one = intents.into_iter().next().expect("stage one effect");
    snapshot_hosted(&ctx, ctx.world, &mut host)?;
    let receipt = EffectReceipt {
        intent_hash: stage_one.intent_hash,
        adapter_id: "adapter.timer".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
            delivered_at_ns: 42,
            key: Some("late".into()),
        })?,
        cost_cents: Some(1),
        signature: vec![1, 2, 3],
    };
    host.enqueue_external(aos_runtime::ExternalEvent::Receipt(receipt.clone()))?;
    host.drain()?;
    assert_eq!(
        host.state("com.acme/LateReceiptResult@1", None),
        Some(vec![0xFB])
    );

    let expected_head = ctx.persistence.journal_head(ctx.universe, ctx.world)?;
    let late_record = EffectReceiptRecord {
        intent_hash: receipt.intent_hash,
        adapter_id: receipt.adapter_id,
        status: receipt.status,
        payload_cbor: receipt.payload_cbor,
        payload_ref: None,
        payload_size: None,
        payload_sha256: None,
        cost_cents: receipt.cost_cents,
        signature: receipt.signature,
        now_ns: now_wallclock_ns(),
        logical_now_ns: host.kernel().logical_time_now_ns(),
        journal_height: expected_head,
        entropy: vec![0; 64],
        manifest_hash: manifest_hash.to_hex(),
    };
    let late_entry = OwnedJournalEntry {
        seq: expected_head,
        kind: JournalRecord::EffectReceipt(late_record.clone()).kind(),
        payload: serde_cbor::to_vec(&JournalRecord::EffectReceipt(late_record))?,
    };
    ctx.persistence.journal_append_batch(
        ctx.universe,
        ctx.world,
        expected_head,
        &[serde_cbor::to_vec(&late_entry)?],
    )?;
    drop(host);

    let initial_segments = ctx
        .persistence
        .segment_index_read_from(ctx.universe, ctx.world, 0, 8)?
        .len();
    let mut supervisor = test_supervisor(&ctx);
    let mut saw_segment_export = false;
    for _ in 0..64 {
        supervisor.run_once_blocking()?;
        let debug = supervisor.active_world_debug_state(aos_node_hosted::ActiveWorldRef {
            universe_id: ctx.universe,
            world_id: ctx.world,
        });
        let segment_count = ctx
            .persistence
            .segment_index_read_from(ctx.universe, ctx.world, 0, 8)?
            .len();
        if segment_count > initial_segments {
            saw_segment_export = true;
            if let Some(debug) = debug {
                assert!(
                    debug.pending_receipt_intent_hashes.is_empty(),
                    "pending receipts resurrected after maintenance reopen: {:?}",
                    debug.pending_receipt_intent_hashes
                );
                assert!(
                    debug.queued_effect_intent_hashes.is_empty(),
                    "queued effects resurrected after maintenance reopen: {:?}",
                    debug.queued_effect_intent_hashes
                );
            }
            break;
        }
    }

    assert!(saw_segment_export, "maintenance did not export a segment");
    Ok(())
}

#[test]
fn worker_executes_portal_send_and_delivers_destination_world_event()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let dest_world = WorldId::from(Uuid::new_v4());
    let store = hosted_store(&ctx);
    seed_hosted_world_at(&ctx, ctx.world, portal_sender_manifest(&store, dest_world))?;
    seed_hosted_world_at(&ctx, dest_world, portal_receiver_manifest(&store))?;
    enqueue_portal_start(&ctx, ctx.world, "wf-portal")?;

    let mut supervisor = test_supervisor(&ctx);
    run_until(&mut supervisor, 16, |supervisor| {
        let source_state = state_bytes_at(&ctx, ctx.world, "com.acme/PortalWorkflow@1")?;
        let dest_state = state_bytes_at(&ctx, dest_world, "com.acme/PortalReceiver@1")?;
        let source_info =
            ctx.persistence
                .world_runtime_info(ctx.universe, ctx.world, now_wallclock_ns())?;
        let dest_info =
            ctx.persistence
                .world_runtime_info(ctx.universe, dest_world, now_wallclock_ns())?;
        Ok(source_state == Some(vec![0xC2])
            && dest_state == Some(vec![0xDD])
            && !source_info.has_pending_effects
            && !dest_info.has_pending_inbox
            && supervisor.active_worlds().is_empty())
    })?;

    Ok(())
}

#[test]
fn lease_failover_continues_pending_effect_work() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let store = hosted_store(&ctx);
    seed_hosted_world(&ctx, workflow_receipt_manifest(&store))?;
    enqueue_workflow_start(&ctx, "wf-failover")?;

    let mut cfg_a = worker_config("worker-a");
    cfg_a.max_effects_per_cycle = 0;
    cfg_a.idle_release_after = Duration::from_secs(60);
    let mut supervisor_a = supervisor_with_config(&ctx, cfg_a);
    let first = supervisor_a.run_once_blocking()?;
    assert_eq!(first.worlds_started, 1);
    assert!(
        ctx.persistence
            .world_runtime_info(ctx.universe, ctx.world, now_wallclock_ns())?
            .has_pending_effects
    );

    release_current_lease(&ctx)?;
    expire_worker_heartbeat(&ctx, "worker-a")?;

    let mut supervisor_b = supervisor_with_config(&ctx, worker_config("worker-b"));
    run_until(&mut supervisor_b, 12, |supervisor| {
        let workflow_state = state_bytes(&ctx, "com.acme/Workflow@1")?;
        let result_state = state_bytes(&ctx, "com.acme/ResultWorkflow@1")?;
        let info =
            ctx.persistence
                .world_runtime_info(ctx.universe, ctx.world, now_wallclock_ns())?;
        Ok(workflow_state == Some(vec![0x02])
            && result_state == Some(vec![0xEE])
            && !info.has_pending_effects
            && supervisor.active_worlds().is_empty())
    })?;

    Ok(())
}

#[test]
fn expired_effect_claim_is_requeued_and_recovered() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let store = hosted_store(&ctx);
    seed_hosted_world(&ctx, workflow_receipt_manifest(&store))?;
    enqueue_workflow_start(&ctx, "wf-requeue")?;

    let mut cfg_a = worker_config("worker-a");
    cfg_a.max_effects_per_cycle = 0;
    cfg_a.idle_release_after = Duration::from_secs(60);
    let mut supervisor_a = supervisor_with_config(&ctx, cfg_a);
    supervisor_a.run_once_blocking()?;
    release_current_lease(&ctx)?;

    let claimed = ctx.persistence.claim_pending_effects_for_world(
        ctx.universe,
        ctx.world,
        "crashed-worker",
        0,
        0,
        8,
    )?;
    assert_eq!(claimed.len(), 1);

    let mut supervisor_b = supervisor_with_config(&ctx, worker_config("worker-a"));
    run_until(&mut supervisor_b, 20, |supervisor| {
        let workflow_state = state_bytes(&ctx, "com.acme/Workflow@1")?;
        let result_state = state_bytes(&ctx, "com.acme/ResultWorkflow@1")?;
        let info =
            ctx.persistence
                .world_runtime_info(ctx.universe, ctx.world, now_wallclock_ns())?;
        Ok(workflow_state == Some(vec![0x02])
            && result_state == Some(vec![0xEE])
            && !info.has_pending_effects
            && supervisor.active_worlds().is_empty())
    })?;

    Ok(())
}

#[test]
fn expired_timer_claim_is_requeued_and_recovered() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let store = hosted_store(&ctx);
    seed_hosted_world(&ctx, timer_receipt_workflow_manifest(&store))?;
    enqueue_timer_start(&ctx, "timer-requeue")?;

    let mut cfg_a = worker_config("worker-a");
    cfg_a.max_timers_per_cycle = 0;
    cfg_a.idle_release_after = Duration::from_secs(60);
    let mut supervisor_a = supervisor_with_config(&ctx, cfg_a);
    supervisor_a.run_once_blocking()?;
    release_current_lease(&ctx)?;

    let claimed = ctx.persistence.claim_due_timers_for_world(
        ctx.universe,
        ctx.world,
        "crashed-worker",
        10,
        0,
        8,
    )?;
    assert_eq!(claimed.len(), 1);

    let mut supervisor_b = supervisor_with_config(&ctx, worker_config("worker-a"));
    run_until(&mut supervisor_b, 20, |supervisor| {
        let state = state_bytes(&ctx, "com.acme/TimerWorkflow@1")?;
        let info =
            ctx.persistence
                .world_runtime_info(ctx.universe, ctx.world, now_wallclock_ns())?;
        Ok(state == Some(vec![0xCC])
            && info.next_timer_due_at_ns.is_none()
            && supervisor.active_worlds().is_empty())
    })?;

    Ok(())
}

#[test]
fn pin_change_reassigns_world_to_eligible_worker() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let store = hosted_store(&ctx);
    seed_hosted_world(&ctx, workflow_receipt_manifest(&store))?;
    enqueue_workflow_start(&ctx, "wf-pin")?;

    let mut cfg_a = worker_config("worker-a");
    cfg_a.max_effects_per_cycle = 0;
    cfg_a.idle_release_after = Duration::from_secs(60);
    let mut supervisor_a = supervisor_with_config(&ctx, cfg_a);
    supervisor_a.run_once_blocking()?;
    assert!(
        ctx.persistence
            .world_runtime_info(ctx.universe, ctx.world, now_wallclock_ns())?
            .has_pending_effects
    );

    ctx.persistence
        .set_world_placement_pin(ctx.universe, ctx.world, Some("gpu".into()))?;
    let release = supervisor_a.run_once_blocking()?;
    assert_eq!(release.worlds_released, 1);
    assert!(supervisor_a.active_worlds().is_empty());
    let info_after_release =
        ctx.persistence
            .world_runtime_info(ctx.universe, ctx.world, now_wallclock_ns())?;
    assert_eq!(
        info_after_release.meta.placement_pin.as_deref(),
        Some("gpu")
    );
    assert!(info_after_release.has_pending_effects);

    let mut cfg_b = worker_config("worker-b");
    cfg_b.worker_pins = std::collections::BTreeSet::from(["gpu".to_string()]);
    let mut supervisor_b = supervisor_with_config(&ctx, cfg_b);
    run_until(&mut supervisor_b, 20, |supervisor| {
        let workflow_state = state_bytes(&ctx, "com.acme/Workflow@1")?;
        let result_state = state_bytes(&ctx, "com.acme/ResultWorkflow@1")?;
        let info =
            ctx.persistence
                .world_runtime_info(ctx.universe, ctx.world, now_wallclock_ns())?;
        Ok(workflow_state == Some(vec![0x02])
            && result_state == Some(vec![0xEE])
            && !info.has_pending_effects
            && supervisor.active_worlds().is_empty())
    })?;

    Ok(())
}

#[test]
fn paused_world_rejects_ingress_and_is_not_acquired() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let store = hosted_store(&ctx);
    seed_hosted_world(&ctx, simple_state_manifest(&store))?;
    ctx.persistence.set_world_admin_lifecycle(
        ctx.universe,
        ctx.world,
        WorldAdminLifecycle {
            status: WorldAdminStatus::Paused,
            updated_at_ns: now_wallclock_ns(),
            operation_id: Some("pause-op".into()),
            reason: Some("test pause".into()),
        },
    )?;

    let err = ctx
        .persistence
        .enqueue_ingress(
            ctx.universe,
            ctx.world,
            InboxItem::DomainEvent(DomainEventIngress {
                schema: START_SCHEMA.into(),
                value: CborPayload::inline(serde_cbor::to_vec(&fixtures::start_event("paused"))?),
                key: None,
                correlation_id: Some("paused".into()),
            }),
        )
        .expect_err("paused world rejects ingress");
    assert!(matches!(
        err,
        aos_fdb::PersistError::Conflict(PersistConflict::WorldAdminBlocked { .. })
    ));

    let mut supervisor = test_supervisor(&ctx);
    let outcome = supervisor.run_once_blocking()?;
    assert_eq!(outcome.worlds_started, 0);
    assert!(supervisor.active_worlds().is_empty());
    assert_eq!(
        ctx.persistence
            .world_runtime_info(ctx.universe, ctx.world, now_wallclock_ns())?
            .meta
            .admin
            .status,
        WorldAdminStatus::Paused
    );

    Ok(())
}

#[test]
fn queued_command_survives_lease_failover_and_completes() -> Result<(), Box<dyn std::error::Error>>
{
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let store = hosted_store(&ctx);
    seed_hosted_world(&ctx, simple_state_manifest(&store))?;

    let submitted_at_ns = now_wallclock_ns();
    ctx.persistence.submit_command(
        ctx.universe,
        ctx.world,
        CommandIngress {
            command_id: "cmd-failover".into(),
            command: "world-pause".into(),
            actor: Some("ops".into()),
            payload: CborPayload::inline(serde_cbor::to_vec(&serde_json::json!({
                "reason": "failover test"
            }))?),
            submitted_at_ns,
        },
        CommandRecord {
            command_id: "cmd-failover".into(),
            command: "world-pause".into(),
            status: CommandStatus::Queued,
            submitted_at_ns,
            started_at_ns: None,
            finished_at_ns: None,
            journal_height: None,
            manifest_hash: None,
            result_payload: None,
            error: None,
        },
    )?;

    let mut cfg_a = worker_config("worker-a");
    cfg_a.max_inbox_batch = 0;
    cfg_a.idle_release_after = Duration::from_secs(60);
    let mut supervisor_a = supervisor_with_config(&ctx, cfg_a);
    let first = supervisor_a.run_once_blocking()?;
    assert_eq!(first.worlds_started, 1);
    assert_eq!(
        ctx.persistence
            .command_record(ctx.universe, ctx.world, "cmd-failover")?
            .expect("command record")
            .status,
        CommandStatus::Queued
    );

    release_current_lease(&ctx)?;
    expire_worker_heartbeat(&ctx, "worker-a")?;

    let mut supervisor_b = supervisor_with_config(&ctx, worker_config("worker-b"));
    run_until(&mut supervisor_b, 20, |supervisor| {
        let record = ctx
            .persistence
            .command_record(ctx.universe, ctx.world, "cmd-failover")?
            .expect("command record");
        let status = ctx
            .persistence
            .world_runtime_info(ctx.universe, ctx.world, now_wallclock_ns())?
            .meta
            .admin
            .status;
        Ok(record.status == CommandStatus::Succeeded
            && status == WorldAdminStatus::Paused
            && supervisor.active_worlds().is_empty())
    })?;

    Ok(())
}

fn worker_config(worker_id: &str) -> FdbWorkerConfig {
    FdbWorkerConfig {
        worker_id: worker_id.into(),
        heartbeat_interval: Duration::from_millis(1),
        heartbeat_ttl: Duration::from_secs(5),
        lease_ttl: Duration::from_secs(5),
        lease_renew_interval: Duration::from_secs(30),
        maintenance_idle_after: Duration::ZERO,
        idle_release_after: Duration::ZERO,
        effect_claim_timeout: Duration::from_secs(5),
        timer_claim_timeout: Duration::from_secs(5),
        ready_scan_limit: 32,
        world_scan_limit: 32,
        max_inbox_batch: 8,
        max_tick_steps_per_cycle: 16,
        max_effects_per_cycle: 8,
        max_timers_per_cycle: 8,
        supervisor_poll_interval: Duration::from_millis(1),
        ..FdbWorkerConfig::default()
    }
}

fn test_supervisor(ctx: &common::TestContext) -> WorkerSupervisor<FdbWorldPersistence> {
    supervisor_with_config(&ctx, worker_config("test-worker"))
}

fn supervisor_with_config(
    ctx: &common::TestContext,
    config: FdbWorkerConfig,
) -> WorkerSupervisor<FdbWorldPersistence> {
    FdbWorker::new(config).with_runtime_for_universes(Arc::clone(&ctx.persistence), [ctx.universe])
}

fn run_until<F>(
    supervisor: &mut WorkerSupervisor<FdbWorldPersistence>,
    max_iters: usize,
    mut done: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(&WorkerSupervisor<FdbWorldPersistence>) -> Result<bool, Box<dyn std::error::Error>>,
{
    for _ in 0..max_iters {
        supervisor.run_once_blocking()?;
        if done(supervisor)? {
            return Ok(());
        }
        std::thread::yield_now();
    }
    Err("worker test condition was not reached".into())
}

fn seed_hosted_world(
    ctx: &common::TestContext,
    loaded: aos_kernel::manifest::LoadedManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    seed_hosted_world_at(ctx, ctx.world, loaded)
}

fn seed_hosted_world_at(
    ctx: &common::TestContext,
    world: WorldId,
    loaded: aos_kernel::manifest::LoadedManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    let persistence: Arc<dyn WorldStore> = ctx.persistence.clone();
    let store = Arc::new(HostedStore::new(Arc::clone(&persistence), ctx.universe));
    let manifest_hash = store_full_manifest(store.as_ref(), &loaded)?;
    let mut host = open_hosted_from_hash_at(ctx, world, manifest_hash)?;
    snapshot_hosted(ctx, world, &mut host)?;
    assert!(
        ctx.persistence
            .snapshot_active_baseline(ctx.universe, world)?
            .manifest_hash
            .is_some()
    );
    Ok(())
}

fn hosted_store(ctx: &common::TestContext) -> Arc<HostedStore> {
    let persistence: Arc<dyn WorldStore> = ctx.persistence.clone();
    Arc::new(HostedStore::new(persistence, ctx.universe))
}

fn open_hosted_at(
    ctx: &common::TestContext,
    world: WorldId,
) -> Result<WorldHost<HostedStore>, HostError> {
    let persistence: Arc<dyn WorldStore> = ctx.persistence.clone();
    open_hosted_world(
        persistence,
        ctx.universe,
        world,
        WorldConfig::default(),
        EffectAdapterConfig::default(),
        KernelConfig::default(),
        None,
    )
}

fn open_hosted_from_hash_at(
    ctx: &common::TestContext,
    world: WorldId,
    manifest_hash: aos_cbor::Hash,
) -> Result<WorldHost<HostedStore>, HostError> {
    let persistence: Arc<dyn WorldStore> = ctx.persistence.clone();
    open_hosted_from_manifest_hash(
        persistence,
        ctx.universe,
        world,
        manifest_hash,
        WorldConfig::default(),
        EffectAdapterConfig::default(),
        KernelConfig::default(),
        None,
    )
}

fn snapshot_hosted(
    ctx: &common::TestContext,
    world: WorldId,
    host: &mut WorldHost<HostedStore>,
) -> Result<(), HostError> {
    let persistence: Arc<dyn WorldStore> = ctx.persistence.clone();
    snapshot_hosted_world(host, &persistence, ctx.universe, world)
}

fn enqueue_workflow_start(
    ctx: &common::TestContext,
    id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    enqueue_workflow_start_at(ctx, ctx.world, id)
}

fn enqueue_workflow_start_at(
    ctx: &common::TestContext,
    world: WorldId,
    id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    ctx.persistence.inbox_enqueue(
        ctx.universe,
        world,
        InboxItem::DomainEvent(DomainEventIngress {
            schema: "com.acme/WorkflowEvent@1".into(),
            value: CborPayload::inline(serde_cbor::to_vec(&serde_json::json!({
                "$tag": "Start",
                "$value": fixtures::start_event(id),
            }))?),
            key: None,
            correlation_id: Some(format!("workflow-{id}")),
        }),
    )?;
    Ok(())
}

fn enqueue_portal_start(
    ctx: &common::TestContext,
    world: WorldId,
    id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    ctx.persistence.inbox_enqueue(
        ctx.universe,
        world,
        InboxItem::DomainEvent(DomainEventIngress {
            schema: "com.acme/PortalWorkflowEvent@1".into(),
            value: CborPayload::inline(serde_cbor::to_vec(&serde_json::json!({
                "$tag": "Start",
                "$value": fixtures::start_event(id),
            }))?),
            key: None,
            correlation_id: Some(format!("portal-{id}")),
        }),
    )?;
    Ok(())
}

fn enqueue_timer_start(
    ctx: &common::TestContext,
    id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    ctx.persistence.inbox_enqueue(
        ctx.universe,
        ctx.world,
        InboxItem::DomainEvent(DomainEventIngress {
            schema: "com.acme/TimerWorkflowEvent@1".into(),
            value: CborPayload::inline(serde_cbor::to_vec(&serde_json::json!({
                "$tag": "Start",
                "$value": fixtures::start_event(id),
            }))?),
            key: None,
            correlation_id: Some(format!("timer-{id}")),
        }),
    )?;
    Ok(())
}

fn release_current_lease(ctx: &common::TestContext) -> Result<(), Box<dyn std::error::Error>> {
    let lease = ctx
        .persistence
        .current_world_lease(ctx.universe, ctx.world)?
        .expect("active lease");
    ctx.persistence
        .release_world_lease(ctx.universe, ctx.world, &lease)?;
    Ok(())
}

fn expire_worker_heartbeat(
    ctx: &common::TestContext,
    worker_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    ctx.persistence.heartbeat_worker(aos_fdb::WorkerHeartbeat {
        worker_id: worker_id.to_string(),
        pins: vec!["default".to_string()],
        last_seen_ns: 0,
        expires_at_ns: 0,
    })?;
    Ok(())
}

fn state_bytes(
    ctx: &common::TestContext,
    workflow: &str,
) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
    state_bytes_at(ctx, ctx.world, workflow)
}

fn state_bytes_at(
    ctx: &common::TestContext,
    world: WorldId,
    workflow: &str,
) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
    let persistence: Arc<dyn WorldStore> = ctx.persistence.clone();
    let host = open_hosted_world(
        persistence,
        ctx.universe,
        world,
        WorldConfig::default(),
        EffectAdapterConfig::default(),
        KernelConfig::default(),
        None,
    )?;
    Ok(host.state(workflow, None))
}

fn store_full_manifest<S: Store + ?Sized>(
    store: &S,
    loaded: &aos_kernel::manifest::LoadedManifest,
) -> Result<aos_cbor::Hash, Box<dyn std::error::Error>> {
    let mut manifest = loaded.manifest.clone();
    patch_named_refs(
        "schema",
        &mut manifest.schemas,
        &store_defs(store, loaded.schemas.values(), AirNode::Defschema)?,
    )?;
    patch_named_refs(
        "module",
        &mut manifest.modules,
        &store_defs(store, loaded.modules.values(), AirNode::Defmodule)?,
    )?;
    patch_named_refs("cap", &mut manifest.caps, &std::collections::HashMap::new())?;
    patch_named_refs(
        "effect",
        &mut manifest.effects,
        &std::collections::HashMap::new(),
    )?;
    patch_named_refs(
        "policy",
        &mut manifest.policies,
        &store_defs(store, loaded.policies.values(), AirNode::Defpolicy)?,
    )?;
    Ok(store.put_node(&AirNode::Manifest(manifest))?)
}

fn store_defs<'a, T, S, F>(
    store: &S,
    defs: impl IntoIterator<Item = &'a T>,
    to_node: F,
) -> Result<std::collections::HashMap<String, HashRef>, Box<dyn std::error::Error>>
where
    T: Clone + HasName + 'a,
    S: Store + ?Sized,
    F: Fn(T) -> AirNode,
{
    let mut hashes = std::collections::HashMap::new();
    for def in defs {
        let def = def.clone();
        let hash = store.put_node(&to_node(def.clone()))?;
        hashes.insert(
            def.name().to_string(),
            HashRef::new(hash.to_hex()).expect("hash ref"),
        );
    }
    Ok(hashes)
}

fn patch_named_refs(
    kind: &str,
    refs: &mut Vec<NamedRef>,
    hashes: &std::collections::HashMap<String, HashRef>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in refs {
        let actual = if let Some(hash) = hashes.get(entry.name.as_str()) {
            hash.clone()
        } else if let Some(builtin) =
            aos_air_types::builtins::find_builtin_schema(entry.name.as_str())
        {
            builtin.hash_ref.clone()
        } else if kind == "effect" {
            aos_air_types::builtins::find_builtin_effect(entry.name.as_str())
                .map(|builtin| builtin.hash_ref.clone())
                .ok_or_else(|| format!("manifest references unknown effect '{}'", entry.name))?
        } else if kind == "module" {
            aos_air_types::builtins::find_builtin_module(entry.name.as_str())
                .map(|builtin| builtin.hash_ref.clone())
                .ok_or_else(|| format!("manifest references unknown module '{}'", entry.name))?
        } else if kind == "cap" {
            aos_air_types::builtins::find_builtin_cap(entry.name.as_str())
                .map(|builtin| builtin.hash_ref.clone())
                .ok_or_else(|| format!("manifest references unknown cap '{}'", entry.name))?
        } else {
            return Err(format!("manifest references unknown {kind} '{}'", entry.name).into());
        };
        entry.hash = actual;
    }
    Ok(())
}

trait HasName {
    fn name(&self) -> &str;
}

impl HasName for aos_air_types::DefSchema {
    fn name(&self) -> &str {
        &self.name
    }
}

impl HasName for aos_air_types::DefModule {
    fn name(&self) -> &str {
        &self.name
    }
}

impl HasName for aos_air_types::DefPolicy {
    fn name(&self) -> &str {
        &self.name
    }
}

fn simple_state_manifest<S: Store + ?Sized>(
    store: &Arc<S>,
) -> aos_kernel::manifest::LoadedManifest {
    let mut workflow = fixtures::stub_workflow_module(
        store,
        "com.acme/Simple@1",
        &WorkflowOutput {
            state: Some(vec![0xAA]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/SimpleState@1"),
        event: fixtures::schema(START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow],
        vec![fixtures::routing_event(START_SCHEMA, "com.acme/Simple@1")],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/SimpleState@1".into(),
                ty: text_type(),
            },
        ],
    );
    loaded
}

fn allow_http_enforcer<S: Store + ?Sized>(store: &Arc<S>) -> DefModule {
    let allow_output = aos_kernel::cap_enforcer::CapCheckOutput {
        constraints_ok: true,
        deny: None,
    };
    let output_bytes = serde_cbor::to_vec(&allow_output).expect("encode cap output");
    let pure_output = PureOutput {
        output: output_bytes,
    };
    fixtures::stub_pure_module(
        store,
        "sys/CapEnforceHttpOut@1",
        &pure_output,
        "sys/CapCheckInput@1",
        "sys/CapCheckOutput@1",
    )
}

fn build_loaded_manifest_with_http_enforcer<S: Store + ?Sized>(
    store: &Arc<S>,
    mut modules: Vec<DefModule>,
    routing: Vec<aos_air_types::RoutingEvent>,
) -> aos_kernel::manifest::LoadedManifest {
    if !modules
        .iter()
        .any(|module| module.name == "sys/CapEnforceHttpOut@1")
    {
        modules.push(allow_http_enforcer(store));
    }
    fixtures::build_loaded_manifest(modules, routing)
}

fn workflow_receipt_manifest<S: Store + ?Sized>(
    store: &Arc<S>,
) -> aos_kernel::manifest::LoadedManifest {
    let start_output = WorkflowOutput {
        state: Some(vec![0x01]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::HTTP_REQUEST,
            serde_cbor::to_vec(&HttpRequestParams {
                method: "GET".into(),
                url: "https://example.com/workflow".into(),
                headers: Default::default(),
                body_ref: None,
            })
            .expect("encode http params"),
            "http",
        )],
        ann: None,
    };
    let receipt_output = WorkflowOutput {
        state: Some(vec![0x02]),
        domain_events: vec![DomainEvent::new(
            "com.acme/WorkflowDone@1".to_string(),
            serde_cbor::to_vec(&serde_json::json!({ "id": "wf-1" }))
                .expect("encode completion event"),
        )],
        effects: vec![],
        ann: None,
    };

    let mut workflow =
        sequenced_workflow_module(store, "com.acme/Workflow@1", &start_output, &receipt_output);
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/WorkflowState@1"),
        event: fixtures::schema("com.acme/WorkflowEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::HTTP_REQUEST.into()],
        cap_slots: Default::default(),
    });

    let mut result_module = fixtures::stub_workflow_module(
        store,
        "com.acme/ResultWorkflow@1",
        &WorkflowOutput {
            state: Some(vec![0xEE]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    result_module.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/ResultState@1"),
        event: fixtures::schema("com.acme/WorkflowDone@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: IndexMap::new(),
    });

    let mut loaded = build_loaded_manifest_with_http_enforcer(
        store,
        vec![workflow, result_module],
        vec![
            fixtures::routing_event("com.acme/WorkflowEvent@1", "com.acme/Workflow@1"),
            fixtures::routing_event("com.acme/WorkflowDone@1", "com.acme/ResultWorkflow@1"),
        ],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/WorkflowEvent@1".into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(START_SCHEMA),
                            }),
                        ),
                        (
                            "Receipt".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("sys/EffectReceiptEnvelope@1"),
                            }),
                        ),
                    ]),
                }),
            },
            def_text_record_schema("com.acme/WorkflowDone@1", vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/WorkflowState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            DefSchema {
                name: "com.acme/ResultState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );
    loaded
        .manifest
        .module_bindings
        .get_mut("com.acme/Workflow@1")
        .expect("workflow binding")
        .slots
        .insert("http".into(), "cap_http".into());
    loaded
}

fn multi_effect_completion_manifest<S: Store + ?Sized>(
    store: &Arc<S>,
) -> aos_kernel::manifest::LoadedManifest {
    let start_output = WorkflowOutput {
        state: Some(vec![0xDE, 0xAD, 0xBE, 0xEF]),
        domain_events: vec![],
        effects: vec![
            WorkflowEffect::new(
                aos_effects::EffectKind::TIMER_SET,
                serde_cbor::to_vec(&TimerSetParams {
                    deliver_at_ns: 42,
                    key: Some("wf".into()),
                })
                .expect("encode timer params"),
            ),
            WorkflowEffect::with_cap_slot(
                aos_effects::EffectKind::BLOB_PUT,
                serde_cbor::to_vec(&BlobPutParams {
                    bytes: b"workflow".to_vec(),
                    blob_ref: None,
                    refs: None,
                })
                .expect("encode blob.put params"),
                "blob",
            ),
        ],
        ann: None,
    };
    let receipt_output = WorkflowOutput {
        state: None,
        domain_events: vec![DomainEvent::new(
            "com.acme/MultiEffectDone@1".to_string(),
            serde_cbor::to_vec(&serde_json::json!({ "id": "wf-reopen" }))
                .expect("encode completion event"),
        )],
        effects: vec![],
        ann: None,
    };

    let mut workflow = sequenced_workflow_module(
        store,
        "com.acme/MultiEffectWorkflow@1",
        &start_output,
        &receipt_output,
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/MultiEffectState@1"),
        event: fixtures::schema("com.acme/MultiEffectEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![
            aos_effects::EffectKind::TIMER_SET.into(),
            aos_effects::EffectKind::BLOB_PUT.into(),
        ],
        cap_slots: Default::default(),
    });

    let mut result_module = fixtures::stub_workflow_module(
        store,
        "com.acme/MultiEffectResult@1",
        &WorkflowOutput {
            state: Some(vec![0xFA]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    result_module.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/MultiEffectResultState@1"),
        event: fixtures::schema("com.acme/MultiEffectDone@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow, result_module],
        vec![
            fixtures::routing_event(
                "com.acme/MultiEffectEvent@1",
                "com.acme/MultiEffectWorkflow@1",
            ),
            fixtures::routing_event("com.acme/MultiEffectDone@1", "com.acme/MultiEffectResult@1"),
        ],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/MultiEffectEvent@1".into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(START_SCHEMA),
                            }),
                        ),
                        (
                            "Receipt".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("sys/EffectReceiptEnvelope@1"),
                            }),
                        ),
                    ]),
                }),
            },
            def_text_record_schema("com.acme/MultiEffectDone@1", vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/MultiEffectState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            DefSchema {
                name: "com.acme/MultiEffectResultState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );
    loaded
        .manifest
        .module_bindings
        .get_mut("com.acme/MultiEffectWorkflow@1")
        .expect("multi-effect workflow binding")
        .slots
        .insert("blob".into(), "blob_cap".into());
    loaded
}

fn late_receipt_repro_manifest<S: Store + ?Sized>(
    store: &Arc<S>,
) -> aos_kernel::manifest::LoadedManifest {
    let start_output = WorkflowOutput {
        state: Some(vec![0xA1]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::new(
            aos_effects::EffectKind::TIMER_SET,
            serde_cbor::to_vec(&TimerSetParams {
                deliver_at_ns: 42,
                key: Some("late".into()),
            })
            .expect("encode timer params"),
        )],
        ann: None,
    };
    let receipt_output = WorkflowOutput {
        state: Some(vec![0xA2]),
        domain_events: vec![DomainEvent::new(
            "com.acme/LateReceiptEvent@1".to_string(),
            serde_cbor::to_vec(&serde_json::json!({
                "$tag": "Complete",
                "$value": { "id": "late" }
            }))
            .expect("encode complete event"),
        )],
        effects: vec![WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::BLOB_PUT,
            serde_cbor::to_vec(&BlobPutParams {
                bytes: b"late-receipt".to_vec(),
                blob_ref: None,
                refs: None,
            })
            .expect("encode blob.put params"),
            "blob",
        )],
        ann: None,
    };
    let complete_output = WorkflowOutput {
        state: None,
        domain_events: vec![DomainEvent::new(
            "com.acme/LateReceiptDone@1".to_string(),
            serde_cbor::to_vec(&serde_json::json!({ "id": "late" })).expect("encode done event"),
        )],
        effects: vec![],
        ann: None,
    };

    let mut workflow = branched_workflow_module(
        store,
        "com.acme/LateReceiptWorkflow@1",
        &start_output,
        &receipt_output,
        &complete_output,
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/LateReceiptState@1"),
        event: fixtures::schema("com.acme/LateReceiptEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![
            aos_effects::EffectKind::TIMER_SET.into(),
            aos_effects::EffectKind::BLOB_PUT.into(),
        ],
        cap_slots: Default::default(),
    });

    let mut result_module = fixtures::stub_workflow_module(
        store,
        "com.acme/LateReceiptResult@1",
        &WorkflowOutput {
            state: Some(vec![0xFB]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    result_module.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/LateReceiptResultState@1"),
        event: fixtures::schema("com.acme/LateReceiptDone@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow, result_module],
        vec![
            fixtures::routing_event(
                "com.acme/LateReceiptEvent@1",
                "com.acme/LateReceiptWorkflow@1",
            ),
            fixtures::routing_event("com.acme/LateReceiptDone@1", "com.acme/LateReceiptResult@1"),
        ],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/LateReceiptEvent@1".into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(START_SCHEMA),
                            }),
                        ),
                        (
                            "Receipt".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("sys/EffectReceiptEnvelope@1"),
                            }),
                        ),
                        (
                            "Complete".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("com.acme/LateReceiptComplete@1"),
                            }),
                        ),
                    ]),
                }),
            },
            def_text_record_schema("com.acme/LateReceiptComplete@1", vec![("id", text_type())]),
            def_text_record_schema("com.acme/LateReceiptDone@1", vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/LateReceiptState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            DefSchema {
                name: "com.acme/LateReceiptResultState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );
    loaded
        .manifest
        .module_bindings
        .get_mut("com.acme/LateReceiptWorkflow@1")
        .expect("late receipt workflow binding")
        .slots
        .insert("blob".into(), "blob_cap".into());
    loaded
}

fn timer_receipt_workflow_manifest<S: Store + ?Sized>(
    store: &Arc<S>,
) -> aos_kernel::manifest::LoadedManifest {
    let start_output = WorkflowOutput {
        state: Some(vec![0x01]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::new(
            aos_effects::EffectKind::TIMER_SET,
            serde_cbor::to_vec(&TimerSetParams {
                deliver_at_ns: 10,
                key: Some("retry".into()),
            })
            .expect("encode timer params"),
        )],
        ann: None,
    };
    let receipt_output = WorkflowOutput {
        state: Some(vec![0xCC]),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };
    let mut workflow = sequenced_workflow_module(
        store,
        "com.acme/TimerWorkflow@1",
        &start_output,
        &receipt_output,
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/TimerWorkflowState@1"),
        event: fixtures::schema("com.acme/TimerWorkflowEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::TIMER_SET.into()],
        cap_slots: Default::default(),
    });
    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow],
        vec![fixtures::routing_event(
            "com.acme/TimerWorkflowEvent@1",
            "com.acme/TimerWorkflow@1",
        )],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/TimerWorkflowEvent@1".into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(START_SCHEMA),
                            }),
                        ),
                        (
                            "Receipt".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("sys/EffectReceiptEnvelope@1"),
                            }),
                        ),
                    ]),
                }),
            },
            DefSchema {
                name: "com.acme/TimerWorkflowState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );
    loaded
}

fn portal_sender_manifest<S: Store + ?Sized>(
    store: &Arc<S>,
    dest_world: WorldId,
) -> aos_kernel::manifest::LoadedManifest {
    let start_output = WorkflowOutput {
        state: Some(vec![0xC1]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::PORTAL_SEND,
            serde_cbor::to_vec(&PortalSendParams {
                dest_universe: None,
                dest_world: dest_world.to_string(),
                mode: PortalSendMode::TypedEvent,
                schema: Some("com.acme/PortalEvent@1".into()),
                value_cbor: Some(
                    serde_cbor::to_vec(&serde_json::json!({ "id": "ported" }))
                        .expect("encode portal event"),
                ),
                inbox: None,
                payload_cbor: None,
                headers: None,
                correlation_id: Some("portal-corr".into()),
            })
            .expect("encode portal params"),
            "portal",
        )],
        ann: None,
    };
    let receipt_output = WorkflowOutput {
        state: Some(vec![0xC2]),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };

    let mut workflow = sequenced_workflow_module(
        store,
        "com.acme/PortalWorkflow@1",
        &start_output,
        &receipt_output,
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/PortalWorkflowState@1"),
        event: fixtures::schema("com.acme/PortalWorkflowEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::PORTAL_SEND.into()],
        cap_slots: Default::default(),
    });

    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow],
        vec![fixtures::routing_event(
            "com.acme/PortalWorkflowEvent@1",
            "com.acme/PortalWorkflow@1",
        )],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/PortalWorkflowEvent@1".into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(START_SCHEMA),
                            }),
                        ),
                        (
                            "Receipt".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("sys/EffectReceiptEnvelope@1"),
                            }),
                        ),
                    ]),
                }),
            },
            DefSchema {
                name: "com.acme/PortalWorkflowState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            def_text_record_schema("com.acme/PortalEvent@1", vec![("id", text_type())]),
        ],
    );
    loaded.manifest.caps.push(NamedRef {
        name: "sys/portal@1".into(),
        hash: fixtures::zero_hash(),
    });
    if let Some(defaults) = loaded.manifest.defaults.as_mut() {
        defaults.cap_grants.push(aos_air_types::CapGrant {
            name: "cap_portal".into(),
            cap: "sys/portal@1".into(),
            params: fixtures::empty_value_literal(),
            expiry_ns: None,
        });
    } else {
        loaded.manifest.defaults = Some(aos_air_types::ManifestDefaults {
            policy: None,
            cap_grants: vec![aos_air_types::CapGrant {
                name: "cap_portal".into(),
                cap: "sys/portal@1".into(),
                params: fixtures::empty_value_literal(),
                expiry_ns: None,
            }],
        });
    }
    loaded
        .manifest
        .module_bindings
        .get_mut("com.acme/PortalWorkflow@1")
        .expect("portal workflow binding")
        .slots
        .insert("portal".into(), "cap_portal".into());
    loaded
}

fn portal_receiver_manifest<S: Store + ?Sized>(
    store: &Arc<S>,
) -> aos_kernel::manifest::LoadedManifest {
    let mut workflow = fixtures::stub_workflow_module(
        store,
        "com.acme/PortalReceiver@1",
        &WorkflowOutput {
            state: Some(vec![0xDD]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/PortalReceiverState@1"),
        event: fixtures::schema("com.acme/PortalEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow],
        vec![fixtures::routing_event(
            "com.acme/PortalEvent@1",
            "com.acme/PortalReceiver@1",
        )],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema("com.acme/PortalEvent@1", vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/PortalReceiverState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );
    loaded
}

fn sequenced_workflow_module<S: Store + ?Sized>(
    store: &Arc<S>,
    name: impl Into<String>,
    first: &WorkflowOutput,
    then: &WorkflowOutput,
) -> DefModule {
    let first_bytes = first.encode().expect("encode first workflow output");
    let then_bytes = then.encode().expect("encode second workflow output");
    let first_literal = first_bytes
        .iter()
        .map(|byte| format!("\\{:02x}", byte))
        .collect::<String>();
    let then_literal = then_bytes
        .iter()
        .map(|byte| format!("\\{:02x}", byte))
        .collect::<String>();
    let first_len = first_bytes.len();
    let then_len = then_bytes.len();
    let second_offset = first_len;
    let heap_start = first_len + then_len;
    let wat = format!(
        r#"(module
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const {heap_start}))
  (data (i32.const 0) "{first_literal}")
  (data (i32.const {second_offset}) "{then_literal}")
  (func (export "alloc") (param i32) (result i32)
    (local $old i32)
    global.get $heap
    local.tee $old
    local.get 0
    i32.add
    global.set $heap
    local.get $old)
  (func $is_receipt_event (param $ptr i32) (param $len i32) (result i32)
    (local $i i32)
    (block $not_found
      (loop $search
        local.get $i
        i32.const 6
        i32.add
        local.get $len
        i32.ge_u
        br_if $not_found

        local.get $ptr
        local.get $i
        i32.add
        i32.load8_u
        i32.const 82
        i32.eq
        if
          local.get $ptr
          local.get $i
          i32.add
          i32.const 1
          i32.add
          i32.load8_u
          i32.const 101
          i32.eq
          if
            local.get $ptr
            local.get $i
            i32.add
            i32.const 2
            i32.add
            i32.load8_u
            i32.const 99
            i32.eq
            if
              local.get $ptr
              local.get $i
              i32.add
              i32.const 3
              i32.add
              i32.load8_u
              i32.const 101
              i32.eq
              if
                local.get $ptr
                local.get $i
                i32.add
                i32.const 4
                i32.add
                i32.load8_u
                i32.const 105
                i32.eq
                if
                  local.get $ptr
                  local.get $i
                  i32.add
                  i32.const 5
                  i32.add
                  i32.load8_u
                  i32.const 112
                  i32.eq
                  if
                    local.get $ptr
                    local.get $i
                    i32.add
                    i32.const 6
                    i32.add
                    i32.load8_u
                    i32.const 116
                    i32.eq
                    if
                      i32.const 1
                      return
                    end
                  end
                end
              end
            end
          end
        end

        local.get $i
        i32.const 1
        i32.add
        local.set $i
        br $search
      )
    )
    i32.const 0
  )
  (func (export "step") (param i32 i32) (result i32 i32)
    local.get 0
    local.get 1
    call $is_receipt_event
    if (result i32 i32)
      (i32.const {second_offset})
      (i32.const {then_len})
    else
      (i32.const 0)
      (i32.const {first_len})
    end)
)"#
    );
    let wasm_bytes = parse_str(&wat).expect("wat compile");
    let wasm_hash = store.put_blob(&wasm_bytes).expect("store wasm");

    DefModule {
        name: name.into(),
        module_kind: aos_air_types::ModuleKind::Workflow,
        wasm_hash: aos_air_types::HashRef::new(wasm_hash.to_hex()).expect("hash ref"),
        key_schema: None,
        abi: aos_air_types::ModuleAbi {
            workflow: None,
            pure: None,
        },
    }
}

fn branched_workflow_module<S: Store + ?Sized>(
    store: &Arc<S>,
    name: impl Into<String>,
    start: &WorkflowOutput,
    receipt: &WorkflowOutput,
    complete: &WorkflowOutput,
) -> DefModule {
    let start_bytes = start.encode().expect("encode start workflow output");
    let receipt_bytes = receipt.encode().expect("encode receipt workflow output");
    let complete_bytes = complete.encode().expect("encode complete workflow output");
    let start_literal = start_bytes
        .iter()
        .map(|byte| format!("\\{:02x}", byte))
        .collect::<String>();
    let receipt_literal = receipt_bytes
        .iter()
        .map(|byte| format!("\\{:02x}", byte))
        .collect::<String>();
    let complete_literal = complete_bytes
        .iter()
        .map(|byte| format!("\\{:02x}", byte))
        .collect::<String>();
    let start_len = start_bytes.len();
    let receipt_len = receipt_bytes.len();
    let complete_len = complete_bytes.len();
    let receipt_offset = start_len;
    let complete_offset = start_len + receipt_len;
    let heap_start = start_len + receipt_len + complete_len;
    let wat = format!(
        r#"(module
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const {heap_start}))
  (data (i32.const 0) "{start_literal}")
  (data (i32.const {receipt_offset}) "{receipt_literal}")
  (data (i32.const {complete_offset}) "{complete_literal}")
  (func (export "alloc") (param i32) (result i32)
    (local $old i32)
    global.get $heap
    local.tee $old
    local.get 0
    i32.add
    global.set $heap
    local.get $old)
  (func $contains (param $ptr i32) (param $len i32) (param $match0 i32) (param $match1 i32) (param $match2 i32) (param $match3 i32) (param $match4 i32) (param $match5 i32) (param $match6 i32) (result i32)
    (local $i i32)
    (block $not_found
      (loop $search
        local.get $i
        i32.const 6
        i32.add
        local.get $len
        i32.ge_u
        br_if $not_found
        local.get $ptr
        local.get $i
        i32.add
        i32.load8_u
        local.get $match0
        i32.eq
        if
          local.get $ptr
          local.get $i
          i32.add
          i32.const 1
          i32.add
          i32.load8_u
          local.get $match1
          i32.eq
          if
            local.get $ptr
            local.get $i
            i32.add
            i32.const 2
            i32.add
            i32.load8_u
            local.get $match2
            i32.eq
            if
              local.get $ptr
              local.get $i
              i32.add
              i32.const 3
              i32.add
              i32.load8_u
              local.get $match3
              i32.eq
              if
                local.get $ptr
                local.get $i
                i32.add
                i32.const 4
                i32.add
                i32.load8_u
                local.get $match4
                i32.eq
                if
                  local.get $ptr
                  local.get $i
                  i32.add
                  i32.const 5
                  i32.add
                  i32.load8_u
                  local.get $match5
                  i32.eq
                  if
                    local.get $ptr
                    local.get $i
                    i32.add
                    i32.const 6
                    i32.add
                    i32.load8_u
                    local.get $match6
                    i32.eq
                    if
                      i32.const 1
                      return
                    end
                  end
                end
              end
            end
          end
        end
        local.get $i
        i32.const 1
        i32.add
        local.set $i
        br $search
      )
    )
    i32.const 0
  )
  (func (export "step") (param i32 i32) (result i32 i32)
    local.get 0
    local.get 1
    i32.const 67
    i32.const 111
    i32.const 109
    i32.const 112
    i32.const 108
    i32.const 101
    i32.const 116
    call $contains
    if (result i32 i32)
      (i32.const {complete_offset})
      (i32.const {complete_len})
    else
      local.get 0
      local.get 1
      i32.const 82
      i32.const 101
      i32.const 99
      i32.const 101
      i32.const 105
      i32.const 112
      i32.const 116
      call $contains
      if (result i32 i32)
        (i32.const {receipt_offset})
        (i32.const {receipt_len})
      else
        (i32.const 0)
        (i32.const {start_len})
      end
    end)
)"#
    );
    let wasm_bytes = parse_str(&wat).expect("wat compile");
    let wasm_hash = store.put_blob(&wasm_bytes).expect("store wasm");

    DefModule {
        name: name.into(),
        module_kind: aos_air_types::ModuleKind::Workflow,
        wasm_hash: aos_air_types::HashRef::new(wasm_hash.to_hex()).expect("hash ref"),
        key_schema: None,
        abi: aos_air_types::ModuleAbi {
            workflow: None,
            pure: None,
        },
    }
}
