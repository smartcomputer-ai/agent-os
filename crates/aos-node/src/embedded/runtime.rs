use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::api::{
    ControlError, DefGetResponse, DefsListResponse, HeadInfoResponse, JournalEntriesResponse,
    JournalEntryResponse, ManifestResponse, RawJournalEntriesResponse, RawJournalEntryResponse,
    StateCellSummary, StateGetResponse, StateListResponse, WorkspaceApplyOp, WorkspaceApplyRequest,
    WorkspaceApplyResponse, WorkspaceResolveResponse,
};
use crate::{
    CborPayload, CommandErrorBody, CommandIngress, CommandRecord, CommandStatus,
    CreateWorldRequest, CreateWorldSource, DomainEventIngress, ForkWorldRequest, HostControl,
    InboxSeq, PersistError, SeedKind, SnapshotSelector, UniverseId, WorldCreateResult, WorldId,
    WorldRecord, WorldRuntimeInfo, rewrite_snapshot_for_fork_policy,
};
use crate::{
    EffectExecutionClass, EffectRuntime, EffectRuntimeEvent, RuntimeError, TimerEntry,
    TimerScheduler, WorldConfig,
};
use crate::{SubmissionEnvelope, SubmissionPayload, WorldLogFrame};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_effect_types::HashRef;
use aos_effects::builtins::TimerSetReceipt;
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use aos_kernel::governance::ManifestPatch;
use aos_kernel::governance_utils::canonicalize_patch;
use aos_kernel::journal::{
    ApprovalDecisionRecord, Journal, JournalRecord, SnapshotRecord as KernelSnapshotRecord,
};
use aos_kernel::patch_doc::{PatchDocument, compile_patch_document};
use aos_kernel::{
    Consistency, Kernel, KernelConfig, KernelDrain, KernelError, LoadedManifest, ManifestLoader,
    StateReader, Store, StoreError, TraceQuery, WorldControl, WorldInput, trace_get,
    workflow_trace_summary_with_routes,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::runtime::{Builder as RuntimeBuilder, Handle, Runtime};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};
use uuid::Uuid;

use super::workspace as local_workspace;
use super::{
    FsCas, LocalBlobBackend, LocalSqliteBackend, LocalStatePaths, LocalStoreError,
    secrets::local_secret_resolver_for_manifest,
};

#[derive(Debug, Error)]
pub enum LocalRuntimeError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Cbor(#[from] serde_cbor::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Ref(#[from] aos_effect_types::RefError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("invalid hash reference '{0}'")]
    InvalidHashRef(String),
    #[error("backend error: {0}")]
    Backend(String),
}

impl From<LocalStoreError> for LocalRuntimeError {
    fn from(value: LocalStoreError) -> Self {
        match value {
            LocalStoreError::Io(err) => Self::Io(err),
            LocalStoreError::Persist(err) => Self::Backend(err.to_string()),
            LocalStoreError::Sqlite(err) => Self::Sqlite(err),
            LocalStoreError::Cbor(err) => Self::Cbor(err),
            LocalStoreError::Backend(message) => Self::Backend(message),
        }
    }
}

impl From<PersistError> for LocalRuntimeError {
    fn from(value: PersistError) -> Self {
        Self::Backend(value.to_string())
    }
}

impl From<LocalRuntimeError> for ControlError {
    fn from(value: LocalRuntimeError) -> Self {
        match value {
            LocalRuntimeError::InvalidHashRef(message) => ControlError::invalid(message),
            LocalRuntimeError::Backend(message) => ControlError::invalid(message),
            LocalRuntimeError::Kernel(err) => ControlError::Kernel(err),
            LocalRuntimeError::Store(err) => ControlError::Store(err),
            LocalRuntimeError::Cbor(err) => ControlError::Cbor(err),
            LocalRuntimeError::Json(err) => ControlError::Json(err),
            LocalRuntimeError::Ref(err) => ControlError::from(err),
            other => ControlError::invalid(other.to_string()),
        }
    }
}

struct WorldSlot {
    world_id: WorldId,
    universe_id: crate::UniverseId,
    created_at_ns: u64,
    initial_manifest_hash: String,
    world_epoch: u64,
    active_baseline: crate::SnapshotRecord,
    next_world_seq: u64,
    last_checkpointed_at_ns: u64,
    kernel: Kernel<FsCas>,
    effect_runtime: EffectRuntime<WorldId>,
    timer_scheduler: TimerScheduler,
    scheduled_timers: HashSet<[u8; 32]>,
    mailbox: VecDeque<SubmissionEnvelope>,
    ready: bool,
    command_records: BTreeMap<String, CommandRecord>,
}

struct RuntimeState {
    sqlite: LocalSqliteBackend,
    next_submission_seq: u64,
    next_frame_offset: u64,
    worlds: BTreeMap<WorldId, WorldSlot>,
    ready_worlds: VecDeque<WorldId>,
}

#[derive(Debug, Clone, Copy)]
struct LocalCheckpointConfig {
    interval: Duration,
    every_events: Option<u32>,
}

impl Default for LocalCheckpointConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60 * 2),
            every_events: Some(2000),
        }
    }
}

impl LocalCheckpointConfig {
    fn from_env() -> Result<Self, LocalRuntimeError> {
        let mut cfg = Self::default();
        if let Ok(raw) = std::env::var("AOS_CHECKPOINT_INTERVAL_MS") {
            cfg.interval =
                Duration::from_millis(parse_u64_env("AOS_CHECKPOINT_INTERVAL_MS", &raw)?);
        }
        if let Ok(raw) = std::env::var("AOS_CHECKPOINT_EVERY_EVENTS") {
            let parsed = parse_u64_env("AOS_CHECKPOINT_EVERY_EVENTS", &raw)?;
            cfg.every_events = if parsed == 0 {
                None
            } else {
                Some(u32::try_from(parsed).map_err(|_| {
                    LocalRuntimeError::Backend(format!(
                        "invalid AOS_CHECKPOINT_EVERY_EVENTS value '{raw}': exceeds u32"
                    ))
                })?)
            };
        }
        Ok(cfg)
    }
}

pub(crate) struct TimerWake {
    pub(crate) world_id: WorldId,
}

struct EdgeRuntime {
    handle: Handle,
    _owned_runtime: Option<Runtime>,
}

impl EdgeRuntime {
    fn owned() -> Result<Self, LocalRuntimeError> {
        let runtime = RuntimeBuilder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|err| {
                LocalRuntimeError::Backend(format!("build embedded edge runtime: {err}"))
            })?;
        let handle = runtime.handle().clone();
        Ok(Self {
            handle,
            _owned_runtime: Some(runtime),
        })
    }

    fn from_handle(handle: Handle) -> Self {
        Self {
            handle,
            _owned_runtime: None,
        }
    }

    fn handle(&self) -> &Handle {
        &self.handle
    }
}

pub struct LocalRuntime {
    paths: LocalStatePaths,
    cas: Arc<FsCas>,
    world_config: WorldConfig,
    checkpoint_config: LocalCheckpointConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
    edge_runtime: EdgeRuntime,
    effect_event_tx: mpsc::Sender<EffectRuntimeEvent<WorldId>>,
    effect_event_rx: Mutex<Option<mpsc::Receiver<EffectRuntimeEvent<WorldId>>>>,
    timer_wake_tx: mpsc::Sender<TimerWake>,
    timer_wake_rx: Mutex<Option<mpsc::Receiver<TimerWake>>>,
    inner: Mutex<RuntimeState>,
}

impl LocalRuntime {
    fn log_world_created(
        &self,
        world: &WorldSlot,
        source_kind: &'static str,
        total_create_ms: u128,
        initial_record_count: usize,
    ) {
        tracing::info!(
            universe_id = %world.universe_id,
            world_id = %world.world_id,
            world_epoch = world.world_epoch,
            source_kind,
            total_create_ms,
            created_at_ns = world.created_at_ns,
            active_baseline_height = world.active_baseline.height,
            next_world_seq = world.next_world_seq,
            initial_record_count,
            manifest_hash = %world.initial_manifest_hash,
            "aos-node-local world created"
        );
    }

    pub fn state_root(&self) -> &std::path::Path {
        self.paths.root()
    }

    fn log_world_opened(
        &self,
        world: &WorldSlot,
        trigger: &'static str,
        total_open_ms: u128,
        frame_count: usize,
        replay_frame_count: usize,
    ) {
        tracing::info!(
            universe_id = %world.universe_id,
            world_id = %world.world_id,
            world_epoch = world.world_epoch,
            trigger,
            total_open_ms,
            active_baseline_height = world.active_baseline.height,
            next_world_seq = world.next_world_seq,
            frame_count,
            replay_frame_count,
            "aos-node-local world opened"
        );
    }

    fn log_world_checkpointed(
        &self,
        world: &WorldSlot,
        trigger: &'static str,
        total_checkpoint_ms: u128,
        retained_record_count: usize,
        checkpoint_world_seq_end: u64,
    ) {
        tracing::info!(
            universe_id = %world.universe_id,
            world_id = %world.world_id,
            world_epoch = world.world_epoch,
            trigger,
            total_checkpoint_ms,
            active_baseline_height = world.active_baseline.height,
            next_world_seq = world.next_world_seq,
            retained_record_count,
            checkpoint_world_seq_end,
            "aos-node-local checkpoint completed"
        );
    }

    pub fn open(paths: LocalStatePaths) -> Result<Arc<Self>, LocalRuntimeError> {
        let mut world_config =
            WorldConfig::from_env_with_fallback_module_cache_dir(Some(paths.wasmtime_cache_dir()));
        world_config.eager_module_load = true;
        Self::open_with_config(
            paths,
            world_config,
            EffectAdapterConfig::default(),
            KernelConfig::default(),
        )
    }

    pub fn open_with_handle(
        paths: LocalStatePaths,
        edge_handle: Handle,
    ) -> Result<Arc<Self>, LocalRuntimeError> {
        let mut world_config =
            WorldConfig::from_env_with_fallback_module_cache_dir(Some(paths.wasmtime_cache_dir()));
        world_config.eager_module_load = true;
        Self::open_with_edge_runtime(
            paths,
            world_config,
            EffectAdapterConfig::default(),
            KernelConfig::default(),
            EdgeRuntime::from_handle(edge_handle),
        )
    }

    pub fn open_with_config(
        paths: LocalStatePaths,
        world_config: WorldConfig,
        adapter_config: EffectAdapterConfig,
        kernel_config: KernelConfig,
    ) -> Result<Arc<Self>, LocalRuntimeError> {
        Self::open_with_edge_runtime(
            paths,
            world_config,
            adapter_config,
            kernel_config,
            EdgeRuntime::owned()?,
        )
    }

    fn open_with_edge_runtime(
        paths: LocalStatePaths,
        world_config: WorldConfig,
        adapter_config: EffectAdapterConfig,
        kernel_config: KernelConfig,
        edge_runtime: EdgeRuntime,
    ) -> Result<Arc<Self>, LocalRuntimeError> {
        let kernel_config = world_config.apply_kernel_defaults(kernel_config);
        let checkpoint_config = LocalCheckpointConfig::from_env()?;
        paths.ensure_root()?;
        std::fs::create_dir_all(paths.cache_root())?;
        std::fs::create_dir_all(paths.run_dir())?;
        std::fs::create_dir_all(paths.logs_dir())?;
        let blob_backend = LocalBlobBackend::from_paths(&paths)?;
        let cas = blob_backend.cas();
        let sqlite = LocalSqliteBackend::from_paths(&paths)?;
        let (next_submission_seq, next_frame_offset) = sqlite.load_runtime_meta()?;
        let inner = RuntimeState {
            sqlite,
            next_submission_seq,
            next_frame_offset,
            worlds: BTreeMap::new(),
            ready_worlds: VecDeque::new(),
        };
        let (effect_event_tx, effect_event_rx) = mpsc::channel(256);
        let (timer_wake_tx, timer_wake_rx) = mpsc::channel(256);
        let runtime = Arc::new(Self {
            paths,
            cas,
            world_config,
            checkpoint_config,
            adapter_config,
            kernel_config,
            edge_runtime,
            effect_event_tx,
            effect_event_rx: Mutex::new(Some(effect_event_rx)),
            timer_wake_tx,
            timer_wake_rx: Mutex::new(Some(timer_wake_rx)),
            inner: Mutex::new(inner),
        });
        {
            let mut inner = runtime.inner.lock().expect("local runtime mutex poisoned");
            runtime.load_hot_worlds_locked(&mut inner)?;
        }
        runtime.process_all_pending()?;
        Ok(runtime)
    }

    pub fn paths(&self) -> &LocalStatePaths {
        &self.paths
    }

    pub fn store(&self) -> Arc<FsCas> {
        Arc::clone(&self.cas)
    }

    pub fn world_config(&self) -> &WorldConfig {
        &self.world_config
    }

    pub fn adapter_config(&self) -> &EffectAdapterConfig {
        &self.adapter_config
    }

    pub(crate) fn checkpoint_interval(&self) -> Duration {
        self.checkpoint_config.interval
    }

    pub fn kernel_config(&self) -> &KernelConfig {
        &self.kernel_config
    }

    pub fn put_blob(&self, bytes: &[u8]) -> Result<Hash, LocalRuntimeError> {
        Ok(self.cas.put_verified(bytes)?)
    }

    pub fn blob_metadata(&self, hash: Hash) -> Result<bool, LocalRuntimeError> {
        Ok(self.cas.has(hash))
    }

    pub fn get_blob(&self, hash: Hash) -> Result<Vec<u8>, LocalRuntimeError> {
        Ok(self.cas.get(hash)?)
    }

    pub fn create_world(
        &self,
        request: CreateWorldRequest,
    ) -> Result<WorldCreateResult, LocalRuntimeError> {
        let request = localize_create_request(request);
        let world_id = request
            .world_id
            .unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        self.create_world_from_request_locked(&mut inner, world_id, request)?;
        let result = WorldCreateResult {
            record: self.world_record_locked(&inner, world_id)?,
        };
        drop(inner);
        self.process_all_pending()?;
        Ok(result)
    }

    pub fn fork_world(
        &self,
        request: ForkWorldRequest,
    ) -> Result<WorldCreateResult, LocalRuntimeError> {
        crate::validate_fork_world_request(&request)?;
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world_id = request
            .new_world_id
            .unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
        let create_request =
            self.create_fork_seed_request_locked(&mut inner, world_id, &request)?;
        self.create_world_from_request_locked(&mut inner, world_id, create_request)?;
        let result = WorldCreateResult {
            record: self.world_record_locked(&inner, world_id)?,
        };
        drop(inner);
        self.process_all_pending()?;
        Ok(result)
    }

    fn kernel_config_for_create_request(
        &self,
        request: &CreateWorldRequest,
    ) -> Result<KernelConfig, LocalRuntimeError> {
        let mut kernel_config = self.kernel_config.clone();
        if kernel_config.secret_resolver.is_some() {
            return Ok(kernel_config);
        }

        let manifest_hash = match &request.source {
            crate::CreateWorldSource::Manifest { manifest_hash } => manifest_hash.clone(),
            crate::CreateWorldSource::Seed { seed } => {
                seed.baseline.manifest_hash.clone().ok_or_else(|| {
                    LocalRuntimeError::Backend(
                        "seed baseline requires manifest_hash for local restore".into(),
                    )
                })?
            }
        };
        let manifest_hash = parse_plane_hash_like(&manifest_hash, "manifest_hash")?;
        let loaded = ManifestLoader::load_from_hash(self.cas.as_ref(), manifest_hash)?;
        if let Some(resolver) = local_secret_resolver_for_manifest(&self.paths, &loaded)? {
            kernel_config.secret_resolver = Some(resolver);
        }
        Ok(kernel_config)
    }

    pub fn world_summary(
        &self,
        world_id: WorldId,
    ) -> Result<(WorldRuntimeInfo, crate::SnapshotRecord), LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        self.world_summary_locked(&inner, world_id)
    }

    pub fn world_runtime(&self, world_id: WorldId) -> Result<WorldRuntimeInfo, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        Ok(runtime_info_from_world(world))
    }

    pub fn list_worlds(
        &self,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let mut worlds = inner
            .worlds
            .iter()
            .filter(|(world_id, _)| after.is_none_or(|after| **world_id > after))
            .map(|(_, world)| runtime_info_from_world(world))
            .collect::<Vec<_>>();
        worlds.sort_by_key(|world| world.world_id);
        worlds.truncate(limit as usize);
        Ok(worlds)
    }

    pub fn worker_worlds(&self, limit: u32) -> Result<Vec<WorldRuntimeInfo>, LocalRuntimeError> {
        self.list_worlds(None, limit)
    }

    pub fn get_command(
        &self,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        world
            .command_records
            .get(command_id)
            .cloned()
            .ok_or_else(|| LocalRuntimeError::Backend(format!("command '{command_id}' not found")))
    }

    pub fn submit_command<T: Serialize>(
        &self,
        world_id: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        payload: &T,
    ) -> Result<CommandRecord, LocalRuntimeError> {
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        if let Some(existing_id) = command_id.as_deref()
            && let Some(existing) = inner
                .worlds
                .get(&world_id)
                .and_then(|world| world.command_records.get(existing_id))
                .cloned()
        {
            return Ok(existing);
        }
        let (command_id, submission) = self.prepare_command_submission_locked(
            &mut inner, world_id, command, command_id, actor, payload,
        )?;
        self.enqueue_world_submission_locked(&mut inner, submission)?;
        drop(inner);
        self.process_all_pending()?;
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        inner
            .worlds
            .get(&world_id)
            .and_then(|world| world.command_records.get(&command_id))
            .cloned()
            .ok_or_else(|| {
                LocalRuntimeError::Backend(format!("command '{command_id}' not found after submit"))
            })
    }

    pub fn queue_command_submission<T: Serialize>(
        &self,
        world_id: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        payload: &T,
    ) -> Result<(Option<SubmissionEnvelope>, CommandRecord), LocalRuntimeError> {
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        if let Some(existing_id) = command_id.as_deref()
            && let Some(existing) = inner
                .worlds
                .get(&world_id)
                .and_then(|world| world.command_records.get(existing_id))
                .cloned()
        {
            return Ok((None, existing));
        }
        let (command_id, submission) = self.prepare_command_submission_locked(
            &mut inner, world_id, command, command_id, actor, payload,
        )?;
        self.enqueue_world_submission_locked(&mut inner, submission.clone())?;
        let record = inner
            .worlds
            .get(&world_id)
            .and_then(|world| world.command_records.get(&command_id))
            .cloned()
            .ok_or_else(|| {
                LocalRuntimeError::Backend(format!("command '{command_id}' not found after queue"))
            })?;
        Ok((Some(submission), record))
    }

    pub fn enqueue_event(
        &self,
        world_id: WorldId,
        ingress: DomainEventIngress,
    ) -> Result<InboxSeq, LocalRuntimeError> {
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        let submission = self.build_event_submission_locked(&inner, world_id, ingress)?;
        let submit_seq = self.allocate_submission_seq_locked(&mut inner)?;
        self.enqueue_world_submission_locked(&mut inner, submission)?;
        Ok(InboxSeq::from_u64(submit_seq))
    }

    pub fn enqueue_receipt(
        &self,
        world_id: WorldId,
        ingress: crate::ReceiptIngress,
    ) -> Result<InboxSeq, LocalRuntimeError> {
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        let submission = self.build_receipt_submission_locked(&inner, world_id, ingress)?;
        let submit_seq = self.allocate_submission_seq_locked(&mut inner)?;
        self.enqueue_world_submission_locked(&mut inner, submission)?;
        Ok(InboxSeq::from_u64(submit_seq))
    }

    pub fn checkpoint_world(
        &self,
        world_id: WorldId,
    ) -> Result<(WorldRuntimeInfo, crate::SnapshotRecord), LocalRuntimeError> {
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        self.drain_pending_locked(&mut inner)?;
        self.checkpoint_world_locked(&mut inner, world_id, "manual")?;
        self.world_summary_locked(&inner, world_id)
    }

    pub fn process_all_pending(&self) -> Result<(), LocalRuntimeError> {
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        self.process_all_pending_locked(&mut inner)
    }

    fn process_all_pending_locked(
        &self,
        inner: &mut RuntimeState,
    ) -> Result<(), LocalRuntimeError> {
        self.drain_pending_locked(inner)?;
        self.checkpoint_idle_worlds_locked(inner)
    }

    fn drain_pending_locked(&self, inner: &mut RuntimeState) -> Result<(), LocalRuntimeError> {
        loop {
            self.drain_async_events_locked(inner)?;
            let Some(world_id) = inner.ready_worlds.pop_front() else {
                break;
            };
            let submission = {
                let world = inner.worlds.get_mut(&world_id).ok_or_else(|| {
                    LocalRuntimeError::Backend(format!("world {world_id} not found"))
                })?;
                world.ready = false;
                world.mailbox.pop_front()
            };
            let Some(submission) = submission else {
                continue;
            };
            self.process_submission_locked(inner, submission)?;
            if inner
                .worlds
                .get(&world_id)
                .is_some_and(|world| !world.mailbox.is_empty())
            {
                self.mark_world_ready_locked(inner, world_id)?;
            }
        }
        Ok(())
    }

    fn checkpoint_idle_worlds_locked(
        &self,
        inner: &mut RuntimeState,
    ) -> Result<(), LocalRuntimeError> {
        let now_ns = now_wallclock_ns();
        let world_ids = inner
            .worlds
            .iter()
            .filter_map(|(&world_id, world)| {
                let journal_bounds = world.kernel.journal_bounds();
                let quiescence = world.kernel.quiescence_status();
                (journal_bounds.next_seq > journal_bounds.retained_from
                    && !world.ready
                    && world.mailbox.is_empty()
                    && quiescence.runtime_quiescent
                    && world.scheduled_timers.is_empty()
                    && self.checkpoint_due(world, now_ns))
                .then_some(world_id)
            })
            .collect::<Vec<_>>();
        for world_id in world_ids {
            self.checkpoint_world_locked(inner, world_id, "auto")?;
        }
        Ok(())
    }

    fn prepare_command_submission_locked<T: Serialize>(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        payload: &T,
    ) -> Result<(String, SubmissionEnvelope), LocalRuntimeError> {
        let command_id = command_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let submitted_at_ns = now_wallclock_ns();
        let payload = CborPayload::inline(to_canonical_cbor(payload)?);
        let world_epoch = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?
            .world_epoch;
        let ingress = CommandIngress {
            command_id: command_id.clone(),
            command: command.to_string(),
            actor,
            payload,
            submitted_at_ns,
        };
        let queued = CommandRecord {
            command_id: command_id.clone(),
            command: command.to_string(),
            status: CommandStatus::Queued,
            submitted_at_ns,
            started_at_ns: None,
            finished_at_ns: None,
            journal_height: None,
            manifest_hash: None,
            result_payload: None,
            error: None,
        };
        let world = inner
            .worlds
            .get_mut(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        world
            .command_records
            .insert(command_id.clone(), queued.clone());
        inner.sqlite.persist_command_projection(world_id, &queued)?;
        let control = self.world_control_from_command_payload(command, &ingress.payload)?;
        Ok((
            command_id.clone(),
            SubmissionEnvelope::world_control(
                command_id,
                local_submission_universe_id(),
                world_id,
                world_epoch,
                ingress,
                control,
            ),
        ))
    }

    pub fn manifest(&self, world_id: WorldId) -> Result<ManifestResponse, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        let manifest = world.kernel.get_manifest(Consistency::Head)?.value;
        Ok(ManifestResponse {
            journal_head: world.kernel.heights().head,
            manifest_hash: world.kernel.manifest_hash().to_hex(),
            manifest,
        })
    }

    pub fn defs_list(
        &self,
        world_id: WorldId,
        kinds: Option<Vec<String>>,
        prefix: Option<String>,
    ) -> Result<DefsListResponse, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        Ok(DefsListResponse {
            journal_head: world.kernel.heights().head,
            manifest_hash: world.kernel.manifest_hash().to_hex(),
            defs: world.kernel.list_defs(kinds.as_deref(), prefix.as_deref()),
        })
    }

    pub fn def_get(
        &self,
        world_id: WorldId,
        name: &str,
    ) -> Result<DefGetResponse, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        let def = world
            .kernel
            .get_def(name)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("definition '{name}' not found")))?;
        Ok(DefGetResponse {
            journal_head: world.kernel.heights().head,
            manifest_hash: world.kernel.manifest_hash().to_hex(),
            def,
        })
    }

    pub fn state_get(
        &self,
        world_id: WorldId,
        workflow: &str,
        key: Option<Vec<u8>>,
    ) -> Result<StateGetResponse, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        let key_bytes = key.unwrap_or_default();
        let state_read = world.kernel.get_workflow_state(
            workflow,
            Some(key_bytes.as_slice()),
            Consistency::Head,
        )?;
        let state = state_read.value;
        let state_hash = state.as_ref().map(|bytes| Hash::of_bytes(bytes).to_hex());
        let size = state.as_ref().map(|bytes| bytes.len() as u64).unwrap_or(0);
        let cell = state_hash.map(|state_hash| StateCellSummary {
            journal_head: state_read.meta.journal_height,
            workflow: workflow.to_string(),
            key_hash: Hash::of_bytes(&key_bytes).as_bytes().to_vec(),
            key_bytes: key_bytes.clone(),
            state_hash,
            size,
            last_active_ns: 0,
        });
        Ok(StateGetResponse {
            journal_head: state_read.meta.journal_height,
            workflow: workflow.to_string(),
            key_b64: Some(BASE64_STANDARD.encode(&key_bytes)),
            cell,
            state_b64: state.map(|bytes| BASE64_STANDARD.encode(bytes)),
        })
    }

    pub fn state_list(
        &self,
        world_id: WorldId,
        workflow: &str,
        limit: u32,
    ) -> Result<StateListResponse, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        let mut cells = world
            .kernel
            .list_cells(workflow)?
            .into_iter()
            .map(|cell| StateCellSummary {
                journal_head: world.kernel.heights().head,
                workflow: workflow.to_string(),
                key_hash: cell.key_hash.to_vec(),
                key_bytes: cell.key_bytes,
                state_hash: Hash::from(cell.state_hash).to_hex(),
                size: cell.size,
                last_active_ns: cell.last_active_ns,
            })
            .collect::<Vec<_>>();
        cells.truncate(limit as usize);
        Ok(StateListResponse {
            journal_head: world.kernel.heights().head,
            workflow: workflow.to_string(),
            cells,
        })
    }

    pub fn trace(
        &self,
        world_id: WorldId,
        query: TraceQuery,
    ) -> Result<serde_json::Value, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        Ok(trace_get(&world.kernel, query)?)
    }

    pub fn trace_summary(&self, world_id: WorldId) -> Result<serde_json::Value, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        Ok(workflow_trace_summary_with_routes(&world.kernel, None)?)
    }

    pub fn workspace_resolve(
        &self,
        world_id: WorldId,
        workspace: &str,
        version: Option<u64>,
    ) -> Result<WorkspaceResolveResponse, LocalRuntimeError> {
        #[derive(Debug, Default, Deserialize)]
        struct WorkspaceHistoryState {
            latest: u64,
            versions: BTreeMap<u64, WorkspaceCommitMetaState>,
        }
        #[derive(Debug, Deserialize)]
        struct WorkspaceCommitMetaState {
            root_hash: String,
        }

        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        let key = serde_cbor::to_vec(&workspace.to_string())?;
        let history = world
            .kernel
            .get_workflow_state("sys/Workspace@1", Some(key.as_slice()), Consistency::Head)?
            .value;
        let receipt = if let Some(bytes) = history {
            let history: WorkspaceHistoryState = serde_cbor::from_slice(&bytes)?;
            let head = Some(history.latest);
            let target = version.unwrap_or(history.latest);
            if let Some(entry) = history.versions.get(&target) {
                aos_effect_types::WorkspaceResolveReceipt {
                    exists: true,
                    resolved_version: Some(target),
                    head,
                    root_hash: Some(HashRef::new(entry.root_hash.clone())?),
                }
            } else {
                aos_effect_types::WorkspaceResolveReceipt {
                    exists: false,
                    resolved_version: None,
                    head,
                    root_hash: None,
                }
            }
        } else {
            aos_effect_types::WorkspaceResolveReceipt {
                exists: false,
                resolved_version: None,
                head: None,
                root_hash: None,
            }
        };
        Ok(WorkspaceResolveResponse {
            workspace: workspace.to_string(),
            receipt,
        })
    }

    pub fn workspace_empty_root(&self) -> Result<HashRef, LocalRuntimeError> {
        Ok(local_workspace::empty_root(self.cas.as_ref())?)
    }

    pub fn workspace_entries(
        &self,
        root_hash: &HashRef,
        path: Option<&str>,
        scope: Option<&str>,
        cursor: Option<&str>,
        limit: u64,
    ) -> Result<aos_effect_types::WorkspaceListReceipt, LocalRuntimeError> {
        Ok(local_workspace::list(
            self.cas.as_ref(),
            root_hash,
            path,
            scope,
            cursor,
            limit,
        )?)
    }

    pub fn workspace_entry(
        &self,
        root_hash: &HashRef,
        path: &str,
    ) -> Result<aos_effect_types::WorkspaceReadRefReceipt, LocalRuntimeError> {
        Ok(local_workspace::read_ref(
            self.cas.as_ref(),
            root_hash,
            path,
        )?)
    }

    pub fn workspace_bytes(
        &self,
        root_hash: &HashRef,
        path: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Vec<u8>, LocalRuntimeError> {
        Ok(local_workspace::read_bytes(
            self.cas.as_ref(),
            root_hash,
            path,
            range,
        )?)
    }

    pub fn workspace_annotations(
        &self,
        root_hash: &HashRef,
        path: Option<&str>,
    ) -> Result<aos_effect_types::WorkspaceAnnotationsGetReceipt, LocalRuntimeError> {
        Ok(local_workspace::annotations_get(
            self.cas.as_ref(),
            root_hash,
            path,
        )?)
    }

    pub fn workspace_apply(
        &self,
        root_hash: HashRef,
        request: WorkspaceApplyRequest,
    ) -> Result<WorkspaceApplyResponse, LocalRuntimeError> {
        let base_root_hash = root_hash;
        let mut current = base_root_hash.clone();
        for operation in request.operations {
            current = match operation {
                WorkspaceApplyOp::WriteBytes {
                    path,
                    bytes_b64,
                    mode,
                } => {
                    let bytes = BASE64_STANDARD.decode(bytes_b64).map_err(|err| {
                        LocalRuntimeError::Backend(format!(
                            "decode workspace write_bytes payload: {err}"
                        ))
                    })?;
                    local_workspace::write_bytes(self.cas.as_ref(), &current, &path, &bytes, mode)?
                        .new_root_hash
                }
                WorkspaceApplyOp::WriteRef {
                    path,
                    blob_hash,
                    mode,
                } => {
                    local_workspace::write_ref(
                        self.cas.as_ref(),
                        &current,
                        &path,
                        &blob_hash,
                        mode,
                    )?
                    .new_root_hash
                }
                WorkspaceApplyOp::Remove { path } => {
                    local_workspace::remove(self.cas.as_ref(), &current, &path)?.new_root_hash
                }
                WorkspaceApplyOp::SetAnnotations {
                    path,
                    annotations_patch,
                } => {
                    local_workspace::annotations_set(
                        self.cas.as_ref(),
                        &current,
                        path.as_deref(),
                        &annotations_patch,
                    )?
                    .new_root_hash
                }
            };
        }
        Ok(WorkspaceApplyResponse {
            base_root_hash,
            new_root_hash: current,
        })
    }

    pub fn workspace_diff(
        &self,
        root_a: &HashRef,
        root_b: &HashRef,
        prefix: Option<&str>,
    ) -> Result<aos_effect_types::WorkspaceDiffReceipt, LocalRuntimeError> {
        Ok(local_workspace::diff(
            self.cas.as_ref(),
            root_a,
            root_b,
            prefix,
        )?)
    }

    pub fn journal_head(&self, world_id: WorldId) -> Result<HeadInfoResponse, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        let bounds = world.kernel.journal_bounds();
        Ok(HeadInfoResponse {
            journal_head: world.kernel.heights().head,
            retained_from: bounds.retained_from,
            manifest_hash: Some(world.kernel.manifest_hash().to_hex()),
        })
    }

    pub fn journal_entries(
        &self,
        world_id: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<JournalEntriesResponse, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        let bounds = world.kernel.journal_bounds();
        let from = from.max(bounds.retained_from);
        let entries = world.kernel.dump_journal_from(from)?;
        let mut rows = Vec::new();
        let mut next_from = from;
        for entry in entries.into_iter().take(limit as usize) {
            let record_value = serde_cbor::from_slice::<serde_cbor::Value>(&entry.payload)
                .ok()
                .and_then(|value| serde_json::to_value(value).ok())
                .unwrap_or_else(
                    || serde_json::json!({ "payload_b64": BASE64_STANDARD.encode(&entry.payload) }),
                );
            rows.push(JournalEntryResponse {
                seq: entry.seq,
                kind: format!("{:?}", entry.kind).to_lowercase(),
                record: record_value,
            });
            next_from = entry.seq.saturating_add(1);
        }
        Ok(JournalEntriesResponse {
            from,
            retained_from: bounds.retained_from,
            next_from,
            entries: rows,
        })
    }

    pub fn journal_entries_raw(
        &self,
        world_id: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<RawJournalEntriesResponse, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        let bounds = world.kernel.journal_bounds();
        let from = from.max(bounds.retained_from);
        let entries = world.kernel.dump_journal_from(from)?;
        let mut rows = Vec::new();
        let mut next_from = from;
        for entry in entries.into_iter().take(limit as usize) {
            rows.push(RawJournalEntryResponse {
                seq: entry.seq,
                entry_cbor: to_canonical_cbor(&entry)?,
            });
            next_from = entry.seq.saturating_add(1);
        }
        Ok(RawJournalEntriesResponse {
            from,
            retained_from: bounds.retained_from,
            next_from,
            entries: rows,
        })
    }

    fn world_summary_locked(
        &self,
        inner: &RuntimeState,
        world_id: WorldId,
    ) -> Result<(WorldRuntimeInfo, crate::SnapshotRecord), LocalRuntimeError> {
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        Ok((
            runtime_info_from_world(world),
            world.active_baseline.clone(),
        ))
    }

    fn world_record_locked(
        &self,
        inner: &RuntimeState,
        world_id: WorldId,
    ) -> Result<WorldRecord, LocalRuntimeError> {
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        Ok(WorldRecord {
            world_id,
            universe_id: world.universe_id,
            created_at_ns: world.created_at_ns,
            manifest_hash: world.kernel.manifest_hash().to_hex(),
            active_baseline: world.active_baseline.clone(),
            journal_head: world.kernel.heights().head,
        })
    }

    fn allocate_submission_seq_locked(
        &self,
        inner: &mut RuntimeState,
    ) -> Result<u64, LocalRuntimeError> {
        let submission_seq = inner.next_submission_seq;
        inner.next_submission_seq = inner.next_submission_seq.saturating_add(1);
        inner
            .sqlite
            .persist_runtime_counters(inner.next_submission_seq, inner.next_frame_offset)?;
        Ok(submission_seq)
    }

    fn build_event_submission_locked(
        &self,
        inner: &RuntimeState,
        world_id: WorldId,
        ingress: DomainEventIngress,
    ) -> Result<SubmissionEnvelope, LocalRuntimeError> {
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        Ok(SubmissionEnvelope {
            submission_id: ingress
                .correlation_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            universe_id: local_submission_universe_id(),
            world_id,
            world_epoch: world.world_epoch,
            command: None,
            payload: SubmissionPayload::WorldInput {
                input: WorldInput::DomainEvent(aos_wasm_abi::DomainEvent {
                    schema: ingress.schema,
                    value: resolve_plane_cbor_payload(self.cas.as_ref(), &ingress.value)?,
                    key: ingress.key,
                }),
            },
        })
    }

    fn build_receipt_submission_locked(
        &self,
        inner: &RuntimeState,
        world_id: WorldId,
        ingress: crate::ReceiptIngress,
    ) -> Result<SubmissionEnvelope, LocalRuntimeError> {
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        Ok(SubmissionEnvelope {
            submission_id: ingress
                .correlation_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            universe_id: local_submission_universe_id(),
            world_id,
            world_epoch: world.world_epoch,
            command: None,
            payload: SubmissionPayload::WorldInput {
                input: WorldInput::Receipt(aos_effects::EffectReceipt {
                    intent_hash: parse_plane_intent_hash(&ingress.intent_hash)?,
                    adapter_id: ingress.adapter_id,
                    status: ingress.status,
                    payload_cbor: resolve_plane_cbor_payload(self.cas.as_ref(), &ingress.payload)?,
                    cost_cents: ingress.cost_cents,
                    signature: ingress.signature,
                }),
            },
        })
    }

    fn build_world_input_submission_locked(
        &self,
        inner: &RuntimeState,
        world_id: WorldId,
        input: WorldInput,
    ) -> Result<SubmissionEnvelope, LocalRuntimeError> {
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        Ok(SubmissionEnvelope {
            submission_id: Uuid::new_v4().to_string(),
            universe_id: local_submission_universe_id(),
            world_id,
            world_epoch: world.world_epoch,
            command: None,
            payload: SubmissionPayload::WorldInput { input },
        })
    }

    fn world_control_from_command_payload(
        &self,
        command: &str,
        payload: &CborPayload,
    ) -> Result<WorldControl, LocalRuntimeError> {
        let payload = resolve_plane_cbor_payload(self.cas.as_ref(), payload)?;

        fn prepare_manifest_patch<S: Store + 'static>(
            store: &S,
            input: aos_effect_types::GovPatchInput,
            manifest_base: Option<HashRef>,
        ) -> Result<ManifestPatch, LocalRuntimeError> {
            match input {
                aos_effect_types::GovPatchInput::Hash(hash) => {
                    if manifest_base.is_some() {
                        return Err(LocalRuntimeError::Kernel(KernelError::Manifest(
                            "manifest_base is not supported with patch hash input".into(),
                        )));
                    }
                    let bytes = store.get_blob(parse_plane_hash_ref(hash.as_str())?)?;
                    Ok(serde_cbor::from_slice(&bytes)?)
                }
                aos_effect_types::GovPatchInput::PatchCbor(bytes) => {
                    if manifest_base.is_some() {
                        return Err(LocalRuntimeError::Kernel(KernelError::Manifest(
                            "manifest_base is not supported with patch_cbor input".into(),
                        )));
                    }
                    let patch: ManifestPatch = serde_cbor::from_slice(&bytes)?;
                    canonicalize_patch(store, patch).map_err(LocalRuntimeError::Kernel)
                }
                aos_effect_types::GovPatchInput::PatchDocJson(bytes) => {
                    let doc: PatchDocument = serde_json::from_slice(&bytes)?;
                    if let Some(expected) = manifest_base.as_ref()
                        && expected.as_str() != doc.base_manifest_hash
                    {
                        return Err(LocalRuntimeError::Kernel(KernelError::Manifest(format!(
                            "manifest_base mismatch: expected {expected}, got {}",
                            doc.base_manifest_hash
                        ))));
                    }
                    compile_patch_document(store, doc).map_err(LocalRuntimeError::Kernel)
                }
                aos_effect_types::GovPatchInput::PatchBlobRef { blob_ref, format } => {
                    let bytes = store.get_blob(parse_plane_hash_ref(blob_ref.as_str())?)?;
                    match format.as_str() {
                        "manifest_patch_cbor" => prepare_manifest_patch(
                            store,
                            aos_effect_types::GovPatchInput::PatchCbor(bytes),
                            manifest_base,
                        ),
                        "patch_doc_json" => prepare_manifest_patch(
                            store,
                            aos_effect_types::GovPatchInput::PatchDocJson(bytes),
                            manifest_base,
                        ),
                        other => Err(LocalRuntimeError::Kernel(KernelError::Manifest(format!(
                            "unknown patch blob format '{other}'"
                        )))),
                    }
                }
            }
        }

        match command {
            "gov-propose" => {
                let params: aos_effect_types::GovProposeParams = serde_cbor::from_slice(&payload)?;
                let patch =
                    prepare_manifest_patch(self.cas.as_ref(), params.patch, params.manifest_base)?;
                Ok(WorldControl::SubmitProposal {
                    patch,
                    description: params.description,
                })
            }
            "gov-shadow" => {
                let params: aos_effect_types::GovShadowParams = serde_cbor::from_slice(&payload)?;
                Ok(WorldControl::RunShadow {
                    proposal_id: params.proposal_id,
                })
            }
            "gov-approve" => {
                let params: aos_effect_types::GovApproveParams = serde_cbor::from_slice(&payload)?;
                let decision = match params.decision {
                    aos_effect_types::GovDecision::Approve => ApprovalDecisionRecord::Approve,
                    aos_effect_types::GovDecision::Reject => ApprovalDecisionRecord::Reject,
                };
                Ok(WorldControl::DecideProposal {
                    proposal_id: params.proposal_id,
                    approver: params.approver,
                    decision,
                })
            }
            "gov-apply" => {
                let params: aos_effect_types::GovApplyParams = serde_cbor::from_slice(&payload)?;
                Ok(WorldControl::ApplyProposal {
                    proposal_id: params.proposal_id,
                })
            }
            other => Err(LocalRuntimeError::Backend(format!(
                "unsupported world control command '{other}'"
            ))),
        }
    }

    fn enqueue_world_submission_locked(
        &self,
        inner: &mut RuntimeState,
        submission: SubmissionEnvelope,
    ) -> Result<(), LocalRuntimeError> {
        let world_id = submission.world_id;
        let world = inner
            .worlds
            .get_mut(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        world.mailbox.push_back(submission);
        if !world.ready {
            world.ready = true;
            inner.ready_worlds.push_back(world_id);
        }
        Ok(())
    }

    fn mark_world_ready_locked(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
    ) -> Result<(), LocalRuntimeError> {
        let world = inner
            .worlds
            .get_mut(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        if !world.ready {
            world.ready = true;
            inner.ready_worlds.push_back(world_id);
        }
        Ok(())
    }

    fn drain_async_events_locked(&self, inner: &mut RuntimeState) -> Result<(), LocalRuntimeError> {
        let mut effect_events = Vec::new();
        {
            let mut rx = self
                .effect_event_rx
                .lock()
                .expect("effect continuation receiver mutex poisoned");
            if let Some(rx) = rx.as_mut() {
                while let Ok(event) = rx.try_recv() {
                    effect_events.push(event);
                }
            }
        }
        for event in effect_events {
            self.enqueue_effect_runtime_event_locked(inner, event)?;
        }

        let mut timer_wakes = Vec::new();
        {
            let mut rx = self
                .timer_wake_rx
                .lock()
                .expect("timer wake receiver mutex poisoned");
            if let Some(rx) = rx.as_mut() {
                while let Ok(wake) = rx.try_recv() {
                    timer_wakes.push(wake);
                }
            }
        }
        for wake in timer_wakes {
            self.handle_timer_wake_locked(inner, wake.world_id)?;
        }
        Ok(())
    }

    fn enqueue_effect_runtime_event_locked(
        &self,
        inner: &mut RuntimeState,
        event: EffectRuntimeEvent<WorldId>,
    ) -> Result<(), LocalRuntimeError> {
        let EffectRuntimeEvent::WorldInput { world_id, input } = event;
        let submission = self.build_world_input_submission_locked(inner, world_id, input)?;
        self.enqueue_world_submission_locked(inner, submission)?;
        Ok(())
    }

    fn handle_timer_wake_locked(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
    ) -> Result<(), LocalRuntimeError> {
        let due = {
            let world = inner
                .worlds
                .get_mut(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
            let due = world.timer_scheduler.pop_due(now_wallclock_ns());
            for entry in &due {
                world.scheduled_timers.remove(&entry.intent_hash);
            }
            due
        };
        for entry in due {
            let receipt = build_timer_receipt(&entry)?;
            let submission = self.build_world_input_submission_locked(
                inner,
                world_id,
                WorldInput::Receipt(receipt),
            )?;
            self.enqueue_world_submission_locked(inner, submission)?;
        }
        Ok(())
    }

    fn process_submission_locked(
        &self,
        inner: &mut RuntimeState,
        submission: SubmissionEnvelope,
    ) -> Result<bool, LocalRuntimeError> {
        let mut command_updates = Vec::new();
        if let Some(command) = &submission.command {
            let world = inner.worlds.get_mut(&submission.world_id).ok_or_else(|| {
                LocalRuntimeError::Backend(format!("world {} not found", submission.world_id))
            })?;
            if submission.world_epoch != world.world_epoch {
                return Err(LocalRuntimeError::Backend(format!(
                    "world epoch mismatch: expected {}, got {}",
                    world.world_epoch, submission.world_epoch
                )));
            }
            if let Some(record) = world.command_records.get_mut(&command.command_id) {
                record.status = CommandStatus::Running;
                record.started_at_ns = Some(record.started_at_ns.unwrap_or_else(now_wallclock_ns));
                record.finished_at_ns = None;
                record.error = None;
                command_updates.push(record.clone());
            }
        }

        for record in &command_updates {
            inner
                .sqlite
                .persist_command_projection(submission.world_id, record)?;
        }

        let service_result = self.service_submission_for_world_locked(inner, &submission);
        match service_result {
            Ok(appended) => {
                if let Some(command) = &submission.command
                    && let Some(world) = inner.worlds.get_mut(&submission.world_id)
                    && let Some(record) = world.command_records.get_mut(&command.command_id)
                {
                    record.status = CommandStatus::Succeeded;
                    record.finished_at_ns = Some(now_wallclock_ns());
                    record.journal_height = Some(world.kernel.heights().head);
                    record.manifest_hash = Some(world.kernel.manifest_hash().to_hex());
                    inner
                        .sqlite
                        .persist_command_projection(submission.world_id, record)?;
                }
                Ok(appended)
            }
            Err(err) => {
                if let Some(command) = &submission.command
                    && let Some(world) = inner.worlds.get_mut(&submission.world_id)
                    && let Some(record) = world.command_records.get_mut(&command.command_id)
                {
                    record.status = CommandStatus::Failed;
                    record.finished_at_ns = Some(now_wallclock_ns());
                    record.journal_height = Some(world.kernel.heights().head);
                    record.manifest_hash = Some(world.kernel.manifest_hash().to_hex());
                    record.error = Some(CommandErrorBody {
                        code: "command_failed".into(),
                        message: err.to_string(),
                    });
                    inner
                        .sqlite
                        .persist_command_projection(submission.world_id, record)?;
                }
                Err(err)
            }
        }
    }

    fn service_submission_for_world_locked(
        &self,
        inner: &mut RuntimeState,
        submission: &SubmissionEnvelope,
    ) -> Result<bool, LocalRuntimeError> {
        if let SubmissionPayload::HostControl { control } = &submission.payload {
            match control {
                HostControl::CreateWorld { request } => {
                    self.create_world_from_request_locked(
                        inner,
                        submission.world_id,
                        request.clone(),
                    )?;
                    return Ok(true);
                }
            }
        }
        {
            let world = inner.worlds.get(&submission.world_id).ok_or_else(|| {
                LocalRuntimeError::Backend(format!("world {} not found", submission.world_id))
            })?;
            if submission.world_epoch != world.world_epoch {
                return Err(LocalRuntimeError::Backend(format!(
                    "world epoch mismatch: expected {}, got {}",
                    world.world_epoch, submission.world_epoch
                )));
            }
        }

        let mut appended_any_frame = false;
        let mut pending_inputs = VecDeque::new();

        match &submission.payload {
            SubmissionPayload::HostControl { .. } => {
                unreachable!("host control bypasses world admission")
            }
            SubmissionPayload::WorldControl { control } => {
                let (appended, followups) =
                    self.apply_control_step_locked(inner, submission.world_id, control.clone())?;
                appended_any_frame |= appended;
                pending_inputs.extend(followups);
            }
            SubmissionPayload::WorldInput { input } => {
                pending_inputs.push_back(input.clone());
            }
        }

        while let Some(input) = pending_inputs.pop_front() {
            let (appended, followups) =
                self.apply_input_step_locked(inner, submission.world_id, input)?;
            appended_any_frame |= appended;
            pending_inputs.extend(followups);
        }

        Ok(appended_any_frame)
    }

    fn apply_input_step_locked(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
        input: WorldInput,
    ) -> Result<(bool, Vec<WorldInput>), LocalRuntimeError> {
        let tail_start = {
            let world = inner
                .worlds
                .get(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
            world.kernel.journal_head()
        };
        let drain = {
            let world = inner
                .worlds
                .get_mut(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
            world.kernel.accept(input)?;
            world.kernel.drain_until_idle_from(tail_start)?
        };
        self.commit_drain_and_collect_followups(inner, world_id, drain)
    }

    fn apply_control_step_locked(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
        control: WorldControl,
    ) -> Result<(bool, Vec<WorldInput>), LocalRuntimeError> {
        let tail_start = {
            let world = inner
                .worlds
                .get(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
            world.kernel.journal_head()
        };
        {
            let world = inner
                .worlds
                .get_mut(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
            let _ = world.kernel.apply_control(control)?;
        }
        let drain = {
            let world = inner
                .worlds
                .get_mut(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
            world.kernel.drain_until_idle_from(tail_start)?
        };
        self.commit_drain_and_collect_followups(inner, world_id, drain)
    }

    fn commit_drain_and_collect_followups(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
        drain: KernelDrain,
    ) -> Result<(bool, Vec<WorldInput>), LocalRuntimeError> {
        let appended =
            if let Some(frame) = self.build_frame_from_drain_locked(inner, world_id, &drain)? {
                self.append_frame_locked(inner, world_id, frame)?;
                true
            } else {
                false
            };

        let mut followups = Vec::new();
        let mut timers = Vec::new();
        let mut external = Vec::new();
        {
            let world = inner
                .worlds
                .get_mut(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
            for opened in &drain.opened_effects {
                match world.effect_runtime.classify_intent(&opened.intent) {
                    EffectExecutionClass::InlineInternal => {
                        if let Some(receipt) =
                            world.kernel.handle_internal_intent(&opened.intent)?
                        {
                            followups.push(WorldInput::Receipt(receipt));
                        }
                    }
                    EffectExecutionClass::OwnerLocalTimer => timers.push(opened.intent.clone()),
                    EffectExecutionClass::ExternalAsync => external.push(opened.intent.clone()),
                }
            }
        }
        for intent in timers {
            self.schedule_timer_intent_locked(inner, world_id, &intent)?;
        }
        for intent in external {
            self.start_external_effect_locked(inner, world_id, intent)?;
        }

        Ok((appended, followups))
    }

    fn create_world_from_request_locked(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
        request: CreateWorldRequest,
    ) -> Result<(), LocalRuntimeError> {
        let create_started = std::time::Instant::now();
        let source_kind = create_world_source_kind(&request);
        let request = localize_create_request(request);
        if inner.worlds.contains_key(&world_id) {
            return Err(LocalRuntimeError::Backend(format!(
                "world {world_id} already exists"
            )));
        }

        let world = self.build_world_slot(world_id, &request)?;
        inner.sqlite.persist_world_directory(
            world_id,
            world.universe_id,
            world.created_at_ns,
            &world.initial_manifest_hash,
            world.world_epoch,
        )?;
        let initial_frame = self.initial_frame_for_world(&world)?;
        let initial_record_count = initial_frame
            .as_ref()
            .map(|frame| frame.records.len())
            .unwrap_or_default();
        let baseline = world.active_baseline.clone();
        inner.worlds.insert(world_id, world);
        if let Some(frame) = initial_frame {
            self.append_frame_locked(inner, world_id, frame)?;
        } else {
            let checkpointed_at_ns = inner
                .worlds
                .get(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?
                .last_checkpointed_at_ns;
            inner
                .sqlite
                .persist_checkpoint_head(world_id, &baseline, 0, checkpointed_at_ns)?;
        }
        self.compact_world_journal_to_active_baseline_locked(inner, world_id)?;
        self.rehydrate_open_work_locked(inner, world_id)?;
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        let total_create_ms = create_started.elapsed().as_millis();
        self.log_world_created(world, source_kind, total_create_ms, initial_record_count);
        self.log_world_opened(
            world,
            "create",
            total_create_ms,
            usize::from(initial_record_count > 0),
            0,
        );
        Ok(())
    }

    fn build_world_slot(
        &self,
        world_id: WorldId,
        request: &CreateWorldRequest,
    ) -> Result<WorldSlot, LocalRuntimeError> {
        let request = localize_create_request(request.clone());
        let kernel_config = self.kernel_config_for_create_request(&request)?;
        let last_checkpointed_at_ns = now_wallclock_ns();
        match &request.source {
            CreateWorldSource::Manifest { manifest_hash } => {
                let initial_manifest_hash = parse_plane_hash_like(manifest_hash, "manifest_hash")?;
                let loaded =
                    ManifestLoader::load_from_hash(self.cas.as_ref(), initial_manifest_hash)?;
                let effect_runtime = EffectRuntime::from_loaded_manifest(
                    self.cas.clone(),
                    &self.adapter_config,
                    &loaded,
                    self.world_config.strict_effect_bindings,
                    self.effect_event_tx.clone(),
                )?;
                let kernel = Kernel::from_loaded_manifest_with_config(
                    self.cas.clone(),
                    loaded,
                    Journal::new(),
                    kernel_config,
                )?;
                let active_baseline = latest_snapshot_from_kernel(&kernel).ok_or_else(|| {
                    LocalRuntimeError::Backend(
                        "create-world snapshot produced no snapshot record".into(),
                    )
                })?;
                Ok(WorldSlot {
                    world_id,
                    universe_id: request.universe_id,
                    created_at_ns: request.created_at_ns,
                    initial_manifest_hash: initial_manifest_hash.to_hex(),
                    world_epoch: 1,
                    active_baseline,
                    next_world_seq: 0,
                    last_checkpointed_at_ns,
                    kernel,
                    effect_runtime,
                    timer_scheduler: TimerScheduler::new(),
                    scheduled_timers: HashSet::new(),
                    mailbox: VecDeque::new(),
                    ready: false,
                    command_records: BTreeMap::new(),
                })
            }
            CreateWorldSource::Seed { seed } => {
                let manifest_hash = seed.baseline.manifest_hash.as_deref().ok_or_else(|| {
                    PersistError::validation("seed baseline requires manifest_hash")
                })?;
                let initial_manifest_hash =
                    parse_plane_hash_like(manifest_hash, "seed.baseline.manifest_hash")?;
                let loaded =
                    ManifestLoader::load_from_hash(self.cas.as_ref(), initial_manifest_hash)?;
                let effect_runtime = EffectRuntime::from_loaded_manifest(
                    self.cas.clone(),
                    &self.adapter_config,
                    &loaded,
                    self.world_config.strict_effect_bindings,
                    self.effect_event_tx.clone(),
                )?;
                let mut kernel = Kernel::from_loaded_manifest_without_replay_with_config(
                    self.cas.clone(),
                    loaded,
                    Journal::new(),
                    kernel_config,
                )?;
                let snapshot = kernel_snapshot_record(&seed.baseline);
                kernel.restore_snapshot_record(&snapshot)?;
                Ok(WorldSlot {
                    world_id,
                    universe_id: request.universe_id,
                    created_at_ns: request.created_at_ns,
                    initial_manifest_hash: initial_manifest_hash.to_hex(),
                    world_epoch: 1,
                    active_baseline: seed.baseline.clone(),
                    next_world_seq: 0,
                    last_checkpointed_at_ns,
                    kernel,
                    effect_runtime,
                    timer_scheduler: TimerScheduler::new(),
                    scheduled_timers: HashSet::new(),
                    mailbox: VecDeque::new(),
                    ready: false,
                    command_records: BTreeMap::new(),
                })
            }
        }
    }

    fn initial_frame_for_world(
        &self,
        world: &WorldSlot,
    ) -> Result<Option<WorldLogFrame>, LocalRuntimeError> {
        let tail = world.kernel.dump_journal_from(0)?;
        if tail.is_empty() {
            return Ok(None);
        }
        let records = tail
            .into_iter()
            .map(|entry| serde_cbor::from_slice::<JournalRecord>(&entry.payload))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(WorldLogFrame {
            format_version: 1,
            universe_id: local_submission_universe_id(),
            world_id: world.world_id,
            world_epoch: world.world_epoch,
            world_seq_start: 0,
            world_seq_end: records.len() as u64 - 1,
            records,
        }))
    }

    fn create_fork_seed_request_locked(
        &self,
        inner: &RuntimeState,
        new_world_id: WorldId,
        request: &ForkWorldRequest,
    ) -> Result<CreateWorldRequest, LocalRuntimeError> {
        let selected =
            self.select_source_snapshot_locked(inner, request.src_world_id, &request.src_snapshot)?;
        let selected_hash = parse_plane_hash_like(&selected.snapshot_ref, "src_snapshot_ref")?;
        let selected_bytes = self.cas.get(selected_hash)?;
        let rewritten =
            rewrite_snapshot_for_fork_policy(&selected_bytes, &request.pending_effect_policy)?;
        let snapshot_ref = if let Some(bytes) = rewritten {
            self.cas.put_blob(&bytes)?.to_hex()
        } else {
            selected.snapshot_ref.clone()
        };
        Ok(CreateWorldRequest {
            world_id: Some(new_world_id),
            universe_id: crate::UniverseId::nil(),
            created_at_ns: request.forked_at_ns,
            source: CreateWorldSource::Seed {
                seed: crate::WorldSeed {
                    baseline: crate::SnapshotRecord {
                        snapshot_ref,
                        height: selected.height,
                        universe_id: crate::UniverseId::nil(),
                        logical_time_ns: selected.logical_time_ns,
                        receipt_horizon_height: selected.receipt_horizon_height,
                        manifest_hash: selected.manifest_hash.clone(),
                    },
                    seed_kind: SeedKind::Import,
                    imported_from: Some(crate::ImportedSeedSource {
                        source: "fork".into(),
                        external_world_id: Some(request.src_world_id.to_string()),
                        external_snapshot_ref: Some(selected.snapshot_ref),
                    }),
                },
            },
        })
    }

    fn select_source_snapshot_locked(
        &self,
        inner: &RuntimeState,
        src_world_id: WorldId,
        selector: &SnapshotSelector,
    ) -> Result<crate::SnapshotRecord, LocalRuntimeError> {
        let world = inner
            .worlds
            .get(&src_world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {src_world_id} not found")))?;
        match selector {
            SnapshotSelector::ActiveBaseline => Ok(world.active_baseline.clone()),
            SnapshotSelector::ByHeight { height } => {
                if world.active_baseline.height == *height {
                    return Ok(world.active_baseline.clone());
                }
                let frames = inner.sqlite.load_frame_log_for_world(src_world_id)?;
                snapshot_record_from_frames(&frames, |snapshot| snapshot.height == *height)
                    .ok_or_else(|| {
                        LocalRuntimeError::Backend(format!(
                            "snapshot at height {height} not found for world {src_world_id}"
                        ))
                    })
            }
            SnapshotSelector::ByRef { snapshot_ref } => {
                if world.active_baseline.snapshot_ref == *snapshot_ref {
                    return Ok(world.active_baseline.clone());
                }
                let frames = inner.sqlite.load_frame_log_for_world(src_world_id)?;
                snapshot_record_from_frames(&frames, |snapshot| {
                    snapshot.snapshot_ref == *snapshot_ref
                })
                .ok_or_else(|| {
                    LocalRuntimeError::Backend(format!(
                        "snapshot ref {snapshot_ref} not found for world {src_world_id}"
                    ))
                })
            }
        }
    }

    fn append_frame_locked(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
        frame: WorldLogFrame,
    ) -> Result<(), LocalRuntimeError> {
        let offset = inner.next_frame_offset;
        inner.next_frame_offset = inner.next_frame_offset.saturating_add(1);
        inner
            .sqlite
            .append_journal_frame(offset, world_id, &frame)?;
        inner
            .sqlite
            .persist_runtime_counters(inner.next_submission_seq, inner.next_frame_offset)?;
        let world = inner
            .worlds
            .get_mut(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        world.next_world_seq = frame.world_seq_end.saturating_add(1);
        if let Some(snapshot) = frame.records.iter().rev().find_map(|record| match record {
            JournalRecord::Snapshot(snapshot) => Some(crate::SnapshotRecord {
                snapshot_ref: snapshot.snapshot_ref.clone(),
                height: snapshot.height,
                universe_id: snapshot.universe_id.into(),
                logical_time_ns: snapshot.logical_time_ns,
                receipt_horizon_height: snapshot.receipt_horizon_height,
                manifest_hash: snapshot.manifest_hash.clone(),
            }),
            _ => None,
        }) {
            world.active_baseline = snapshot;
        }
        inner.sqlite.persist_checkpoint_head(
            world_id,
            &world.active_baseline,
            world.next_world_seq,
            world.last_checkpointed_at_ns,
        )?;
        Ok(())
    }

    fn checkpoint_world_locked(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
        trigger: &'static str,
    ) -> Result<(), LocalRuntimeError> {
        let checkpoint_started = std::time::Instant::now();
        let checkpointed_at_ns = now_wallclock_ns();
        let tail_start = {
            let world = inner
                .worlds
                .get(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
            world.kernel.journal_head()
        };
        {
            let world = inner
                .worlds
                .get_mut(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
            world.kernel.create_snapshot()?;
        }
        let Some(frame) = self.build_frame_from_tail_locked(inner, world_id, tail_start)? else {
            return Err(LocalRuntimeError::Backend(format!(
                "checkpoint for world {world_id} produced no retained journal tail"
            )));
        };
        let retained_record_count = frame.records.len();
        let checkpoint_world_seq_end = frame.world_seq_end;
        inner
            .worlds
            .get_mut(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?
            .last_checkpointed_at_ns = checkpointed_at_ns;
        self.append_frame_locked(inner, world_id, frame)?;
        self.compact_world_journal_to_active_baseline_locked(inner, world_id)?;
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        self.log_world_checkpointed(
            world,
            trigger,
            checkpoint_started.elapsed().as_millis(),
            retained_record_count,
            checkpoint_world_seq_end,
        );
        Ok(())
    }

    fn build_frame_from_drain_locked(
        &self,
        inner: &RuntimeState,
        world_id: WorldId,
        drain: &KernelDrain,
    ) -> Result<Option<WorldLogFrame>, LocalRuntimeError> {
        if drain.tail.entries.is_empty() {
            return Ok(None);
        }
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        Ok(Some(WorldLogFrame {
            format_version: 1,
            universe_id: local_submission_universe_id(),
            world_id,
            world_epoch: world.world_epoch,
            world_seq_start: world.next_world_seq,
            world_seq_end: world.next_world_seq + drain.tail.entries.len() as u64 - 1,
            records: drain
                .tail
                .entries
                .iter()
                .map(|entry| entry.record.clone())
                .collect(),
        }))
    }

    fn build_frame_from_tail_locked(
        &self,
        inner: &RuntimeState,
        world_id: WorldId,
        tail_start: u64,
    ) -> Result<Option<WorldLogFrame>, LocalRuntimeError> {
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        let tail = world.kernel.dump_journal_from(tail_start)?;
        if tail.is_empty() {
            return Ok(None);
        }
        let records = tail
            .iter()
            .map(|entry| serde_cbor::from_slice::<JournalRecord>(&entry.payload))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(WorldLogFrame {
            format_version: 1,
            universe_id: local_submission_universe_id(),
            world_id,
            world_epoch: world.world_epoch,
            world_seq_start: world.next_world_seq,
            world_seq_end: world.next_world_seq + records.len() as u64 - 1,
            records,
        }))
    }

    fn compact_world_journal_to_active_baseline_locked(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
    ) -> Result<(), LocalRuntimeError> {
        let world = inner
            .worlds
            .get_mut(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        world
            .kernel
            .compact_journal_through(world.active_baseline.height)?;
        Ok(())
    }
    fn load_hot_worlds_locked(&self, inner: &mut RuntimeState) -> Result<(), LocalRuntimeError> {
        for (directory, checkpoint) in inner.sqlite.load_world_directory()? {
            debug_assert_eq!(directory.world_id, checkpoint.world_id);
            let open_started = std::time::Instant::now();
            let loaded = ManifestLoader::load_from_hash(
                self.cas.as_ref(),
                parse_plane_hash_like(&directory.initial_manifest_hash, "manifest_hash")?,
            )?;
            let frames = inner.sqlite.load_frame_log_for_world(directory.world_id)?;
            let frame_count = frames.len();
            let replay_frame_count = frames
                .iter()
                .filter(|frame| frame.world_seq_end > checkpoint.active_baseline.height)
                .count();
            let effect_runtime = EffectRuntime::from_loaded_manifest(
                self.cas.clone(),
                &self.adapter_config,
                &loaded,
                self.world_config.strict_effect_bindings,
                self.effect_event_tx.clone(),
            )?;
            let kernel = reopen_kernel_from_frame_log(
                &self.paths,
                self.cas.clone(),
                loaded,
                &checkpoint.active_baseline,
                &frames,
                self.kernel_config.clone(),
            )?;
            let command_records = inner.sqlite.load_command_projection(directory.world_id)?;
            inner.worlds.insert(
                directory.world_id,
                WorldSlot {
                    world_id: directory.world_id,
                    universe_id: directory.universe_id,
                    created_at_ns: directory.created_at_ns,
                    initial_manifest_hash: directory.initial_manifest_hash,
                    world_epoch: directory.world_epoch,
                    active_baseline: checkpoint.active_baseline,
                    next_world_seq: checkpoint.next_world_seq,
                    last_checkpointed_at_ns: checkpoint.checkpointed_at_ns,
                    kernel,
                    effect_runtime,
                    timer_scheduler: TimerScheduler::new(),
                    scheduled_timers: HashSet::new(),
                    mailbox: VecDeque::new(),
                    ready: false,
                    command_records,
                },
            );
            self.rehydrate_open_work_locked(inner, directory.world_id)?;
            let world = inner.worlds.get(&directory.world_id).ok_or_else(|| {
                LocalRuntimeError::Backend(format!("world {} not found", directory.world_id))
            })?;
            self.log_world_opened(
                world,
                "startup",
                open_started.elapsed().as_millis(),
                frame_count,
                replay_frame_count,
            );
        }
        Ok(())
    }

    fn rehydrate_open_work_locked(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
    ) -> Result<(), LocalRuntimeError> {
        let pending = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?
            .kernel
            .pending_workflow_receipts_snapshot();
        for pending in pending {
            let intent = EffectIntent {
                kind: pending.effect_kind.clone().into(),
                cap_name: pending.cap_name.clone(),
                params_cbor: pending.params_cbor.clone(),
                idempotency_key: pending.idempotency_key,
                intent_hash: pending.intent_hash,
            };
            match inner
                .worlds
                .get(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?
                .effect_runtime
                .classify_intent(&intent)
            {
                EffectExecutionClass::InlineInternal => {}
                EffectExecutionClass::OwnerLocalTimer => {
                    self.schedule_timer_intent_locked(inner, world_id, &intent)?;
                }
                EffectExecutionClass::ExternalAsync => {
                    self.start_external_effect_locked(inner, world_id, intent)?;
                }
            }
        }
        Ok(())
    }

    fn start_external_effect_locked(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
        intent: EffectIntent,
    ) -> Result<(), LocalRuntimeError> {
        let effect_runtime = &inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?
            .effect_runtime;
        let _guard = self.edge_runtime.handle().enter();
        let _ = effect_runtime.ensure_started(world_id, intent)?;
        Ok(())
    }

    fn schedule_timer_intent_locked(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
        intent: &EffectIntent,
    ) -> Result<(), LocalRuntimeError> {
        let params = intent
            .params::<aos_effect_types::TimerSetParams>()
            .map_err(LocalRuntimeError::Cbor)?;
        {
            let world = inner
                .worlds
                .get_mut(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
            if !world.scheduled_timers.insert(intent.intent_hash) {
                return Ok(());
            }
            world.timer_scheduler.schedule(intent)?;
        }
        self.spawn_timer_wake(world_id, params.deliver_at_ns);
        Ok(())
    }

    fn spawn_timer_wake(&self, world_id: WorldId, deliver_at_ns: u64) {
        let wake_at = if deliver_at_ns <= now_wallclock_ns() {
            Instant::now()
        } else {
            Instant::now() + Duration::from_nanos(deliver_at_ns - now_wallclock_ns())
        };
        let tx = self.timer_wake_tx.clone();
        self.edge_runtime.handle().spawn(async move {
            sleep_until(wake_at).await;
            let _ = tx.send(TimerWake { world_id }).await;
        });
    }

    pub(crate) fn take_effect_event_rx(
        &self,
    ) -> Option<mpsc::Receiver<EffectRuntimeEvent<WorldId>>> {
        self.effect_event_rx
            .lock()
            .expect("effect continuation receiver mutex poisoned")
            .take()
    }

    pub(crate) fn take_timer_wake_rx(&self) -> Option<mpsc::Receiver<TimerWake>> {
        self.timer_wake_rx
            .lock()
            .expect("timer wake receiver mutex poisoned")
            .take()
    }

    pub(crate) fn enqueue_effect_runtime_event(
        &self,
        event: EffectRuntimeEvent<WorldId>,
    ) -> Result<(), LocalRuntimeError> {
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        self.enqueue_effect_runtime_event_locked(&mut inner, event)
    }

    pub(crate) fn process_timer_wake(&self, world_id: WorldId) -> Result<(), LocalRuntimeError> {
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        self.handle_timer_wake_locked(&mut inner, world_id)
    }
}

fn reopen_kernel_from_frame_log(
    paths: &LocalStatePaths,
    store: Arc<FsCas>,
    loaded: LoadedManifest,
    active_baseline: &crate::SnapshotRecord,
    frames: &[WorldLogFrame],
    mut kernel_config: KernelConfig,
) -> Result<Kernel<FsCas>, LocalRuntimeError> {
    if kernel_config.secret_resolver.is_none() {
        if let Some(resolver) = local_secret_resolver_for_manifest(paths, &loaded)? {
            kernel_config.secret_resolver = Some(resolver);
        }
    }
    if frames.is_empty() {
        let mut kernel = Kernel::from_loaded_manifest_without_replay_with_config(
            store,
            loaded,
            Journal::with_retained_from(active_baseline.height.saturating_add(1)),
            kernel_config,
        )?;
        kernel.restore_snapshot_record(&kernel_snapshot_record(active_baseline))?;
        kernel.compact_journal_through(active_baseline.height)?;
        return Ok(kernel);
    }

    let replay_entries =
        journal_entries_from_world_frames_after_height(frames, active_baseline.height)?;
    if replay_entries.is_empty() {
        let mut kernel = Kernel::from_loaded_manifest_without_replay_with_config(
            store,
            loaded,
            Journal::with_retained_from(active_baseline.height.saturating_add(1)),
            kernel_config,
        )?;
        kernel.restore_snapshot_record(&kernel_snapshot_record(active_baseline))?;
        kernel.compact_journal_through(active_baseline.height)?;
        return Ok(kernel);
    }
    let replay_from = replay_entries.first().map(|entry| entry.seq).unwrap_or(0);
    let journal = Journal::from_entries(&replay_entries)
        .map_err(|err| LocalRuntimeError::Backend(err.to_string()))?;
    let mut kernel = Kernel::from_loaded_manifest_without_replay_with_config(
        store,
        loaded,
        journal,
        kernel_config,
    )?;
    kernel.restore_snapshot_record(&kernel_snapshot_record(active_baseline))?;
    kernel.replay_entries_from(replay_from)?;
    kernel.compact_journal_through(active_baseline.height)?;
    Ok(kernel)
}

fn runtime_info_from_world(world: &WorldSlot) -> WorldRuntimeInfo {
    let quiescence = world.kernel.quiescence_status();
    let journal_bounds = world.kernel.journal_bounds();
    WorldRuntimeInfo {
        world_id: world.world_id,
        universe_id: world.universe_id,
        created_at_ns: world.created_at_ns,
        manifest_hash: Some(world.kernel.manifest_hash().to_hex()),
        active_baseline_height: Some(world.active_baseline.height),
        notify_counter: world.next_world_seq,
        has_pending_inbox: world.ready || !world.mailbox.is_empty(),
        has_pending_effects: quiescence.queued_effects > 0
            || quiescence.pending_workflow_receipts > 0
            || quiescence.inflight_workflow_intents > 0
            || !world.scheduled_timers.is_empty(),
        next_timer_due_at_ns: world.timer_scheduler.next_due_at_ns(),
        has_pending_maintenance: journal_bounds.next_seq > journal_bounds.retained_from,
    }
}

impl LocalRuntime {
    fn checkpoint_due(&self, world: &WorldSlot, now_ns: u64) -> bool {
        let current_head = world.kernel.journal_head().saturating_sub(1);
        let head_delta = current_head.saturating_sub(world.active_baseline.height);
        if self
            .checkpoint_config
            .every_events
            .is_some_and(|threshold| head_delta >= u64::from(threshold))
        {
            return true;
        }
        head_delta > 0
            && now_ns.saturating_sub(world.last_checkpointed_at_ns)
                >= self
                    .checkpoint_config
                    .interval
                    .as_nanos()
                    .min(u128::from(u64::MAX)) as u64
    }
}

fn local_submission_universe_id() -> UniverseId {
    UniverseId::from(Uuid::nil())
}

fn parse_u64_env(field: &'static str, raw: &str) -> Result<u64, LocalRuntimeError> {
    raw.trim()
        .parse::<u64>()
        .map_err(|err| LocalRuntimeError::Backend(format!("invalid {field} value '{raw}': {err}")))
}

fn localize_create_request(mut request: CreateWorldRequest) -> CreateWorldRequest {
    request.universe_id = crate::UniverseId::nil();
    request
}

fn create_world_source_kind(request: &CreateWorldRequest) -> &'static str {
    match request.source {
        CreateWorldSource::Manifest { .. } => "manifest",
        CreateWorldSource::Seed { .. } => "seed",
    }
}

fn snapshot_record_from_frames(
    frames: &[WorldLogFrame],
    mut predicate: impl FnMut(&crate::SnapshotRecord) -> bool,
) -> Option<crate::SnapshotRecord> {
    for frame in frames.iter().rev() {
        for record in frame.records.iter().rev() {
            let JournalRecord::Snapshot(snapshot) = record else {
                continue;
            };
            let candidate = crate::SnapshotRecord {
                snapshot_ref: snapshot.snapshot_ref.clone(),
                height: snapshot.height,
                universe_id: snapshot.universe_id.into(),
                logical_time_ns: snapshot.logical_time_ns,
                receipt_horizon_height: snapshot.receipt_horizon_height,
                manifest_hash: snapshot.manifest_hash.clone(),
            };
            if predicate(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn journal_entries_from_world_frames_after_height(
    frames: &[WorldLogFrame],
    active_baseline_height: u64,
) -> Result<Vec<aos_kernel::journal::OwnedJournalEntry>, LocalRuntimeError> {
    let replay_from = active_baseline_height.saturating_add(1);
    let mut entries = Vec::new();
    for frame in frames {
        if frame.world_seq_end < replay_from {
            continue;
        }
        for (offset, record) in frame.records.iter().enumerate() {
            let seq = frame.world_seq_start + offset as u64;
            if seq < replay_from {
                continue;
            }
            entries.push(aos_kernel::journal::OwnedJournalEntry {
                seq,
                kind: record.kind(),
                payload: serde_cbor::to_vec(record)?,
            });
        }
    }
    Ok(entries)
}

fn latest_snapshot_from_kernel(kernel: &Kernel<FsCas>) -> Option<crate::SnapshotRecord> {
    kernel
        .dump_journal()
        .ok()?
        .into_iter()
        .rev()
        .find_map(
            |entry| match serde_cbor::from_slice::<JournalRecord>(&entry.payload).ok()? {
                JournalRecord::Snapshot(snapshot) => Some(crate::SnapshotRecord {
                    snapshot_ref: snapshot.snapshot_ref,
                    height: snapshot.height,
                    universe_id: snapshot.universe_id.into(),
                    logical_time_ns: snapshot.logical_time_ns,
                    receipt_horizon_height: snapshot.receipt_horizon_height,
                    manifest_hash: snapshot.manifest_hash,
                }),
                _ => None,
            },
        )
}

fn kernel_snapshot_record(snapshot: &crate::SnapshotRecord) -> KernelSnapshotRecord {
    KernelSnapshotRecord {
        snapshot_ref: snapshot.snapshot_ref.clone(),
        height: snapshot.height,
        universe_id: snapshot.universe_id.as_uuid(),
        logical_time_ns: snapshot.logical_time_ns,
        receipt_horizon_height: snapshot.receipt_horizon_height,
        manifest_hash: snapshot.manifest_hash.clone(),
    }
}

fn build_timer_receipt(entry: &TimerEntry) -> Result<EffectReceipt, LocalRuntimeError> {
    Ok(EffectReceipt {
        intent_hash: entry.intent_hash,
        adapter_id: "timer.local".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
            delivered_at_ns: entry.deliver_at_ns,
            key: entry.key.clone(),
        })?,
        cost_cents: None,
        signature: Vec::new(),
    })
}

fn parse_plane_hash_like(value: &str, field: &str) -> Result<Hash, LocalRuntimeError> {
    let trimmed = value.trim();
    let normalized = if trimmed.starts_with(aos_cbor::HASH_PREFIX) {
        trimmed.to_string()
    } else {
        format!("{}{}", aos_cbor::HASH_PREFIX, trimmed)
    };
    Hash::from_hex_str(&normalized)
        .map_err(|_| LocalRuntimeError::InvalidHashRef(format!("invalid {field} '{value}'")))
}

fn parse_plane_hash_ref(value: &str) -> Result<Hash, LocalRuntimeError> {
    let normalized = if value.starts_with(aos_cbor::HASH_PREFIX) {
        value.to_string()
    } else {
        format!("{}{}", aos_cbor::HASH_PREFIX, value)
    };
    Hash::from_hex_str(&normalized)
        .map_err(|_| LocalRuntimeError::InvalidHashRef(value.to_string()))
}

fn resolve_plane_cbor_payload<S: Store + ?Sized>(
    store: &S,
    payload: &CborPayload,
) -> Result<Vec<u8>, LocalRuntimeError> {
    payload
        .validate()
        .map_err(|err| LocalRuntimeError::Backend(err.to_string()))?;
    if let Some(inline) = &payload.inline_cbor {
        return Ok(inline.clone());
    }
    let Some(hash_ref) = payload.cbor_ref.as_deref() else {
        return Err(LocalRuntimeError::InvalidHashRef("<missing>".into()));
    };
    Ok(store.get_blob(parse_plane_hash_ref(hash_ref)?)?)
}

fn parse_plane_intent_hash(bytes: &[u8]) -> Result<[u8; 32], LocalRuntimeError> {
    bytes
        .try_into()
        .map_err(|_| LocalRuntimeError::Backend(format!("invalid intent hash len {}", bytes.len())))
}

fn now_wallclock_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_kernel::journal::{CustomRecord, JournalRecord};
    use uuid::Uuid;

    fn custom_record(tag: &str) -> JournalRecord {
        JournalRecord::Custom(CustomRecord {
            tag: tag.to_string(),
            data: Vec::new(),
        })
    }

    fn frame(world_seq_start: u64, records: Vec<JournalRecord>) -> WorldLogFrame {
        WorldLogFrame {
            format_version: 1,
            universe_id: crate::UniverseId::nil(),
            world_id: Uuid::nil().into(),
            world_epoch: 1,
            world_seq_start,
            world_seq_end: world_seq_start + records.len() as u64 - 1,
            records,
        }
    }

    #[test]
    fn journal_entries_after_height_skips_records_before_active_baseline() {
        let frames = vec![
            frame(
                0,
                vec![custom_record("0"), custom_record("1"), custom_record("2")],
            ),
            frame(
                3,
                vec![custom_record("3"), custom_record("4"), custom_record("5")],
            ),
        ];

        let entries = journal_entries_from_world_frames_after_height(&frames, 3).unwrap();
        let seqs = entries.iter().map(|entry| entry.seq).collect::<Vec<_>>();
        assert_eq!(seqs, vec![4, 5]);

        let tags = entries
            .into_iter()
            .map(
                |entry| match serde_cbor::from_slice::<JournalRecord>(&entry.payload).unwrap() {
                    JournalRecord::Custom(record) => record.tag,
                    other => panic!("expected custom record, got {other:?}"),
                },
            )
            .collect::<Vec<_>>();
        assert_eq!(tags, vec!["4".to_string(), "5".to_string()]);
    }
}
