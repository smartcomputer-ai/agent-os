use std::collections::BTreeSet;
use std::sync::Arc;

use aos_kernel::{KernelError, LoadedManifest, StoreError};
use aos_node::{CommandRecord, PlaneError, SnapshotRecord, UniverseId, WorldId};
use aos_runtime::{HostError, WorldConfig, WorldHost};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use thiserror::Error;

use crate::blobstore::{HostedBlobMetaStore, HostedCas};
use crate::kafka::HostedKafkaBackend;
use crate::vault::HostedVault;

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error(transparent)]
    LogFirst(#[from] PlaneError),
    #[error(transparent)]
    Host(#[from] HostError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Persist(#[from] aos_node::PersistError),
    #[error(transparent)]
    Build(#[from] anyhow::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Cbor(#[from] serde_cbor::Error),
    #[error("runtime mutex poisoned")]
    RuntimePoisoned,
    #[error("background build task failed: {0}")]
    BackgroundBuild(String),
    #[error("invalid world root '{0}'")]
    InvalidWorldRoot(String),
    #[error("world {world_id} in universe {universe_id} is not registered")]
    UnknownWorld {
        universe_id: UniverseId,
        world_id: WorldId,
    },
    #[error(
        "command '{command_id}' for world {world_id} in universe {universe_id} is not registered"
    )]
    UnknownCommand {
        universe_id: UniverseId,
        world_id: WorldId,
        command_id: String,
    },
    #[error(
        "world epoch mismatch for world {world_id} in universe {universe_id}: expected {expected}, got {got}"
    )]
    WorldEpochMismatch {
        universe_id: UniverseId,
        world_id: WorldId,
        expected: u64,
        got: u64,
    },
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SupervisorOutcome {
    pub frames_appended: usize,
    pub checkpoints_published: usize,
    pub registered_worlds: usize,
    pub pending_submissions: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SupervisorRunProfile {
    pub total: std::time::Duration,
    pub sync_assignments: std::time::Duration,
    pub sync_active_worlds: std::time::Duration,
    pub run_partitions: std::time::Duration,
    pub publish_checkpoints: std::time::Duration,
    pub partition_drain_submissions: std::time::Duration,
    pub partition_process_create: std::time::Duration,
    pub partition_process_existing: std::time::Duration,
    pub partition_activate_world: std::time::Duration,
    pub partition_apply_submission: std::time::Duration,
    pub partition_build_external_event: std::time::Duration,
    pub partition_host_drain: std::time::Duration,
    pub partition_post_apply: std::time::Duration,
    pub partition_commit_batch: std::time::Duration,
    pub partition_commit_command_records: std::time::Duration,
    pub partition_promote_worlds: std::time::Duration,
    pub partition_inline_checkpoint: std::time::Duration,
    pub assigned_partitions: usize,
    pub newly_assigned_partitions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitEventRequest {
    pub universe_id: UniverseId,
    pub world_id: WorldId,
    pub schema: String,
    pub value: JsonValue,
    #[serde(default)]
    pub submission_id: Option<String>,
    #[serde(default)]
    pub expected_world_epoch: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionAccepted {
    pub submission_id: String,
    pub submission_offset: u64,
    pub world_epoch: u64,
    pub effective_partition: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorldAccepted {
    pub submission_id: String,
    pub submission_offset: u64,
    pub world_id: WorldId,
    pub effective_partition: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostedWorldSummary {
    pub universe_id: UniverseId,
    pub world_id: WorldId,
    pub world_root: String,
    pub manifest_hash: String,
    pub world_epoch: u64,
    pub effective_partition: u32,
    pub next_world_seq: u64,
    pub workflow_modules: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct HostedWorldMetadata {
    pub workflow_modules: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ProjectionContinuity {
    pub world_epoch: u64,
    pub last_projected_head: u64,
    pub active_baseline: SnapshotRecord,
}

pub(super) struct RegisteredWorld {
    pub universe_id: UniverseId,
    pub store: Arc<HostedCas>,
    pub loaded: LoadedManifest,
    pub manifest_hash: String,
    pub world_epoch: u64,
    pub projection_token: String,
    pub projection_continuity: Option<ProjectionContinuity>,
    pub disabled_reason: Option<String>,
    pub metadata: HostedWorldMetadata,
}

pub(super) struct ActiveWorld {
    pub host: WorldHost<HostedCas>,
    pub accepted_submission_ids: BTreeSet<String>,
    pub last_checkpointed_head: u64,
    pub last_checkpointed_at_ns: u64,
    pub projection_bootstrapped: bool,
}

pub(super) struct PendingCreatedWorld {
    pub registered: RegisteredWorld,
    pub host: WorldHost<HostedCas>,
    pub accepted_submission_ids: BTreeSet<String>,
    pub total_open_ms: u128,
}

pub(super) struct HostedWorkerInfra {
    pub default_universe_id: UniverseId,
    pub paths: aos_node::LocalStatePaths,
    pub blobstore_config: crate::blobstore::BlobStoreConfig,
    pub vault: HostedVault,
    pub world_config: WorldConfig,
    pub kafka: HostedKafkaBackend,
    pub stores_by_domain: std::collections::BTreeMap<UniverseId, Arc<HostedCas>>,
    pub blob_meta_by_domain: std::collections::BTreeMap<UniverseId, HostedBlobMetaStore>,
}

#[derive(Default)]
pub(super) struct HostedWorkerState {
    pub registered_worlds: std::collections::BTreeMap<WorldId, RegisteredWorld>,
    pub active_worlds: std::collections::BTreeMap<WorldId, ActiveWorld>,
}

pub(super) struct HostedWorkerRuntimeInner {
    pub infra: HostedWorkerInfra,
    pub state: HostedWorkerState,
}

#[derive(Debug, Clone, Default)]
pub(super) struct PartitionRunOutcome {
    pub frames_appended: usize,
    pub checkpoint_event_frames: usize,
    pub inline_checkpoint_published: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct PartitionRunProfile {
    pub drain_submissions: std::time::Duration,
    pub process_create: std::time::Duration,
    pub process_existing: std::time::Duration,
    pub activate_world: std::time::Duration,
    pub apply_submission: std::time::Duration,
    pub build_external_event: std::time::Duration,
    pub host_drain: std::time::Duration,
    pub post_apply: std::time::Duration,
    pub commit_batch: std::time::Duration,
    pub commit_command_records: std::time::Duration,
    pub promote_worlds: std::time::Duration,
    pub inline_checkpoint: std::time::Duration,
}

pub(super) type CommandRollbackRecords =
    std::collections::BTreeMap<(WorldId, String), CommandRecord>;
pub(super) type SubmissionRollbackIds = std::collections::BTreeMap<WorldId, BTreeSet<String>>;
pub(super) type CommitCommandRecords = Vec<(WorldId, CommandRecord)>;
pub(super) type PendingCreatedWorlds = std::collections::BTreeMap<WorldId, PendingCreatedWorld>;
