pub mod fs;
pub mod mem;

use aos_effects::ReceiptStatus;
use serde::{Deserialize, Serialize};
use thiserror::Error;

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
    Snapshot,
    PolicyDecision,
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
    PolicyDecision(PolicyDecisionRecord),
    Snapshot(SnapshotRecord),
    Custom(CustomRecord),
}

impl JournalRecord {
    pub fn kind(&self) -> JournalKind {
        match self {
            JournalRecord::DomainEvent(_) => JournalKind::DomainEvent,
            JournalRecord::EffectIntent(_) => JournalKind::EffectIntent,
            JournalRecord::EffectReceipt(_) => JournalKind::EffectReceipt,
            JournalRecord::PolicyDecision(_) => JournalKind::PolicyDecision,
            JournalRecord::Snapshot(_) => JournalKind::Snapshot,
            JournalRecord::Custom(_) => JournalKind::Custom,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DomainEventRecord {
    pub schema: String,
    #[serde(with = "serde_bytes")]
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffectIntentRecord {
    pub intent_hash: [u8; 32],
    pub kind: String,
    pub cap_name: String,
    #[serde(with = "serde_bytes")]
    pub params_cbor: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub idempotency_key: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffectReceiptRecord {
    pub intent_hash: [u8; 32],
    pub adapter_id: String,
    pub status: ReceiptStatus,
    #[serde(with = "serde_bytes")]
    pub payload_cbor: Vec<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_cents: Option<u64>,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyDecisionRecord {
    pub intent_hash: [u8; 32],
    pub policy_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_index: Option<u32>,
    pub decision: PolicyDecisionOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PolicyDecisionOutcome {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotRecord {
    /// Reference to the snapshot blob stored in CAS (sha256:... string).
    pub snapshot_ref: String,
    /// Logical height the snapshot represents (number of events applied).
    pub height: JournalSeq,
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

/// Uniform interface implemented by concrete journal backends (filesystem,
/// in-memory) so the kernel stepper can target a single abstraction.
pub trait Journal: Send {
    fn append(&mut self, entry: JournalEntry<'_>) -> Result<JournalSeq, JournalError>;

    /// Loads entries starting at `from` (inclusive). Passing 0 returns the full log.
    fn load_from(&self, from: JournalSeq) -> Result<Vec<OwnedJournalEntry>, JournalError>;

    /// Returns the next sequence that will be assigned on append.
    fn next_seq(&self) -> JournalSeq;
}

/// Helper used by on-disk implementations to encode/decode entries in a stable
/// format. Exposed so tests can assert over the raw representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiskRecord<'a> {
    seq: JournalSeq,
    kind: JournalKind,
    #[serde(with = "serde_bytes")]
    payload: &'a [u8],
}

impl OwnedJournalEntry {
    fn from_disk(seq: JournalSeq, kind: JournalKind, payload: Vec<u8>) -> Self {
        Self { seq, kind, payload }
    }
}

impl<'a> From<&'a OwnedJournalEntry> for DiskRecord<'a> {
    fn from(entry: &'a OwnedJournalEntry) -> Self {
        Self {
            seq: entry.seq,
            kind: entry.kind,
            payload: &entry.payload,
        }
    }
}

impl<'a> DiskRecord<'a> {
    fn into_owned(self) -> OwnedJournalEntry {
        OwnedJournalEntry {
            seq: self.seq,
            kind: self.kind,
            payload: self.payload.to_vec(),
        }
    }
}
