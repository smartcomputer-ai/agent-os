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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainEventIngress {
    pub schema: String,
    #[serde(with = "serde_bytes")]
    pub value_cbor: Vec<u8>,
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
    pub adapter_id: String,
    #[serde(with = "serde_bytes")]
    pub payload_cbor: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalInboxIngress {
    pub source: String,
    #[serde(with = "serde_bytes")]
    pub payload_cbor: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerFiredIngress {
    #[serde(with = "serde_bytes")]
    pub intent_hash: Vec<u8>,
    pub deliver_at_ns: u64,
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
    Conflict(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("backend error: {0}")]
    Backend(String),
}

impl PersistError {
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

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
        return Err(PersistError::conflict(format!(
            "snapshot index at height {} already exists",
            record.height
        )));
    }
    Ok(())
}
