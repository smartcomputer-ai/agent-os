use aos_cbor::Hash;
use aos_kernel::{KernelError, Store, StoreError};
use aos_runtime::{HostError, WorldHost};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{PersistError, SnapshotRecord, UniverseId, WorldId};

use super::model::{PartitionCheckpoint, SubmissionEnvelope, WorldLogFrame};

#[derive(Debug, Error)]
pub enum PlaneError {
    #[error(transparent)]
    Host(#[from] HostError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
    #[error(transparent)]
    Cbor(#[from] serde_cbor::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Persist(#[from] PersistError),
    #[error("partition_count must be greater than zero")]
    InvalidPartitionCount,
    #[error("world {world_id} in universe {universe_id} is not registered")]
    UnknownWorld {
        universe_id: UniverseId,
        world_id: WorldId,
    },
    #[error("no blob plane store is registered for universe {0}")]
    UnknownUniverseStore(UniverseId),
    #[error("invalid blob hash ref '{0}'")]
    InvalidHashRef(String),
    #[error("receipt intent hash must be 32 bytes, got {0}")]
    InvalidIntentHashLen(usize),
    #[error("unsupported create-world source '{0}' on the plane seam")]
    UnsupportedCreateWorldSource(&'static str),
    #[error(
        "world log frame sequence is not contiguous for world {world_id} in universe {universe_id}: expected {expected}, got {actual}"
    )]
    NonContiguousWorldSeq {
        universe_id: UniverseId,
        world_id: WorldId,
        expected: u64,
        actual: u64,
    },
}

pub trait BlobPlane {
    fn put_blob(&self, universe_id: UniverseId, bytes: &[u8]) -> Result<Hash, PlaneError>;
    fn get_blob(&self, universe_id: UniverseId, hash: Hash) -> Result<Vec<u8>, PlaneError>;
    fn has_blob(&self, universe_id: UniverseId, hash: Hash) -> Result<bool, PlaneError>;
}

pub trait SubmissionPlane {
    fn submit(&mut self, submission: SubmissionEnvelope) -> Result<u64, PlaneError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldLogAppendResult {
    pub journal_offset: u64,
}

pub trait WorldLogPlane {
    fn append_frame(&mut self, frame: WorldLogFrame) -> Result<WorldLogAppendResult, PlaneError>;
    fn world_frames(&self, world_id: WorldId) -> &[WorldLogFrame];
}

pub trait CheckpointPlane {
    fn commit_checkpoint(&mut self, checkpoint: PartitionCheckpoint) -> Result<(), PlaneError>;
    fn latest_checkpoint(
        &self,
        journal_topic: &str,
        partition: u32,
    ) -> Option<&PartitionCheckpoint>;
}

pub struct PlaneCreatedWorld<S: Store + 'static> {
    pub host: WorldHost<S>,
    pub initial_manifest_hash: String,
    pub active_baseline: SnapshotRecord,
    pub initial_frame: Option<WorldLogFrame>,
}
