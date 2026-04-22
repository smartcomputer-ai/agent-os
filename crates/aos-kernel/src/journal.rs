use aos_air_types::HashRef;
use aos_effects::ReceiptStatus;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use thiserror::Error;
use uuid::Uuid;

mod serde_bytes_opt {
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_bytes::{ByteBuf, Bytes};

    pub fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(bytes) => serializer.serialize_some(Bytes::new(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<ByteBuf>::deserialize(deserializer).map(|opt| opt.map(|buf| buf.into_vec()))
    }
}

/// Monotonic cursor assigned to every persisted journal entry.
pub type JournalSeq = u64;

/// High-level classification of a journal entry. These align with the runtime
/// flows described in spec/02-architecture.md and can evolve as WP4 grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalKind {
    DomainEvent,
    EffectIntent,
    EffectReceipt,
    StreamFrame,
    Manifest,
    Snapshot,
    Governance,
    PlanStarted,
    PlanResult,
    PlanEnded,
    Custom,
}

/// Type-safe payloads for each `JournalKind`. These are serialized into the
/// `payload` field so downstream readers can match on the enum and decode the
/// appropriate structure without bespoke wiring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "record_kind", rename_all = "snake_case")]
pub enum JournalRecord {
    DomainEvent(DomainEventRecord),
    EffectIntent(EffectIntentRecord),
    EffectReceipt(EffectReceiptRecord),
    StreamFrame(StreamFrameRecord),
    Manifest(ManifestRecord),
    Snapshot(SnapshotRecord),
    Governance(GovernanceRecord),
    PlanStarted(PlanStartedRecord),
    PlanResult(PlanResultRecord),
    PlanEnded(PlanEndedRecord),
    Custom(CustomRecord),
}

impl JournalRecord {
    pub fn kind(&self) -> JournalKind {
        match self {
            JournalRecord::DomainEvent(_) => JournalKind::DomainEvent,
            JournalRecord::EffectIntent(_) => JournalKind::EffectIntent,
            JournalRecord::EffectReceipt(_) => JournalKind::EffectReceipt,
            JournalRecord::StreamFrame(_) => JournalKind::StreamFrame,
            JournalRecord::Manifest(_) => JournalKind::Manifest,
            JournalRecord::Snapshot(_) => JournalKind::Snapshot,
            JournalRecord::Governance(_) => JournalKind::Governance,
            JournalRecord::PlanStarted(_) => JournalKind::PlanStarted,
            JournalRecord::PlanResult(_) => JournalKind::PlanResult,
            JournalRecord::PlanEnded(_) => JournalKind::PlanEnded,
            JournalRecord::Custom(_) => JournalKind::Custom,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanEndStatus {
    Ok,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanEndedRecord {
    pub plan_name: String,
    pub plan_id: u64,
    pub status: PlanEndStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanStartedRecord {
    pub plan_name: String,
    pub plan_id: u64,
    pub input_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_instance_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DomainEventRecord {
    pub schema: String,
    #[serde(with = "serde_bytes")]
    pub value: Vec<u8>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub key: Option<Vec<u8>>,
    #[serde(default)]
    pub now_ns: u64,
    #[serde(default)]
    pub logical_now_ns: u64,
    #[serde(default)]
    pub journal_height: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "serde_bytes")]
    pub entropy: Vec<u8>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub event_hash: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manifest_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffectIntentRecord {
    pub intent_hash: [u8; 32],
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub effect_op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_op_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_module: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_module_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_entrypoint: Option<String>,
    pub kind: String,
    #[serde(with = "serde_bytes")]
    pub params_cbor: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_sha256: Option<HashRef>,
    #[serde(with = "serde_bytes")]
    pub idempotency_key: [u8; 32],
    pub origin: IntentOriginRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "origin_kind")]
pub enum IntentOriginRecord {
    Workflow {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_op_hash: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "serde_bytes_opt"
        )]
        instance_key: Option<Vec<u8>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        issuer_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        emitted_at_seq: Option<u64>,
    },
    Plan {
        name: String,
        plan_id: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffectReceiptRecord {
    pub intent_hash: [u8; 32],
    pub adapter_id: String,
    pub status: ReceiptStatus,
    #[serde(with = "serde_bytes")]
    pub payload_cbor: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_sha256: Option<HashRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_cents: Option<u64>,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
    #[serde(default)]
    pub now_ns: u64,
    #[serde(default)]
    pub logical_now_ns: u64,
    #[serde(default)]
    pub journal_height: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "serde_bytes")]
    pub entropy: Vec<u8>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manifest_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamFrameRecord {
    pub intent_hash: [u8; 32],
    pub adapter_id: String,
    pub origin_module_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_workflow_op_hash: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub origin_instance_key: Option<Vec<u8>>,
    #[serde(default, alias = "effect_kind")]
    pub effect_op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_op_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_module: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_module_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_entrypoint: Option<String>,
    pub emitted_at_seq: u64,
    pub seq: u64,
    pub frame_kind: String,
    #[serde(with = "serde_bytes")]
    pub payload_cbor: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_ref: Option<String>,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
    #[serde(default)]
    pub now_ns: u64,
    #[serde(default)]
    pub logical_now_ns: u64,
    #[serde(default)]
    pub journal_height: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "serde_bytes")]
    pub entropy: Vec<u8>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manifest_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanResultRecord {
    pub plan_name: String,
    pub plan_id: u64,
    pub output_schema: String,
    #[serde(with = "serde_bytes")]
    pub value_cbor: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotRecord {
    /// Reference to the snapshot blob stored in CAS (sha256:... string).
    pub snapshot_ref: String,
    /// Logical height the snapshot represents (number of events applied).
    pub height: JournalSeq,
    /// Universe identity for durable hosted artifacts derived from this world.
    #[serde(default)]
    pub universe_id: Uuid,
    /// Logical runtime time captured in this baseline snapshot.
    #[serde(default)]
    pub logical_time_ns: u64,
    /// Optional safety fence for receipts included in baseline state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_horizon_height: Option<JournalSeq>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestRecord {
    pub manifest_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GovernanceRecord {
    Proposed(ProposedRecord),
    ShadowReport(ShadowReportRecord),
    Approved(ApprovedRecord),
    Applied(AppliedRecord),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProposedRecord {
    pub proposal_id: u64,
    pub description: Option<String>,
    pub patch_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShadowReportRecord {
    pub proposal_id: u64,
    pub patch_hash: String,
    pub manifest_hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects_predicted: Vec<crate::shadow::PredictedEffect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_workflow_receipts: Vec<crate::shadow::PendingWorkflowReceipt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_instances: Vec<crate::shadow::WorkflowInstancePreview>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub module_effect_allowlists: Vec<crate::shadow::ModuleEffectAllowlist>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionRecord {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovedRecord {
    pub proposal_id: u64,
    pub patch_hash: String,
    pub approver: String,
    #[serde(default = "ApprovalDecisionRecord::default_approve")]
    pub decision: ApprovalDecisionRecord,
}

impl ApprovalDecisionRecord {
    fn default_approve() -> Self {
        ApprovalDecisionRecord::Approve
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppliedRecord {
    pub proposal_id: u64,
    pub patch_hash: String,
    pub manifest_hash_new: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomRecord {
    pub tag: String,
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}

/// Borrowed entry used when appending to the journal.
#[derive(Debug, Clone, Copy)]
pub struct JournalEntry<'a> {
    pub kind: JournalKind,
    pub payload: &'a [u8],
}

impl<'a> JournalEntry<'a> {
    pub fn new(kind: JournalKind, payload: &'a [u8]) -> Self {
        Self { kind, payload }
    }
}

/// Owned entry returned by journal readers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnedJournalEntry {
    pub seq: JournalSeq,
    pub kind: JournalKind,
    #[serde(with = "serde_bytes")]
    pub payload: Vec<u8>,
}

impl OwnedJournalEntry {
    pub fn into_payload(self) -> Vec<u8> {
        self.payload
    }
}

#[derive(Debug, Error)]
pub enum JournalError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Cbor(#[from] serde_cbor::Error),
    #[error("corrupt entry: {0}")]
    Corrupt(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalBounds {
    pub retained_from: JournalSeq,
    pub next_seq: JournalSeq,
}

#[derive(Debug, Clone, Default)]
pub struct Journal {
    retained_from: JournalSeq,
    entries: VecDeque<OwnedJournalEntry>,
}

impl Journal {
    pub fn new() -> Self {
        Self {
            retained_from: 0,
            entries: VecDeque::new(),
        }
    }

    pub fn with_retained_from(retained_from: JournalSeq) -> Self {
        Self {
            retained_from,
            entries: VecDeque::new(),
        }
    }

    pub fn from_entries(entries: &[OwnedJournalEntry]) -> Result<Self, JournalError> {
        if entries.is_empty() {
            return Ok(Self::new());
        }

        let retained_from = entries[0].seq;
        let mut expected = retained_from;
        let mut retained = VecDeque::with_capacity(entries.len());
        for entry in entries {
            if entry.seq != expected {
                return Err(JournalError::Corrupt(format!(
                    "journal sequence is not contiguous: expected {expected}, got {}",
                    entry.seq
                )));
            }
            retained.push_back(entry.clone());
            expected = expected.saturating_add(1);
        }
        Ok(Self {
            retained_from,
            entries: retained,
        })
    }

    pub fn append(&mut self, entry: JournalEntry<'_>) -> Result<JournalSeq, JournalError> {
        let seq = self.next_seq();
        self.entries.push_back(OwnedJournalEntry {
            seq,
            kind: entry.kind,
            payload: entry.payload.to_vec(),
        });
        Ok(seq)
    }

    pub fn append_batch(
        &mut self,
        entries: &[JournalEntry<'_>],
    ) -> Result<JournalSeq, JournalError> {
        let first_seq = self.next_seq();
        for entry in entries {
            self.append(*entry)?;
        }
        Ok(first_seq)
    }

    /// Loads retained entries starting at `from` (inclusive). Passing 0 returns
    /// the full retained journal tail.
    pub fn load_from(&self, from: JournalSeq) -> Result<Vec<OwnedJournalEntry>, JournalError> {
        let from = from.max(self.retained_from);
        let start_idx = from.saturating_sub(self.retained_from) as usize;
        Ok(self.collect_from_index(start_idx, None))
    }

    /// Loads up to `limit` retained entries starting at `from` (inclusive).
    pub fn load_batch_from(
        &self,
        from: JournalSeq,
        limit: usize,
    ) -> Result<Vec<OwnedJournalEntry>, JournalError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let from = from.max(self.retained_from);
        let start_idx = from.saturating_sub(self.retained_from) as usize;
        Ok(self.collect_from_index(start_idx, Some(limit)))
    }

    pub fn next_seq(&self) -> JournalSeq {
        self.retained_from + self.entries.len() as JournalSeq
    }

    pub fn bounds(&self) -> JournalBounds {
        JournalBounds {
            retained_from: self.retained_from,
            next_seq: self.next_seq(),
        }
    }

    pub fn compact_through(&mut self, inclusive_seq: JournalSeq) -> Result<(), JournalError> {
        let target_retained = inclusive_seq
            .saturating_add(1)
            .clamp(self.retained_from, self.next_seq());
        let to_drop = target_retained.saturating_sub(self.retained_from) as usize;
        for _ in 0..to_drop {
            let _ = self.entries.pop_front();
        }
        self.retained_from = target_retained;
        Ok(())
    }

    fn collect_from_index(&self, start_idx: usize, limit: Option<usize>) -> Vec<OwnedJournalEntry> {
        if start_idx >= self.entries.len() {
            return Vec::new();
        }

        let available = self.entries.len() - start_idx;
        let take = limit.unwrap_or(available).min(available);
        let (front, back) = self.entries.as_slices();
        let mut out = Vec::with_capacity(take);

        if start_idx < front.len() {
            let front_start = start_idx;
            let front_take = take.min(front.len() - front_start);
            out.extend(front[front_start..front_start + front_take].iter().cloned());
            let remaining = take - front_take;
            if remaining > 0 {
                out.extend(back[..remaining].iter().cloned());
            }
        } else {
            let back_start = start_idx - front.len();
            out.extend(back[back_start..back_start + take].iter().cloned());
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_load_from_returns_contiguous_entries() {
        let mut journal = Journal::new();
        journal
            .append(JournalEntry::new(JournalKind::DomainEvent, b"first"))
            .unwrap();
        journal
            .append(JournalEntry::new(JournalKind::EffectIntent, b"second"))
            .unwrap();

        let all = journal.load_from(0).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].seq, 0);
        assert_eq!(all[1].seq, 1);
        assert_eq!(all[1].kind, JournalKind::EffectIntent);
        assert_eq!(journal.bounds().retained_from, 0);
        assert_eq!(journal.bounds().next_seq, 2);
    }

    #[test]
    fn compact_through_advances_retained_prefix_without_resetting_sequence() {
        let mut journal = Journal::new();
        journal
            .append(JournalEntry::new(JournalKind::DomainEvent, b"a"))
            .unwrap();
        journal
            .append(JournalEntry::new(JournalKind::DomainEvent, b"b"))
            .unwrap();
        journal
            .append(JournalEntry::new(JournalKind::DomainEvent, b"c"))
            .unwrap();

        journal.compact_through(1).unwrap();
        let retained = journal.load_from(0).unwrap();
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].seq, 2);
        assert_eq!(
            journal.bounds(),
            JournalBounds {
                retained_from: 2,
                next_seq: 3,
            }
        );

        let next = journal
            .append(JournalEntry::new(JournalKind::EffectReceipt, b"d"))
            .unwrap();
        assert_eq!(next, 3);
        assert_eq!(journal.bounds().next_seq, 4);
    }

    #[test]
    fn from_entries_rejects_non_contiguous_sequence() {
        let err = Journal::from_entries(&[
            OwnedJournalEntry {
                seq: 0,
                kind: JournalKind::DomainEvent,
                payload: b"a".to_vec(),
            },
            OwnedJournalEntry {
                seq: 2,
                kind: JournalKind::DomainEvent,
                payload: b"b".to_vec(),
            },
        ])
        .unwrap_err();

        assert!(matches!(err, JournalError::Corrupt(_)));
    }

    #[test]
    fn with_retained_from_preserves_next_seq_for_empty_journal() {
        let journal = Journal::with_retained_from(42);
        assert_eq!(journal.bounds().retained_from, 42);
        assert_eq!(journal.bounds().next_seq, 42);
    }
}
