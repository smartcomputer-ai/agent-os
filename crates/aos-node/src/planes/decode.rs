use aos_cbor::{HASH_PREFIX, Hash};
use aos_effects::EffectReceipt;
use aos_kernel::Store;
use aos_kernel::journal::{JournalRecord, OwnedJournalEntry};
use aos_runtime::{ExternalEvent, HostError};

use crate::CborPayload;

use super::model::{SYS_TIMER_FIRED_SCHEMA, SubmissionPayload, WorldLogFrame};
use super::traits::PlaneError;

pub fn submission_payload_to_external_event<S: Store + ?Sized>(
    store: &S,
    payload: &SubmissionPayload,
) -> Result<ExternalEvent, PlaneError> {
    match payload {
        SubmissionPayload::DomainEvent { schema, value, key } => Ok(ExternalEvent::DomainEvent {
            schema: schema.clone(),
            value: resolve_plane_cbor_payload(store, value)?,
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
            intent_hash: parse_plane_intent_hash(intent_hash)?,
            adapter_id: adapter_id.clone(),
            status: status.clone(),
            payload_cbor: resolve_plane_cbor_payload(store, payload)?,
            cost_cents: *cost_cents,
            signature: signature.clone(),
        })),
        SubmissionPayload::TimerFired { payload } => Ok(ExternalEvent::DomainEvent {
            schema: SYS_TIMER_FIRED_SCHEMA.into(),
            value: resolve_plane_cbor_payload(store, payload)?,
            key: None,
        }),
        SubmissionPayload::Command { .. } | SubmissionPayload::CreateWorld { .. } => {
            Err(PlaneError::Host(HostError::External(
                "submission payload is not an external event".into(),
            )))
        }
    }
}

pub fn parse_plane_hash_like(value: &str, field: &str) -> Result<Hash, PlaneError> {
    let trimmed = value.trim();
    let normalized = if trimmed.starts_with(HASH_PREFIX) {
        trimmed.to_string()
    } else {
        format!("{HASH_PREFIX}{trimmed}")
    };
    Hash::from_hex_str(&normalized)
        .map_err(|_| PlaneError::InvalidHashRef(format!("invalid {field} '{value}'")))
}

pub fn resolve_plane_cbor_payload<S: Store + ?Sized>(
    store: &S,
    payload: &CborPayload,
) -> Result<Vec<u8>, PlaneError> {
    payload.validate()?;
    if let Some(inline) = &payload.inline_cbor {
        return Ok(inline.clone());
    }

    let Some(hash_ref) = payload.cbor_ref.as_deref() else {
        return Err(PlaneError::InvalidHashRef("<missing>".into()));
    };
    let hash = parse_plane_hash_ref(hash_ref)?;
    Ok(store.get_blob(hash)?)
}

pub fn parse_plane_hash_ref(value: &str) -> Result<Hash, PlaneError> {
    let normalized = if value.starts_with(HASH_PREFIX) {
        value.to_string()
    } else {
        format!("{HASH_PREFIX}{value}")
    };
    Hash::from_hex_str(&normalized).map_err(|_| PlaneError::InvalidHashRef(value.to_string()))
}

pub fn parse_plane_intent_hash(bytes: &[u8]) -> Result<[u8; 32], PlaneError> {
    bytes
        .try_into()
        .map_err(|_| PlaneError::InvalidIntentHashLen(bytes.len()))
}

pub fn latest_plane_snapshot_record(
    entries: &[OwnedJournalEntry],
) -> Option<aos_kernel::journal::SnapshotRecord> {
    entries.iter().rev().find_map(|entry| {
        match serde_cbor::from_slice::<JournalRecord>(&entry.payload) {
            Ok(JournalRecord::Snapshot(record)) => Some(record),
            _ => None,
        }
    })
}

pub fn journal_entries_from_world_frames(
    frames: &[WorldLogFrame],
) -> Result<Vec<OwnedJournalEntry>, PlaneError> {
    let mut entries = Vec::new();
    for frame in frames {
        for (offset, record) in frame.records.iter().enumerate() {
            entries.push(OwnedJournalEntry {
                seq: frame.world_seq_start + offset as u64,
                kind: record.kind(),
                payload: serde_cbor::to_vec(record)?,
            });
        }
    }
    Ok(entries)
}
