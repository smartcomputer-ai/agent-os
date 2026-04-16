use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use aos_cbor::{HASH_PREFIX, Hash};
use aos_effect_types::TimerSetParams;
use aos_effect_types::TimerSetReceipt;
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use aos_kernel::journal::{
    Journal, JournalRecord, OwnedJournalEntry, SnapshotRecord as KernelSnapshotRecord,
};
use aos_kernel::{Kernel, KernelConfig, LoadedManifest, Store};
use aos_node::{
    BackendError, CborPayload, SnapshotRecord, TimerEntry, WorldLogFrame, WorldRuntimeInfo,
};
use uuid::Uuid;

use crate::blobstore::HostedCas;

use super::types::{ActiveWorld, AsyncWorldState, WorkerError};

pub(super) fn resolve_cbor_payload<S: Store>(
    store: &S,
    payload: &CborPayload,
) -> Result<Vec<u8>, WorkerError> {
    payload.validate()?;
    if let Some(inline) = &payload.inline_cbor {
        return Ok(inline.clone());
    }

    let Some(hash_ref) = payload.cbor_ref.as_deref() else {
        return Err(WorkerError::LogFirst(BackendError::InvalidHashRef(
            "<missing>".into(),
        )));
    };
    Ok(store.get_blob(parse_hash_ref(hash_ref)?)?)
}

pub(super) fn parse_hash_ref(value: &str) -> Result<Hash, WorkerError> {
    let normalized = if value.starts_with(HASH_PREFIX) {
        value.to_owned()
    } else {
        format!("{HASH_PREFIX}{value}")
    };
    Hash::from_hex_str(&normalized)
        .map_err(|_| WorkerError::LogFirst(BackendError::InvalidHashRef(value.to_owned())))
}

pub(super) fn default_state_root() -> Result<PathBuf, WorkerError> {
    if let Ok(raw) = std::env::var("AOS_STATE_ROOT") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    Ok(std::env::current_dir()
        .map_err(|err| WorkerError::Persist(aos_node::PersistError::backend(err.to_string())))?
        .join(".aos-node"))
}

pub(super) fn temp_embedded_state_root() -> PathBuf {
    std::env::temp_dir().join(format!("aos-node-{}", Uuid::new_v4()))
}

pub(super) fn latest_snapshot_record(
    entries: &[OwnedJournalEntry],
) -> Option<aos_kernel::journal::SnapshotRecord> {
    entries.iter().rev().find_map(|entry| {
        match serde_cbor::from_slice::<JournalRecord>(&entry.payload) {
            Ok(JournalRecord::Snapshot(record)) => Some(record),
            _ => None,
        }
    })
}

pub(super) fn snapshot_record_from_checkpoint(
    baseline: &aos_node::PromotableBaselineRef,
) -> SnapshotRecord {
    SnapshotRecord {
        snapshot_ref: baseline.snapshot_ref.clone(),
        height: baseline.height,
        universe_id: baseline.universe_id,
        logical_time_ns: baseline.logical_time_ns,
        receipt_horizon_height: Some(baseline.receipt_horizon_height),
        manifest_hash: Some(baseline.manifest_hash.clone()),
    }
}

pub(super) fn snapshot_record_from_frames(
    frames: &[WorldLogFrame],
    mut predicate: impl FnMut(&SnapshotRecord) -> bool,
) -> Option<SnapshotRecord> {
    for frame in frames.iter().rev() {
        for record in frame.records.iter().rev() {
            let JournalRecord::Snapshot(snapshot) = record else {
                continue;
            };
            let candidate = SnapshotRecord {
                snapshot_ref: snapshot.snapshot_ref.clone(),
                height: snapshot.height,
                universe_id: snapshot.universe_id.into(),
                logical_time_ns: snapshot.logical_time_ns,
                receipt_horizon_height: snapshot.receipt_horizon_height,
                manifest_hash: snapshot.manifest_hash.clone(),
            };
            if predicate(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

pub(super) fn journal_entries_from_world_frames(
    frames: &[WorldLogFrame],
) -> Result<Vec<aos_kernel::journal::OwnedJournalEntry>, WorkerError> {
    let mut entries = Vec::new();
    for frame in frames {
        for (offset, record) in frame.records.iter().enumerate() {
            entries.push(aos_kernel::journal::OwnedJournalEntry {
                seq: frame.world_seq_start + offset as u64,
                kind: record.kind(),
                payload: serde_cbor::to_vec(record)?,
            });
        }
    }
    Ok(entries)
}

pub(super) fn kernel_snapshot_record(snapshot: &SnapshotRecord) -> KernelSnapshotRecord {
    KernelSnapshotRecord {
        snapshot_ref: snapshot.snapshot_ref.clone(),
        height: snapshot.height,
        universe_id: snapshot.universe_id.as_uuid(),
        logical_time_ns: snapshot.logical_time_ns,
        receipt_horizon_height: snapshot.receipt_horizon_height,
        manifest_hash: snapshot.manifest_hash.clone(),
    }
}

pub(super) fn reopen_kernel_from_frame_log(
    store: std::sync::Arc<HostedCas>,
    loaded: LoadedManifest,
    active_baseline: &SnapshotRecord,
    frames: &[WorldLogFrame],
    kernel_config: KernelConfig,
) -> Result<Kernel<HostedCas>, WorkerError> {
    if frames.is_empty() {
        let mut kernel = Kernel::from_loaded_manifest_without_replay_with_config(
            store,
            loaded,
            Journal::new(),
            kernel_config,
        )?;
        kernel.restore_snapshot_record(&kernel_snapshot_record(active_baseline))?;
        kernel.compact_journal_through(active_baseline.height)?;
        return Ok(kernel);
    }

    let replay_entries = journal_entries_from_world_frames(frames)?;
    let replay_from = replay_entries.first().map(|entry| entry.seq).unwrap_or(0);
    let journal =
        Journal::from_entries(&replay_entries).map_err(|err| WorkerError::Build(err.into()))?;
    let mut kernel = Kernel::from_loaded_manifest_without_replay_with_config(
        store,
        loaded,
        journal,
        kernel_config,
    )?;
    kernel.restore_snapshot_record(&kernel_snapshot_record(active_baseline))?;
    kernel.replay_entries_from(replay_from)?;
    kernel.compact_journal_through(active_baseline.height)?;
    Ok(kernel)
}

pub(super) fn build_timer_receipt(entry: &TimerEntry) -> Result<EffectReceipt, WorkerError> {
    Ok(EffectReceipt {
        intent_hash: entry.intent_hash,
        adapter_id: "timer.local".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
            delivered_at_ns: entry.deliver_at_ns,
            key: entry.key.clone(),
        })?,
        cost_cents: None,
        signature: Vec::new(),
    })
}

pub(super) fn timer_entry_from_intent(intent: &EffectIntent) -> Result<TimerEntry, WorkerError> {
    let params: TimerSetParams = serde_cbor::from_slice(&intent.params_cbor)
        .map_err(|err| WorkerError::Persist(aos_node::PersistError::validation(err.to_string())))?;
    Ok(TimerEntry {
        deliver_at_ns: params.deliver_at_ns,
        intent_hash: intent.intent_hash,
        key: params.key,
        params_cbor: intent.params_cbor.clone(),
    })
}

pub(super) fn effect_intent_from_pending(
    pending: &aos_kernel::snapshot::WorkflowReceiptSnapshot,
) -> Result<EffectIntent, WorkerError> {
    let mut intent = aos_effects::EffectIntent::from_raw_params(
        pending.effect_kind.clone().into(),
        pending.cap_name.clone(),
        pending.params_cbor.clone(),
        pending.idempotency_key,
    )
    .map_err(|err| WorkerError::Persist(aos_node::PersistError::validation(err.to_string())))?;
    intent.intent_hash = pending.intent_hash;
    Ok(intent)
}

pub(super) fn runtime_info_from_world(
    world: &ActiveWorld,
    async_state: Option<&AsyncWorldState>,
) -> WorldRuntimeInfo {
    let quiescence = world.kernel.quiescence_status();
    let journal_bounds = world.kernel.journal_bounds();
    let scheduled_timers = async_state
        .map(|state| !state.scheduled_timers.is_empty())
        .unwrap_or(false);
    let next_timer_due_at_ns = async_state.and_then(|state| state.timer_scheduler.next_due_at_ns());
    WorldRuntimeInfo {
        world_id: world.world_id,
        universe_id: world.universe_id,
        created_at_ns: world.created_at_ns,
        manifest_hash: Some(world.kernel.manifest_hash().to_hex()),
        active_baseline_height: Some(world.active_baseline.height),
        notify_counter: world.next_world_seq,
        has_pending_inbox: !world.mailbox.is_empty(),
        has_pending_effects: quiescence.queued_effects > 0
            || quiescence.pending_workflow_receipts > 0
            || quiescence.inflight_workflow_intents > 0
            || quiescence.non_terminal_workflow_instances > 0
            || scheduled_timers,
        next_timer_due_at_ns,
        has_pending_maintenance: journal_bounds.next_seq > journal_bounds.retained_from,
    }
}

pub(super) fn unix_time_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default()
}
