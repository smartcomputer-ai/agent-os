use std::collections::BTreeMap;

use aos_cbor::Hash;
use aos_kernel::StoreError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    PersistError, SubmissionRejection, UniverseId, WorldCheckpointRecord, WorldId,
    WorldJournalCursor, WorldLogFrame,
};

#[derive(Debug, Error)]
pub enum BackendError {
    #[error(transparent)]
    Persist(#[from] PersistError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Cbor(#[from] serde_cbor::Error),
    #[error("invalid hash ref '{0}'")]
    InvalidHashRef(String),
    #[error("invalid intent hash length {0}")]
    InvalidIntentHashLen(usize),
    #[error("invalid partition count")]
    InvalidPartitionCount,
    #[error(
        "non-contiguous world sequence for {world_id} in universe {universe_id}: expected {expected}, got {actual}"
    )]
    NonContiguousWorldSeq {
        universe_id: UniverseId,
        world_id: WorldId,
        expected: u64,
        actual: u64,
    },
    #[error("unknown world {world_id} in universe {universe_id}")]
    UnknownWorld {
        universe_id: UniverseId,
        world_id: WorldId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldLogAppendResult {
    pub journal_offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldDurableHead {
    pub next_world_seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum JournalSourceAck {
    PartitionOffset { partition: u32, offset: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JournalDisposition {
    RejectedSubmission {
        source_ack: Option<JournalSourceAck>,
        world_id: WorldId,
        reason: SubmissionRejection,
    },
    CommandFailure {
        source_ack: Option<JournalSourceAck>,
        world_id: WorldId,
        command_id: String,
        error_code: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalFlush {
    pub frames: Vec<WorldLogFrame>,
    pub dispositions: Vec<JournalDisposition>,
    pub source_acks: Vec<JournalSourceAck>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalCommit {
    pub world_cursors: BTreeMap<WorldId, WorldJournalCursor>,
}

pub trait BlobBackend {
    fn put_blob(&self, universe_id: UniverseId, bytes: &[u8]) -> Result<Hash, BackendError>;
    fn get_blob(&self, universe_id: UniverseId, hash: Hash) -> Result<Vec<u8>, BackendError>;
    fn has_blob(&self, universe_id: UniverseId, hash: Hash) -> Result<bool, BackendError>;
}

pub trait CheckpointBackend {
    fn commit_world_checkpoint(
        &mut self,
        checkpoint: WorldCheckpointRecord,
    ) -> Result<(), BackendError>;
    fn latest_world_checkpoint(
        &self,
        world_id: WorldId,
    ) -> Result<Option<WorldCheckpointRecord>, BackendError>;
    fn list_world_checkpoints(&self) -> Result<Vec<WorldCheckpointRecord>, BackendError>;
}

pub trait WorldInventoryBackend {
    fn list_worlds(&self) -> Result<Vec<WorldId>, BackendError>;
}

pub trait JournalBackend {
    fn refresh_all(&mut self) -> Result<(), BackendError>;
    fn refresh_world(&mut self, world_id: WorldId) -> Result<(), BackendError>;
    fn world_ids(&self) -> Vec<WorldId>;
    fn durable_head(&self, world_id: WorldId) -> Result<WorldDurableHead, BackendError>;
    fn world_frames(&self, world_id: WorldId) -> Result<Vec<WorldLogFrame>, BackendError>;
    fn world_tail_frames(
        &self,
        world_id: WorldId,
        after_world_seq: u64,
        cursor: Option<&WorldJournalCursor>,
    ) -> Result<Vec<WorldLogFrame>, BackendError>;
    fn commit_flush(&mut self, flush: JournalFlush) -> Result<JournalCommit, BackendError>;
}

pub trait WorldLogBackend {
    fn append_frame(&mut self, frame: WorldLogFrame) -> Result<WorldLogAppendResult, BackendError>;
    fn world_frames(&self, world_id: WorldId) -> &[WorldLogFrame];
}
