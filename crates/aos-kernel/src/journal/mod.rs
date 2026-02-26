pub mod fs;
pub mod mem;

use aos_effects::ReceiptStatus;
use serde::{Deserialize, Serialize};
use thiserror::Error;

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
    CapDecision,
    Manifest,
    Snapshot,
    PolicyDecision,
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
    CapDecision(CapDecisionRecord),
    PolicyDecision(PolicyDecisionRecord),
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
            JournalRecord::CapDecision(_) => JournalKind::CapDecision,
            JournalRecord::PolicyDecision(_) => JournalKind::PolicyDecision,
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
    pub kind: String,
    pub cap_name: String,
    #[serde(with = "serde_bytes")]
    pub params_cbor: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub idempotency_key: [u8; 32],
    pub origin: IntentOriginRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "origin_kind")]
pub enum IntentOriginRecord {
    Reducer {
        name: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "serde_bytes_opt"
        )]
        instance_key: Option<Vec<u8>>,
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub origin_instance_key: Option<Vec<u8>>,
    pub effect_kind: String,
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
#[serde(rename_all = "snake_case")]
pub enum CapDecisionStage {
    Enqueue,
    Settle,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapDecisionOutcome {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapDenyReason {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for CapDenyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.code, self.message)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapDecisionRecord {
    pub intent_hash: [u8; 32],
    pub stage: CapDecisionStage,
    pub effect_kind: String,
    pub cap_name: String,
    pub cap_type: String,
    pub grant_hash: [u8; 32],
    pub enforcer_module: String,
    pub decision: CapDecisionOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny: Option<CapDenyReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry_ns: Option<u64>,
    pub logical_now_ns: u64,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ledger_deltas: Vec<crate::shadow::LedgerDelta>,
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
