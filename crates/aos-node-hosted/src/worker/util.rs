use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use aos_cbor::{HASH_PREFIX, Hash};
use aos_effects::EffectReceipt;
use aos_kernel::Store;
use aos_kernel::journal::{JournalRecord, OwnedJournalEntry};
use aos_node::{CborPayload, PlaneError, SnapshotRecord, SubmissionPayload, WorldLogFrame};
use aos_runtime::{ExternalEvent, HostError};
use uuid::Uuid;

use super::types::WorkerError;

pub(super) fn submission_to_external_event(
    store: &impl Store,
    payload: &SubmissionPayload,
) -> Result<ExternalEvent, WorkerError> {
    match payload {
        SubmissionPayload::DomainEvent { schema, value, key } => Ok(ExternalEvent::DomainEvent {
            schema: schema.clone(),
            value: resolve_cbor_payload(store, value)?,
            key: key.clone(),
        }),
        SubmissionPayload::EffectReceipt {
            intent_hash,
            adapter_id,
            status,
            payload,
            cost_cents,
            signature,
        } => Ok(ExternalEvent::Receipt(EffectReceipt {
            intent_hash: parse_intent_hash(intent_hash)?,
            adapter_id: adapter_id.clone(),
            status: status.clone(),
            payload_cbor: resolve_cbor_payload(store, payload)?,
            cost_cents: *cost_cents,
            signature: signature.clone(),
        })),
        SubmissionPayload::TimerFired { payload } => Ok(ExternalEvent::DomainEvent {
            schema: aos_node::SYS_TIMER_FIRED_SCHEMA.into(),
            value: resolve_cbor_payload(store, payload)?,
            key: None,
        }),
        SubmissionPayload::Command { .. } | SubmissionPayload::CreateWorld { .. } => {
            Err(WorkerError::Host(HostError::External(
                "submission payload is not an external event".into(),
            )))
        }
    }
}

pub(super) fn resolve_cbor_payload<S: Store>(
    store: &S,
    payload: &CborPayload,
) -> Result<Vec<u8>, WorkerError> {
    payload.validate()?;
    if let Some(inline) = &payload.inline_cbor {
        return Ok(inline.clone());
    }

    let Some(hash_ref) = payload.cbor_ref.as_deref() else {
        return Err(WorkerError::LogFirst(PlaneError::InvalidHashRef(
            "<missing>".into(),
        )));
    };
    let hash = parse_hash_ref(hash_ref)?;
    Ok(store.get_blob(hash)?)
}

pub(super) fn parse_hash_ref(value: &str) -> Result<Hash, WorkerError> {
    let normalized = if value.starts_with(HASH_PREFIX) {
        value.to_owned()
    } else {
        format!("{HASH_PREFIX}{value}")
    };
    Hash::from_hex_str(&normalized)
        .map_err(|_| WorkerError::LogFirst(PlaneError::InvalidHashRef(value.to_owned())))
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
        .join(".aos-hosted"))
}

pub(super) fn temp_embedded_state_root() -> PathBuf {
    std::env::temp_dir().join(format!("aos-node-hosted-{}", Uuid::new_v4()))
}

pub(super) fn parse_intent_hash(bytes: &[u8]) -> Result<[u8; 32], WorkerError> {
    bytes
        .try_into()
        .map_err(|_| WorkerError::LogFirst(PlaneError::InvalidIntentHashLen(bytes.len())))
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

pub(super) fn unix_time_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default()
}
