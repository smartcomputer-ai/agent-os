use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use aos_cbor::Hash;
use serde::{Deserialize, Serialize};
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

pub type JournalHeight = u64;
pub type ShardId = u16;
pub type TimeBucket = u64;
pub type QueueSeq = InboxSeq;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CasConfig {
    pub inline_threshold_bytes: usize,
    pub verify_reads: bool,
}

impl Default for CasConfig {
    fn default() -> Self {
        Self {
            inline_threshold_bytes: 4 * 1024,
            verify_reads: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalConfig {
    pub max_batch_entries: usize,
    pub max_batch_bytes: usize,
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            max_batch_entries: 256,
            max_batch_bytes: 256 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboxConfig {
    pub inline_payload_threshold_bytes: usize,
}

impl Default for InboxConfig {
    fn default() -> Self {
        Self {
            inline_payload_threshold_bytes: 4 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PersistenceConfig {
    pub cas: CasConfig,
    pub journal: JournalConfig,
    pub inbox: InboxConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UniverseId(Uuid);

impl UniverseId {
    pub fn new(value: Uuid) -> Self {
        Self(value)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl From<Uuid> for UniverseId {
    fn from(value: Uuid) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for UniverseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for UniverseId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorldId(Uuid);

impl WorldId {
    pub fn new(value: Uuid) -> Self {
        Self(value)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl From<Uuid> for WorldId {
    fn from(value: Uuid) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for WorldId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for WorldId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

/// Opaque, serializable, totally ordered cursor token.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InboxSeq(#[serde(with = "serde_bytes")] Vec<u8>);

impl InboxSeq {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    pub fn from_u64(value: u64) -> Self {
        Self(value.to_be_bytes().to_vec())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for InboxSeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InboxSeq({})", hex::encode(&self.0))
    }
}

impl fmt::Display for InboxSeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(&self.0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SegmentId {
    pub start: JournalHeight,
    pub end: JournalHeight,
}

impl SegmentId {
    pub fn new(start: JournalHeight, end: JournalHeight) -> Result<Self, PersistError> {
        if end < start {
            return Err(PersistError::validation(format!(
                "segment end {end} must be >= start {start}"
            )));
        }
        Ok(Self { start, end })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobStorage {
    Inline,
    ObjectStore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CasMeta {
    pub size: u64,
    pub storage: BlobStorage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_key: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub inline_bytes: Option<Vec<u8>>,
}

pub fn cas_object_key(universe: UniverseId, hash: Hash) -> String {
    let hash_hex = hex::encode(hash.as_bytes());
    format!("cas/{universe}/sha256/{hash_hex}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CborPayload {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub inline_cbor: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cbor_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cbor_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cbor_sha256: Option<String>,
}

impl CborPayload {
    pub fn inline(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            inline_cbor: Some(bytes.into()),
            cbor_ref: None,
            cbor_size: None,
            cbor_sha256: None,
        }
    }

    pub fn externalized(hash: Hash, size: u64) -> Self {
        Self {
            inline_cbor: None,
            cbor_ref: Some(hash.to_hex()),
            cbor_size: Some(size),
            cbor_sha256: Some(hash.to_hex()),
        }
    }

    pub fn inline_len(&self) -> usize {
        self.inline_cbor
            .as_ref()
            .map(|bytes| bytes.len())
            .unwrap_or(0)
    }

    pub fn validate(&self) -> Result<(), PersistError> {
        let has_inline = self.inline_cbor.is_some();
        let has_external =
            self.cbor_ref.is_some() || self.cbor_size.is_some() || self.cbor_sha256.is_some();
        if has_inline && has_external {
            return Err(PersistError::validation(
                "payload cannot contain both inline bytes and externalized metadata",
            ));
        }
        if !has_inline {
            match (&self.cbor_ref, self.cbor_size, &self.cbor_sha256) {
                (Some(_), Some(_), Some(_)) => {}
                (None, None, None) => {
                    return Err(PersistError::validation(
                        "payload must contain inline bytes or full externalized metadata",
                    ));
                }
                _ => {
                    return Err(PersistError::validation(
                        "externalized payload requires cbor_ref, cbor_size, and cbor_sha256",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_baseline_height: Option<JournalHeight>,
    #[serde(default)]
    pub created_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InboxItem {
    DomainEvent(DomainEventIngress),
    Receipt(ReceiptIngress),
    Inbox(ExternalInboxIngress),
    TimerFired(TimerFiredIngress),
    Control(ControlIngress),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainEventIngress {
    pub schema: String,
    pub value: CborPayload,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub key: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptIngress {
    #[serde(with = "serde_bytes")]
    pub intent_hash: Vec<u8>,
    pub effect_kind: String,
    pub adapter_id: String,
    pub payload: CborPayload,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalInboxIngress {
    pub inbox_name: String,
    pub payload: CborPayload,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerFiredIngress {
    pub timer_id: String,
    pub payload: CborPayload,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlIngress {
    pub cmd: String,
    pub payload: CborPayload,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRecord {
    pub snapshot_ref: String,
    pub height: JournalHeight,
    #[serde(default)]
    pub logical_time_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_horizon_height: Option<JournalHeight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentIndexRecord {
    pub segment: SegmentId,
    pub object_key: String,
    pub checksum: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinReason {
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectDispatchItem {
    pub shard: ShardId,
    pub universe_id: UniverseId,
    pub world_id: WorldId,
    #[serde(with = "serde_bytes")]
    pub intent_hash: Vec<u8>,
    pub effect_kind: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub params_inline_cbor: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_sha256: Option<String>,
    pub origin_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_context_hash: Option<String>,
    #[serde(default)]
    pub enqueued_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectInFlightItem {
    pub dispatch: EffectDispatchItem,
    #[serde(default)]
    pub claim_until_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchStatus {
    Pending,
    InFlight,
    Complete,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerDueItem {
    pub shard: ShardId,
    pub universe_id: UniverseId,
    pub world_id: WorldId,
    #[serde(with = "serde_bytes")]
    pub intent_hash: Vec<u8>,
    pub time_bucket: TimeBucket,
    pub deliver_at_ns: u64,
    #[serde(with = "serde_bytes")]
    pub payload_cbor: Vec<u8>,
    #[serde(default)]
    pub enqueued_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerClaim {
    #[serde(with = "serde_bytes")]
    pub intent_hash: Vec<u8>,
    #[serde(default)]
    pub claim_until_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveredStatus {
    Pending,
    Delivered,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PersistError {
    #[error("conflict: {0}")]
    Conflict(#[from] PersistConflict),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("corruption: {0}")]
    Corrupt(#[from] PersistCorruption),
    #[error("backend error: {0}")]
    Backend(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PersistConflict {
    #[error("journal head advanced: expected {expected}, actual {actual}")]
    HeadAdvanced {
        expected: JournalHeight,
        actual: JournalHeight,
    },
    #[error("inbox cursor compare-and-swap failed: expected {expected:?}, actual {actual:?}")]
    InboxCursorAdvanced {
        expected: Option<InboxSeq>,
        actual: Option<InboxSeq>,
    },
    #[error("snapshot index at height {height} already exists")]
    SnapshotExists { height: JournalHeight },
    #[error("snapshot at height {height} differs from promotion record")]
    SnapshotMismatch { height: JournalHeight },
    #[error("baseline at height {height} already points at a different snapshot")]
    BaselineMismatch { height: JournalHeight },
    #[error("segment index for end height {end_height} already exists")]
    SegmentExists { end_height: JournalHeight },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PersistCorruption {
    #[error("journal entry missing at height {height}")]
    MissingJournalEntry { height: JournalHeight },
    #[error("inline CAS metadata missing bytes for {hash}")]
    MissingInlineCasBytes { hash: Hash },
    #[error("object-store CAS metadata missing object key for {hash}")]
    MissingCasObjectKey { hash: Hash },
    #[error("object-store body missing for {hash} at key {object_key}")]
    MissingCasObjectBody { hash: Hash, object_key: String },
    #[error("CAS body hash mismatch for {expected}: loaded {actual}")]
    CasBodyHashMismatch { expected: Hash, actual: Hash },
}

impl PersistError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }

    pub fn backend(message: impl Into<String>) -> Self {
        Self::Backend(message.into())
    }
}

/// Narrow runtime/storage protocol boundary for hosted world persistence.
pub trait WorldPersistence: Send + Sync {
    fn cas_put_verified(&self, universe: UniverseId, bytes: &[u8]) -> Result<Hash, PersistError>;
    fn cas_get(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, PersistError>;
    fn cas_has(&self, universe: UniverseId, hash: Hash) -> Result<bool, PersistError>;

    fn journal_append_batch(
        &self,
        universe: UniverseId,
        world: WorldId,
        expected_head: JournalHeight,
        entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError>;

    fn journal_read_range(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_inclusive: JournalHeight,
        limit: u32,
    ) -> Result<Vec<(JournalHeight, Vec<u8>)>, PersistError>;

    /// Returns the next height that will be assigned on append.
    fn journal_head(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<JournalHeight, PersistError>;

    fn inbox_enqueue(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError>;

    fn inbox_read_after(
        &self,
        universe: UniverseId,
        world: WorldId,
        after_exclusive: Option<InboxSeq>,
        limit: u32,
    ) -> Result<Vec<(InboxSeq, InboxItem)>, PersistError>;

    fn inbox_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<InboxSeq>, PersistError>;

    fn inbox_commit_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
    ) -> Result<(), PersistError>;

    fn drain_inbox_to_journal(
        &self,
        universe: UniverseId,
        world: WorldId,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
        expected_head: JournalHeight,
        journal_entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError>;

    fn snapshot_index(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError>;

    fn snapshot_at_height(
        &self,
        universe: UniverseId,
        world: WorldId,
        height: JournalHeight,
    ) -> Result<SnapshotRecord, PersistError>;

    fn snapshot_active_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<SnapshotRecord, PersistError>;

    fn snapshot_promote_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError>;

    fn segment_index_put(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SegmentIndexRecord,
    ) -> Result<(), PersistError>;

    fn segment_index_read_from(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_end_inclusive: JournalHeight,
        limit: u32,
    ) -> Result<Vec<SegmentIndexRecord>, PersistError>;
}

pub(crate) fn sample_world_meta() -> WorldMeta {
    WorldMeta {
        manifest_hash: None,
        active_baseline_height: None,
        created_at_ns: 0,
    }
}

pub(crate) fn ensure_monotonic_snapshot_records(
    records: &BTreeMap<JournalHeight, SnapshotRecord>,
    record: &SnapshotRecord,
) -> Result<(), PersistError> {
    if let Some(existing) = records.get(&record.height) {
        if existing == record {
            return Ok(());
        }
        return Err(PersistConflict::SnapshotExists {
            height: record.height,
        }
        .into());
    }
    Ok(())
}
