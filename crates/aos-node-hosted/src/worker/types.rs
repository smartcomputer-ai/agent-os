use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::sync::Arc;

use aos_kernel::cell_index::CellMeta;
use aos_kernel::{Kernel, KernelError, LoadedManifest, StoreError};
use aos_node::{
    BackendError, EffectRuntime, EffectRuntimeEvent, RuntimeError, SharedEffectRuntime,
    SnapshotRecord, TimerScheduler, UniverseId, WorldConfig, WorldId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::blobstore::{HostedBlobMetaStore, HostedCas};
use crate::config::ProjectionCommitMode;
use crate::kafka::HostedKafkaBackend;
use crate::vault::HostedVault;

use super::core::{FlushLimits, SchedulerMsg, SchedulerState, SliceId, WorkItem};

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error(transparent)]
    LogFirst(#[from] BackendError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
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
    #[error(
        "world {world_id} is not strictly quiescent for {operation} (non_terminal_workflow_instances={non_terminal_workflow_instances}, inflight_workflow_intents={inflight_workflow_intents}, pending_workflow_receipts={pending_workflow_receipts}, queued_effects={queued_effects}, workflow_queue_pending={workflow_queue_pending}, mailbox_len={mailbox_len}, running={running}, commit_blocked={commit_blocked}, pending_slice={pending_slice}, scheduled_timers={scheduled_timers})"
    )]
    StrictQuiescenceBlocked {
        world_id: WorldId,
        operation: &'static str,
        non_terminal_workflow_instances: usize,
        inflight_workflow_intents: usize,
        pending_workflow_receipts: usize,
        queued_effects: usize,
        workflow_queue_pending: bool,
        mailbox_len: usize,
        running: bool,
        commit_blocked: bool,
        pending_slice: bool,
        scheduled_timers: usize,
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

impl SupervisorRunProfile {
    pub fn merge(&mut self, other: Self) {
        self.total += other.total;
        self.sync_assignments += other.sync_assignments;
        self.sync_active_worlds += other.sync_active_worlds;
        self.run_partitions += other.run_partitions;
        self.publish_checkpoints += other.publish_checkpoints;
        self.partition_drain_submissions += other.partition_drain_submissions;
        self.partition_process_create += other.partition_process_create;
        self.partition_process_existing += other.partition_process_existing;
        self.partition_activate_world += other.partition_activate_world;
        self.partition_apply_submission += other.partition_apply_submission;
        self.partition_build_external_event += other.partition_build_external_event;
        self.partition_host_drain += other.partition_host_drain;
        self.partition_post_apply += other.partition_post_apply;
        self.partition_commit_batch += other.partition_commit_batch;
        self.partition_commit_command_records += other.partition_commit_command_records;
        self.partition_promote_worlds += other.partition_promote_worlds;
        self.partition_inline_checkpoint += other.partition_inline_checkpoint;
        self.assigned_partitions += other.assigned_partitions;
        self.newly_assigned_partitions += other.newly_assigned_partitions;
    }

    pub fn has_activity(&self) -> bool {
        !self.total.is_zero()
            || !self.sync_assignments.is_zero()
            || !self.sync_active_worlds.is_zero()
            || !self.run_partitions.is_zero()
            || !self.publish_checkpoints.is_zero()
            || !self.partition_drain_submissions.is_zero()
            || !self.partition_process_create.is_zero()
            || !self.partition_process_existing.is_zero()
            || !self.partition_activate_world.is_zero()
            || !self.partition_apply_submission.is_zero()
            || !self.partition_build_external_event.is_zero()
            || !self.partition_host_drain.is_zero()
            || !self.partition_post_apply.is_zero()
            || !self.partition_commit_batch.is_zero()
            || !self.partition_commit_command_records.is_zero()
            || !self.partition_promote_worlds.is_zero()
            || !self.partition_inline_checkpoint.is_zero()
            || self.assigned_partitions > 0
            || self.newly_assigned_partitions > 0
    }
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
    pub effect_runtime: EffectRuntime<WorldId>,
    pub world_epoch: u64,
    pub projection_token: String,
    pub projection_continuity: Option<ProjectionContinuity>,
    pub disabled_reason: Option<String>,
    pub metadata: HostedWorldMetadata,
}

pub(super) struct ActiveWorld {
    pub world_id: WorldId,
    pub universe_id: UniverseId,
    pub created_at_ns: u64,
    pub world_epoch: u64,
    pub active_baseline: SnapshotRecord,
    pub next_world_seq: u64,
    pub kernel: Kernel<HostedCas>,
    pub accepted_submission_ids: BTreeSet<String>,
    pub mailbox: VecDeque<WorkItem>,
    pub ready: bool,
    pub running: bool,
    pub commit_blocked: bool,
    pub pending_slice: Option<SliceId>,
    pub pending_slices: VecDeque<SliceId>,
    pub disabled_reason: Option<String>,
    pub last_checkpointed_head: u64,
    pub last_checkpointed_at_ns: u64,
    pub pending_create_checkpoint: bool,
    pub projection_bootstrapped: bool,
}

impl ActiveWorld {
    pub fn journal_head(&self) -> u64 {
        self.kernel.journal_head()
    }

    pub fn state(&self, workflow: &str, key: Option<&[u8]>) -> Option<Vec<u8>> {
        self.kernel
            .workflow_state_bytes(workflow, key)
            .ok()
            .flatten()
    }

    pub fn list_cells(&self, workflow: &str) -> Result<Vec<CellMeta>, WorkerError> {
        self.kernel
            .list_cells(workflow)
            .map_err(WorkerError::Kernel)
    }

    pub fn drain_workspace_projection_deltas(
        &mut self,
    ) -> Result<Vec<aos_kernel::WorkspaceProjectionDelta>, WorkerError> {
        self.kernel
            .drain_workspace_projection_deltas()
            .map_err(WorkerError::Kernel)
    }

    pub fn drain_cell_projection_deltas(&mut self) -> Vec<aos_kernel::CellProjectionDelta> {
        self.kernel.drain_cell_projection_deltas()
    }
}

pub(super) struct AsyncWorldState {
    pub timer_scheduler: TimerScheduler,
    pub scheduled_timers: HashSet<[u8; 32]>,
    pub timer_tasks: BTreeMap<[u8; 32], JoinHandle<()>>,
}

impl AsyncWorldState {
    pub fn abort_all_timers(&mut self) {
        for (_, handle) in std::mem::take(&mut self.timer_tasks) {
            handle.abort();
        }
    }
}

pub(super) struct PendingCreatedWorld;

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

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct HostedStrictQuiescence {
    pub non_terminal_workflow_instances: usize,
    pub inflight_workflow_intents: usize,
    pub pending_workflow_receipts: usize,
    pub queued_effects: usize,
    pub workflow_queue_pending: bool,
    pub mailbox_len: usize,
    pub running: bool,
    pub commit_blocked: bool,
    pub pending_slice: bool,
    pub scheduled_timers: usize,
}

#[derive(Default)]
pub(super) struct HostedWorkerState {
    pub registered_worlds: std::collections::BTreeMap<WorldId, RegisteredWorld>,
    pub active_worlds: std::collections::BTreeMap<WorldId, ActiveWorld>,
    pub async_worlds: std::collections::BTreeMap<WorldId, AsyncWorldState>,
    pub pending_created_worlds: PendingCreatedWorlds,
    pub projection_dirty_worlds: VecDeque<WorldId>,
    pub projection_dirty_set: BTreeSet<WorldId>,
    pub ready_worlds: VecDeque<WorldId>,
    pub scheduler: SchedulerState,
    pub assigned_partitions: BTreeSet<u32>,
    pub next_slice_id: u64,
}

pub(super) struct HostedWorkerCore {
    pub infra: HostedWorkerInfra,
    pub state: HostedWorkerState,
    pub effect_event_tx: mpsc::Sender<EffectRuntimeEvent<WorldId>>,
    pub effect_event_rx: Option<mpsc::Receiver<EffectRuntimeEvent<WorldId>>>,
    pub shared_effect_runtimes: BTreeMap<UniverseId, SharedEffectRuntime<WorldId>>,
    pub scheduler_tx: Option<mpsc::UnboundedSender<SchedulerMsg>>,
    pub flush_limits: FlushLimits,
    pub max_local_continuation_slices_per_flush: usize,
    pub projection_commit_mode: ProjectionCommitMode,
    pub max_uncommitted_slices_per_world: usize,
    pub debug_skip_flush_commit: bool,
    pub debug_fail_after_next_flush_commit: bool,
}

pub(super) type PendingCreatedWorlds = std::collections::BTreeMap<WorldId, PendingCreatedWorld>;
