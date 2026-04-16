use aos_cbor::Hash;
use aos_kernel::StoreError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    PartitionCheckpoint, PersistError, SubmissionEnvelope, UniverseId, WorldId, WorldLogFrame,
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

pub trait BlobBackend {
    fn put_blob(&self, universe_id: UniverseId, bytes: &[u8]) -> Result<Hash, BackendError>;
    fn get_blob(&self, universe_id: UniverseId, hash: Hash) -> Result<Vec<u8>, BackendError>;
    fn has_blob(&self, universe_id: UniverseId, hash: Hash) -> Result<bool, BackendError>;
}

pub trait CheckpointBackend {
    fn commit_checkpoint(&mut self, checkpoint: PartitionCheckpoint) -> Result<(), BackendError>;
    fn latest_checkpoint(
        &self,
        journal_topic: &str,
        partition: u32,
    ) -> Option<&PartitionCheckpoint>;
}

pub trait SubmissionBackend {
    fn submit(&mut self, submission: SubmissionEnvelope) -> Result<u64, BackendError>;
}

pub trait WorldLogBackend {
    fn append_frame(&mut self, frame: WorldLogFrame) -> Result<WorldLogAppendResult, BackendError>;
    fn world_frames(&self, world_id: WorldId) -> &[WorldLogFrame];
}
