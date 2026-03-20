use std::collections::{BTreeMap, HashSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use aos_cbor::{HASH_PREFIX, Hash};
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_effect_types::{
    GovApplyParams, GovApplyReceipt, GovApproveParams, GovApproveReceipt, GovDecision,
    GovLedgerChange, GovLedgerDelta, GovLedgerKind, GovModuleEffectAllowlist,
    GovPendingWorkflowReceipt, GovPredictedEffect, GovProposeParams, GovProposeReceipt,
    GovShadowParams, GovShadowReceipt, GovWorkflowInstancePreview, HashRef,
};
use aos_kernel::governance::ManifestPatch;
use aos_kernel::governance_utils::canonicalize_patch;
use aos_kernel::patch_doc::{PatchDocument, compile_patch_document};
use aos_kernel::{KernelConfig, KernelError, LoadedManifest, ManifestLoader};
use aos_kernel::{Store, StoreError};
use aos_node::control::ControlError;
use aos_node::{
    CborPayload, CommandErrorBody, CommandIngress, CommandRecord, CommandStatus, CommandStore,
    HotWorld, HotWorldError, InboxItem, NodeCatalog, SharedBlobCache, UniverseId, UniverseStore,
    WorkerHeartbeat, WorldAdminLifecycle, WorldAdminStatus, WorldId, WorldLease, WorldStore,
    apply_ingress_item_to_hot_world, encode_ingress_as_journal_entry, parse_hash_ref,
    resolve_cbor_payload,
};
use aos_runtime::trace::{TraceQuery, trace_get, workflow_trace_summary_with_routes};
use aos_runtime::{HostError, WorldConfig, now_wallclock_ns};
use aos_sqlite::{LocalSecretConfig, LocalSecretResolver, SqliteNodeStore};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::runtime::Builder;

const CMD_GOV_PROPOSE: &str = "gov-propose";
const CMD_GOV_SHADOW: &str = "gov-shadow";
const CMD_GOV_APPROVE: &str = "gov-approve";
const CMD_GOV_APPLY: &str = "gov-apply";
const CMD_WORLD_PAUSE: &str = "world-pause";
const CMD_WORLD_ARCHIVE: &str = "world-archive";
const CMD_WORLD_DELETE: &str = "world-delete";

#[derive(Debug, Clone)]
pub struct LocalSupervisorConfig {
    pub poll_interval: Duration,
}

impl Default for LocalSupervisorConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(100),
        }
    }
}

#[derive(Debug, Error)]
pub enum LocalNodeError {
    #[error(transparent)]
    Persist(#[from] aos_node::PersistError),
    #[error(transparent)]
    HotWorld(#[from] HotWorldError),
    #[error(transparent)]
    Host(#[from] HostError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
    #[error(transparent)]
    Cbor(#[from] serde_cbor::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Ref(#[from] aos_effect_types::RefError),
    #[error(transparent)]
    Runtime(#[from] std::io::Error),
    #[error("invalid hash reference '{0}'")]
    InvalidHash(String),
}

#[derive(Debug, Default, Deserialize, serde::Serialize)]
pub struct LifecycleCommandParams {
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Default)]
struct ControlCommandOutcome {
    journal_height: Option<u64>,
    manifest_hash: Option<String>,
    result_payload: Option<CborPayload>,
}

#[derive(Debug, Deserialize, Default)]
struct WorkspaceHistoryState {
    latest: u64,
    versions: BTreeMap<u64, WorkspaceCommitMetaState>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceCommitMetaState {
    root_hash: String,
    #[allow(dead_code)]
    owner: String,
    #[allow(dead_code)]
    created_at: u64,
}

#[derive(Default)]
struct SupervisorState {
    worlds: BTreeMap<(UniverseId, WorldId), HotWorld>,
}

pub struct LocalSupervisor {
    store: Arc<SqliteNodeStore>,
    config: LocalSupervisorConfig,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    secret_config: LocalSecretConfig,
    shared_cache: SharedBlobCache,
    state: Mutex<SupervisorState>,
    shutdown: AtomicBool,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl LocalSupervisor {
    pub fn new(
        store: Arc<SqliteNodeStore>,
        config: LocalSupervisorConfig,
        secret_config: LocalSecretConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            config,
            world_config: WorldConfig::default(),
            adapter_config: EffectAdapterConfig::default(),
            secret_config,
            shared_cache: SharedBlobCache::new(4096, 64 * 1024 * 1024, 8 * 1024 * 1024),
            state: Mutex::new(SupervisorState::default()),
            shutdown: AtomicBool::new(false),
            thread: Mutex::new(None),
        })
    }

    pub fn start(self: &Arc<Self>) {
        let mut thread_guard = self.thread.lock().expect("local supervisor mutex poisoned");
        if thread_guard.is_some() {
            return;
        }
        let this = Arc::clone(self);
        *thread_guard = Some(thread::spawn(move || {
            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build local supervisor tokio runtime");
            while !this.shutdown.load(Ordering::Relaxed) {
                if let Err(err) = this.tick_all(&runtime) {
                    tracing::warn!("local supervisor tick failed: {err}");
                }
                thread::sleep(this.config.poll_interval);
            }
        }));
    }

    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self
            .thread
            .lock()
            .expect("local supervisor mutex poisoned")
            .take()
        {
            let _ = handle.join();
        }
    }

    pub fn ensure_hot(&self, universe: UniverseId, world: WorldId) -> Result<(), LocalNodeError> {
        self.ensure_loaded(universe, world)
    }

    pub fn runtime_info(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<aos_node::WorldRuntimeInfo, LocalNodeError> {
        self.ensure_loaded(universe, world)?;
        let mut runtime = self
            .store
            .world_runtime_info(universe, world, now_wallclock_ns())?;
        let state = self.state.lock().expect("local supervisor mutex poisoned");
        if let Some(hot) = state.worlds.get(&(universe, world)) {
            runtime.has_pending_effects = hot.host.has_pending_effects();
            runtime.next_timer_due_at_ns = hot.scheduler.next_due_at_ns();
        }
        Ok(runtime)
    }

    pub fn worker_heartbeat(&self, worker_id: &str) -> WorkerHeartbeat {
        let now_ns = now_wallclock_ns();
        WorkerHeartbeat {
            worker_id: worker_id.to_string(),
            pins: Vec::new(),
            last_seen_ns: now_ns,
            expires_at_ns: now_ns.saturating_add(self.config.poll_interval.as_nanos() as u64),
        }
    }

    pub fn world_summary(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<aos_node::control::WorldSummaryResponse, LocalNodeError> {
        Ok(aos_node::control::WorldSummaryResponse {
            runtime: self.runtime_info(universe, world)?,
            active_baseline: self.store.snapshot_active_baseline(universe, world)?,
        })
    }

    pub fn list_worlds(
        &self,
        universe: UniverseId,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<aos_node::WorldRuntimeInfo>, LocalNodeError> {
        let mut worlds = self
            .store
            .list_worlds(universe, now_wallclock_ns(), after, limit)?;
        for runtime in &mut worlds {
            if let Ok(live) = self.runtime_info(universe, runtime.world_id) {
                *runtime = live;
            }
        }
        Ok(worlds)
    }

    pub fn worker_worlds(
        &self,
        universe: UniverseId,
        worker_id: &str,
        limit: u32,
        local_worker_id: &str,
    ) -> Result<Vec<aos_node::WorldRuntimeInfo>, LocalNodeError> {
        if worker_id != local_worker_id || limit == 0 {
            return Ok(Vec::new());
        }
        self.sync_catalog()?;
        let expires_at_ns =
            now_wallclock_ns().saturating_add(self.config.poll_interval.as_nanos() as u64);
        let active = {
            let state = self.state.lock().expect("local supervisor mutex poisoned");
            state
                .worlds
                .keys()
                .filter(|(u, _)| *u == universe)
                .map(|(_, world)| *world)
                .collect::<Vec<_>>()
        };
        let mut worlds = Vec::new();
        for world in active {
            if worlds.len() >= limit as usize {
                break;
            }
            let mut runtime = self.runtime_info(universe, world)?;
            runtime.lease = Some(WorldLease {
                holder_worker_id: worker_id.to_string(),
                epoch: 1,
                expires_at_ns,
            });
            worlds.push(runtime);
        }
        worlds.sort_by_key(|runtime| runtime.world_id);
        Ok(worlds)
    }

    pub(crate) fn with_world<T>(
        &self,
        universe: UniverseId,
        world: WorldId,
        operation: impl FnOnce(&HotWorld) -> Result<T, LocalNodeError>,
    ) -> Result<T, LocalNodeError> {
        self.ensure_loaded(universe, world)?;
        let state = self.state.lock().expect("local supervisor mutex poisoned");
        let hot = state
            .worlds
            .get(&(universe, world))
            .ok_or_else(|| aos_node::PersistError::not_found(format!("world {world}")))?;
        operation(hot)
    }

    fn with_world_mut<T>(
        &self,
        universe: UniverseId,
        world: WorldId,
        operation: impl FnOnce(&mut HotWorld) -> Result<T, LocalNodeError>,
    ) -> Result<T, LocalNodeError> {
        self.ensure_loaded(universe, world)?;
        let mut state = self.state.lock().expect("local supervisor mutex poisoned");
        let hot = state
            .worlds
            .get_mut(&(universe, world))
            .ok_or_else(|| aos_node::PersistError::not_found(format!("world {world}")))?;
        operation(hot)
    }

    pub fn execute_command(
        &self,
        universe: UniverseId,
        world: WorldId,
        control: CommandIngress,
    ) -> Result<CommandRecord, LocalNodeError> {
        self.ensure_loaded(universe, world)?;
        self.with_world_mut(universe, world, |hot| {
            self.execute_command_in_hot(universe, world, hot, control)
        })
    }

    pub fn run_ingress_once(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<(), LocalNodeError> {
        self.ensure_loaded(universe, world)?;
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| {
                let runtime = Builder::new_current_thread().enable_all().build()?;
                self.pump_world(&runtime, universe, world)
            })
        } else {
            let runtime = Builder::new_current_thread().enable_all().build()?;
            self.pump_world(&runtime, universe, world)
        }
    }

    fn tick_all(&self, runtime: &tokio::runtime::Runtime) -> Result<(), LocalNodeError> {
        self.sync_catalog()?;
        let keys = {
            let state = self.state.lock().expect("local supervisor mutex poisoned");
            state.worlds.keys().copied().collect::<Vec<_>>()
        };
        for (universe, world) in keys {
            self.pump_world(runtime, universe, world)?;
        }
        Ok(())
    }

    fn sync_catalog(&self) -> Result<(), LocalNodeError> {
        let universes = self.store.list_universes(None, u32::MAX)?;
        let mut live = HashSet::new();
        for universe in universes {
            if matches!(
                universe.admin.status,
                aos_node::UniverseAdminStatus::Deleted
            ) {
                continue;
            }
            let worlds =
                self.store
                    .list_worlds(universe.universe_id, now_wallclock_ns(), None, u32::MAX)?;
            for runtime in worlds {
                if matches!(runtime.meta.admin.status, WorldAdminStatus::Deleted) {
                    continue;
                }
                let key = (universe.universe_id, runtime.world_id);
                live.insert(key);
                self.ensure_loaded(key.0, key.1)?;
            }
        }
        let mut state = self.state.lock().expect("local supervisor mutex poisoned");
        state.worlds.retain(|key, _| live.contains(key));
        Ok(())
    }

    fn ensure_loaded(&self, universe: UniverseId, world: WorldId) -> Result<(), LocalNodeError> {
        {
            let state = self.state.lock().expect("local supervisor mutex poisoned");
            if state.worlds.contains_key(&(universe, world)) {
                return Ok(());
            }
        }
        let persistence: Arc<dyn WorldStore> = self.store.clone();
        let resolver = Arc::new(LocalSecretResolver::new(
            Arc::clone(&self.store),
            universe,
            self.secret_config.clone(),
        ));
        let hot = HotWorld::open(
            Arc::clone(&persistence),
            universe,
            world,
            self.world_config.clone(),
            self.adapter_config.clone(),
            KernelConfig {
                secret_resolver: Some(resolver),
                ..KernelConfig::default()
            },
            Some(self.shared_cache.clone()),
        )?;
        let mut state = self.state.lock().expect("local supervisor mutex poisoned");
        state.worlds.insert((universe, world), hot);
        Ok(())
    }

    fn pump_world(
        &self,
        runtime: &tokio::runtime::Runtime,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<(), LocalNodeError> {
        self.ensure_loaded(universe, world)?;
        let mut state = self.state.lock().expect("local supervisor mutex poisoned");
        let hot = state
            .worlds
            .get_mut(&(universe, world))
            .ok_or_else(|| aos_node::PersistError::not_found(format!("world {world}")))?;

        let _ingress = self.drain_ingress_batch(universe, world, hot)?;
        let _drain = runtime.block_on(hot.run_daemon_until_quiescent())?;

        let runtime_info = self
            .store
            .world_runtime_info(universe, world, now_wallclock_ns())?;
        if runtime_info.has_pending_maintenance {
            let persistence: Arc<dyn WorldStore> = self.store.clone();
            hot.snapshot(persistence, universe, world)?;
        }
        if let Some(next_admin) = finalize_quiescent_admin(&runtime_info.meta.admin) {
            self.store
                .set_world_admin_lifecycle(universe, world, next_admin)?;
        }
        Ok(())
    }

    fn drain_ingress_batch(
        &self,
        universe: UniverseId,
        world: WorldId,
        hot: &mut HotWorld,
    ) -> Result<u32, LocalNodeError> {
        let old_cursor = self.store.inbox_cursor(universe, world)?;
        let items = self
            .store
            .inbox_read_after(universe, world, old_cursor.clone(), 256)?;
        if items.is_empty() {
            return Ok(0);
        }

        let mut consumed = 0u32;
        let mut batch_old_cursor = old_cursor;
        let mut batch_expected_head = self.store.journal_head(universe, world)?;
        let mut batch_items = Vec::new();

        for (seq, item) in items {
            if let InboxItem::Control(control) = item {
                let control_old_cursor = batch_items
                    .last()
                    .map(|(item_seq, _): &(aos_node::InboxSeq, InboxItem)| item_seq.clone())
                    .or(batch_old_cursor.clone());
                consumed = consumed.saturating_add(self.flush_journal_batch(
                    universe,
                    world,
                    hot,
                    batch_old_cursor.take(),
                    &mut batch_expected_head,
                    &mut batch_items,
                )?);
                self.store
                    .inbox_commit_cursor(universe, world, control_old_cursor, seq.clone())?;
                let _ = self.execute_command_in_hot(universe, world, hot, control)?;
                batch_old_cursor = Some(seq);
                batch_expected_head = self.store.journal_head(universe, world)?;
                consumed = consumed.saturating_add(1);
            } else {
                batch_items.push((seq, item));
            }
        }

        consumed = consumed.saturating_add(self.flush_journal_batch(
            universe,
            world,
            hot,
            batch_old_cursor,
            &mut batch_expected_head,
            &mut batch_items,
        )?);
        Ok(consumed)
    }

    fn flush_journal_batch(
        &self,
        universe: UniverseId,
        world: WorldId,
        hot: &mut HotWorld,
        old_cursor: Option<aos_node::InboxSeq>,
        expected_head: &mut u64,
        items: &mut Vec<(aos_node::InboxSeq, InboxItem)>,
    ) -> Result<u32, LocalNodeError> {
        if items.is_empty() {
            return Ok(0);
        }

        let mut journal_entries = Vec::with_capacity(items.len());
        let mut applied_items = Vec::with_capacity(items.len());
        let mut last_seq = None;
        for (offset, (seq, item)) in items.iter().cloned().enumerate() {
            let journal_seq = expected_head.saturating_add(offset as u64);
            journal_entries.push(encode_ingress_as_journal_entry(
                &*self.store,
                universe,
                hot,
                journal_seq,
                item.clone(),
            )?);
            applied_items.push(item);
            last_seq = Some(seq);
        }

        let first_height = self.store.drain_inbox_to_journal(
            universe,
            world,
            old_cursor,
            last_seq.expect("batch has sequence"),
            *expected_head,
            &journal_entries,
        )?;
        debug_assert_eq!(first_height, *expected_head);
        *expected_head = expected_head.saturating_add(journal_entries.len() as u64);
        hot.set_journal_next_seq(*expected_head);
        for item in applied_items {
            apply_ingress_item_to_hot_world(&*self.store, universe, hot, item)?;
        }
        items.clear();
        Ok(journal_entries.len() as u32)
    }

    fn execute_command_in_hot(
        &self,
        universe: UniverseId,
        world: WorldId,
        hot: &mut HotWorld,
        control: CommandIngress,
    ) -> Result<CommandRecord, LocalNodeError> {
        let Some(existing) = self
            .store
            .command_record(universe, world, &control.command_id)?
        else {
            return Err(aos_node::PersistError::not_found(format!(
                "command {}",
                control.command_id
            ))
            .into());
        };

        if matches!(
            existing.status,
            CommandStatus::Succeeded | CommandStatus::Failed
        ) {
            return Ok(existing);
        }

        let started_at_ns = existing.started_at_ns.unwrap_or_else(now_wallclock_ns);
        let mut running = existing.clone();
        running.status = CommandStatus::Running;
        running.started_at_ns = Some(started_at_ns);
        running.finished_at_ns = None;
        running.error = None;
        self.store
            .update_command_record(universe, world, running.clone())?;

        let final_record = match self.run_control_command_in_hot(
            universe,
            world,
            hot,
            &control,
            &resolve_cbor_payload(&*self.store, universe, &control.payload)?,
        ) {
            Ok(outcome) => {
                let mut record = running;
                record.status = CommandStatus::Succeeded;
                record.finished_at_ns = Some(now_wallclock_ns());
                record.journal_height = outcome.journal_height;
                record.manifest_hash = outcome.manifest_hash;
                record.result_payload = outcome.result_payload;
                record.error = None;
                record
            }
            Err(err) => {
                let mut record = running;
                record.status = CommandStatus::Failed;
                record.finished_at_ns = Some(now_wallclock_ns());
                record.journal_height = Some(self.store.journal_head(universe, world)?);
                record.manifest_hash = Some(hot.host.kernel().manifest_hash().to_hex());
                record.result_payload = None;
                record.error = Some(command_error_body(&err));
                record
            }
        };
        self.store
            .update_command_record(universe, world, final_record.clone())?;
        Ok(final_record)
    }

    fn run_control_command_in_hot(
        &self,
        universe: UniverseId,
        world: WorldId,
        hot: &mut HotWorld,
        control: &CommandIngress,
        payload: &[u8],
    ) -> Result<ControlCommandOutcome, LocalNodeError> {
        match control.command.as_str() {
            CMD_GOV_PROPOSE => run_gov_propose(hot, control, &payload),
            CMD_GOV_SHADOW => run_gov_shadow(hot, &payload),
            CMD_GOV_APPROVE => run_gov_approve(hot, &payload),
            CMD_GOV_APPLY => run_gov_apply(hot, &payload),
            CMD_WORLD_PAUSE => run_lifecycle_command(
                &self.store,
                universe,
                world,
                hot,
                control,
                WorldAdminStatus::Pausing,
                &payload,
            ),
            CMD_WORLD_ARCHIVE => run_lifecycle_command(
                &self.store,
                universe,
                world,
                hot,
                control,
                WorldAdminStatus::Archiving,
                &payload,
            ),
            CMD_WORLD_DELETE => run_lifecycle_command(
                &self.store,
                universe,
                world,
                hot,
                control,
                WorldAdminStatus::Deleting,
                &payload,
            ),
            other => Err(HostError::External(format!("unknown command '{other}'")).into()),
        }
    }

    pub fn manifest(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<(u64, String, LoadedManifest), LocalNodeError> {
        self.with_world(universe, world, |hot| {
            let manifest_hash = hot.host.kernel().manifest_hash();
            let loaded = ManifestLoader::load_from_hash(hot.host.store(), manifest_hash)?;
            Ok((hot.host.heights().head, manifest_hash.to_hex(), loaded))
        })
    }

    pub fn defs_list(
        &self,
        universe: UniverseId,
        world: WorldId,
        kinds: Option<&[String]>,
        prefix: Option<&str>,
    ) -> Result<(u64, String, Vec<aos_kernel::DefListing>), LocalNodeError> {
        self.with_world(universe, world, |hot| {
            Ok((
                hot.host.heights().head,
                hot.host.kernel().manifest_hash().to_hex(),
                hot.host.list_defs(kinds, prefix)?,
            ))
        })
    }

    pub fn def_get(
        &self,
        universe: UniverseId,
        world: WorldId,
        name: &str,
    ) -> Result<(u64, String, aos_air_types::AirNode), LocalNodeError> {
        self.with_world(universe, world, |hot| {
            Ok((
                hot.host.heights().head,
                hot.host.kernel().manifest_hash().to_hex(),
                hot.host.get_def(name)?,
            ))
        })
    }

    pub fn state_get(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        key: Option<Vec<u8>>,
    ) -> Result<aos_node::control::StateGetResponse, LocalNodeError> {
        self.with_world(universe, world, |hot| {
            let key_bytes = key.unwrap_or_default();
            let state = hot.host.state(workflow, Some(&key_bytes));
            let state_hash = state.as_ref().map(|bytes| Hash::of_bytes(bytes).to_hex());
            let size = state.as_ref().map(|bytes| bytes.len() as u64).unwrap_or(0);
            let cell = state_hash.map(|state_hash| aos_node::CellStateProjectionRecord {
                journal_head: hot.host.heights().head,
                workflow: workflow.to_string(),
                key_hash: Hash::of_bytes(&key_bytes).as_bytes().to_vec(),
                key_bytes: key_bytes.clone(),
                state_hash,
                size,
                last_active_ns: 0,
            });
            Ok(aos_node::control::StateGetResponse {
                journal_head: hot.host.heights().head,
                workflow: workflow.to_string(),
                key_b64: Some(BASE64_STANDARD.encode(&key_bytes)),
                cell,
                state_b64: state.map(|bytes| BASE64_STANDARD.encode(bytes)),
            })
        })
    }

    pub fn state_list(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        limit: u32,
    ) -> Result<aos_node::control::StateListResponse, LocalNodeError> {
        self.with_world(universe, world, |hot| {
            let mut cells = hot
                .host
                .list_cells(workflow)?
                .into_iter()
                .map(|cell| aos_node::CellStateProjectionRecord {
                    journal_head: hot.host.heights().head,
                    workflow: workflow.to_string(),
                    key_hash: cell.key_hash.to_vec(),
                    key_bytes: cell.key_bytes,
                    state_hash: Hash::from(cell.state_hash).to_hex(),
                    size: cell.size,
                    last_active_ns: cell.last_active_ns,
                })
                .collect::<Vec<_>>();
            cells.truncate(limit as usize);
            Ok(aos_node::control::StateListResponse {
                journal_head: hot.host.heights().head,
                workflow: workflow.to_string(),
                cells,
            })
        })
    }

    pub fn workspace_resolve(
        &self,
        universe: UniverseId,
        world: WorldId,
        workspace: &str,
        version: Option<u64>,
    ) -> Result<aos_node::control::WorkspaceResolveResponse, LocalNodeError> {
        self.with_world(universe, world, |hot| {
            let key = serde_cbor::to_vec(&workspace.to_string())?;
            let history = hot.host.state("sys/Workspace@1", Some(&key));
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
            Ok(aos_node::control::WorkspaceResolveResponse {
                workspace: workspace.to_string(),
                receipt,
            })
        })
    }

    pub fn trace(
        &self,
        universe: UniverseId,
        world: WorldId,
        query: TraceQuery,
    ) -> Result<Value, LocalNodeError> {
        self.with_world(universe, world, |hot| {
            Ok(trace_get(hot.host.kernel(), query)?)
        })
    }

    pub fn trace_summary(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Value, LocalNodeError> {
        self.with_world(universe, world, |hot| {
            Ok(workflow_trace_summary_with_routes(
                hot.host.kernel(),
                Some(hot.host.effect_route_diagnostics()),
            )?)
        })
    }
}

impl Drop for LocalSupervisor {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self
            .thread
            .lock()
            .expect("local supervisor mutex poisoned")
            .take()
        {
            let _ = handle.join();
        }
    }
}

fn command_success_outcome<T: serde::Serialize>(
    value: &T,
) -> Result<ControlCommandOutcome, LocalNodeError> {
    Ok(ControlCommandOutcome {
        result_payload: Some(CborPayload::inline(serde_cbor::to_vec(value)?)),
        ..ControlCommandOutcome::default()
    })
}

fn command_error_body(err: &LocalNodeError) -> CommandErrorBody {
    let code = match err {
        LocalNodeError::Persist(aos_node::PersistError::NotFound(_)) => "not_found",
        LocalNodeError::Persist(aos_node::PersistError::Conflict(_)) => "conflict",
        LocalNodeError::Persist(aos_node::PersistError::Validation(_)) => "validation_failed",
        LocalNodeError::Kernel(KernelError::ProposalNotFound(_)) => "not_found",
        LocalNodeError::Kernel(KernelError::ProposalAlreadyApplied(_))
        | LocalNodeError::Kernel(KernelError::ProposalStateInvalid { .. })
        | LocalNodeError::Kernel(KernelError::ManifestApplyBlockedInFlight { .. }) => "conflict",
        LocalNodeError::Json(_) | LocalNodeError::Cbor(_) => "invalid_request",
        _ => "command_failed",
    };
    CommandErrorBody {
        code: code.into(),
        message: err.to_string(),
    }
}

fn hash_ref_from_hex(hex: &str) -> Result<HashRef, LocalNodeError> {
    let value = if hex.starts_with(HASH_PREFIX) {
        hex.to_string()
    } else {
        format!("{HASH_PREFIX}{hex}")
    };
    HashRef::new(value).map_err(|_| LocalNodeError::InvalidHash(hex.into()))
}

fn next_admin_lifecycle(
    current: &WorldAdminLifecycle,
    world_id: WorldId,
    target_status: WorldAdminStatus,
    command_id: &str,
    reason: Option<String>,
    now_ns: u64,
) -> Result<WorldAdminLifecycle, LocalNodeError> {
    let same_operation = current.operation_id.as_deref() == Some(command_id);
    let already_done = matches!(
        (target_status, current.status),
        (WorldAdminStatus::Pausing, WorldAdminStatus::Paused)
            | (WorldAdminStatus::Archiving, WorldAdminStatus::Archived)
            | (WorldAdminStatus::Deleting, WorldAdminStatus::Deleted)
    );
    if same_operation && (current.status == target_status || already_done) {
        return Ok(current.clone());
    }

    let allowed = match target_status {
        WorldAdminStatus::Pausing => matches!(current.status, WorldAdminStatus::Active),
        WorldAdminStatus::Archiving => matches!(
            current.status,
            WorldAdminStatus::Active
                | WorldAdminStatus::Paused
                | WorldAdminStatus::Pausing
                | WorldAdminStatus::Archived
        ),
        WorldAdminStatus::Deleting => !matches!(current.status, WorldAdminStatus::Deleted),
        _ => false,
    };
    if !allowed {
        return Err(aos_node::PersistError::Conflict(
            aos_node::PersistConflict::WorldAdminBlocked {
                world_id,
                status: current.status,
                action: format!("transition to {target_status:?}"),
            },
        )
        .into());
    }

    Ok(WorldAdminLifecycle {
        status: target_status,
        updated_at_ns: now_ns,
        operation_id: Some(command_id.into()),
        reason,
    })
}

fn finalize_quiescent_admin(current: &WorldAdminLifecycle) -> Option<WorldAdminLifecycle> {
    let status = match current.status {
        WorldAdminStatus::Pausing => WorldAdminStatus::Paused,
        WorldAdminStatus::Archiving => WorldAdminStatus::Archived,
        WorldAdminStatus::Deleting => WorldAdminStatus::Deleted,
        _ => return None,
    };
    let mut admin = current.clone();
    admin.status = status;
    admin.updated_at_ns = now_wallclock_ns();
    Some(admin)
}

fn run_gov_propose(
    hot: &mut HotWorld,
    control: &CommandIngress,
    payload: &[u8],
) -> Result<ControlCommandOutcome, LocalNodeError> {
    let params: GovProposeParams = serde_cbor::from_slice(payload)?;
    let patch = prepare_manifest_patch(hot, params.patch.clone(), params.manifest_base.clone())?;
    let proposal_id = hot
        .host
        .kernel_mut()
        .submit_proposal(patch, params.description.clone())?;
    let proposal = hot
        .host
        .kernel()
        .governance()
        .proposals()
        .get(&proposal_id)
        .ok_or(KernelError::ProposalNotFound(proposal_id))?;
    let receipt = GovProposeReceipt {
        proposal_id,
        patch_hash: hash_ref_from_hex(&proposal.patch_hash)?,
        manifest_base: params.manifest_base,
    };
    let mut outcome = command_success_outcome(&receipt)?;
    outcome.journal_height = Some(hot.host.heights().head);
    outcome.manifest_hash = Some(hot.host.kernel().manifest_hash().to_hex());
    let _ = control;
    Ok(outcome)
}

fn run_gov_shadow(
    hot: &mut HotWorld,
    payload: &[u8],
) -> Result<ControlCommandOutcome, LocalNodeError> {
    let params: GovShadowParams = serde_cbor::from_slice(payload)?;
    let summary = hot.host.kernel_mut().run_shadow(params.proposal_id, None)?;
    let receipt = GovShadowReceipt {
        proposal_id: params.proposal_id,
        manifest_hash: hash_ref_from_hex(&summary.manifest_hash)?,
        predicted_effects: summary
            .predicted_effects
            .into_iter()
            .map(|effect| {
                Ok(GovPredictedEffect {
                    kind: effect.kind,
                    cap: effect.cap,
                    intent_hash: hash_ref_from_hex(&effect.intent_hash)?,
                    params_json: effect
                        .params_json
                        .map(|value| serde_json::to_string(&value))
                        .transpose()?,
                })
            })
            .collect::<Result<Vec<_>, LocalNodeError>>()?,
        pending_workflow_receipts: summary
            .pending_workflow_receipts
            .into_iter()
            .map(|pending| {
                Ok(GovPendingWorkflowReceipt {
                    instance_id: pending.instance_id,
                    origin_module_id: pending.origin_module_id,
                    origin_instance_key_b64: pending.origin_instance_key_b64,
                    intent_hash: hash_ref_from_hex(&pending.intent_hash)?,
                    effect_kind: pending.effect_kind,
                    emitted_at_seq: pending.emitted_at_seq,
                })
            })
            .collect::<Result<Vec<_>, LocalNodeError>>()?,
        workflow_instances: summary
            .workflow_instances
            .into_iter()
            .map(|instance| GovWorkflowInstancePreview {
                instance_id: instance.instance_id,
                status: instance.status,
                last_processed_event_seq: instance.last_processed_event_seq,
                module_version: instance.module_version,
                inflight_intents: instance.inflight_intents as u64,
            })
            .collect(),
        module_effect_allowlists: summary
            .module_effect_allowlists
            .into_iter()
            .map(|allowlist| GovModuleEffectAllowlist {
                module: allowlist.module,
                effects_emitted: allowlist.effects_emitted,
            })
            .collect(),
        ledger_deltas: summary
            .ledger_deltas
            .into_iter()
            .map(|delta| GovLedgerDelta {
                ledger: match delta.ledger {
                    aos_kernel::shadow::LedgerKind::Capability => GovLedgerKind::Capability,
                    aos_kernel::shadow::LedgerKind::Policy => GovLedgerKind::Policy,
                },
                name: delta.name,
                change: match delta.change {
                    aos_kernel::shadow::DeltaKind::Added => GovLedgerChange::Added,
                    aos_kernel::shadow::DeltaKind::Removed => GovLedgerChange::Removed,
                    aos_kernel::shadow::DeltaKind::Changed => GovLedgerChange::Changed,
                },
            })
            .collect(),
    };
    let mut outcome = command_success_outcome(&receipt)?;
    outcome.journal_height = Some(hot.host.heights().head);
    outcome.manifest_hash = Some(hot.host.kernel().manifest_hash().to_hex());
    Ok(outcome)
}

fn run_gov_approve(
    hot: &mut HotWorld,
    payload: &[u8],
) -> Result<ControlCommandOutcome, LocalNodeError> {
    let params: GovApproveParams = serde_cbor::from_slice(payload)?;
    match params.decision {
        GovDecision::Approve => hot
            .host
            .kernel_mut()
            .approve_proposal(params.proposal_id, params.approver.clone())?,
        GovDecision::Reject => hot
            .host
            .kernel_mut()
            .reject_proposal(params.proposal_id, params.approver.clone())?,
    }
    let proposal = hot
        .host
        .kernel()
        .governance()
        .proposals()
        .get(&params.proposal_id)
        .ok_or(KernelError::ProposalNotFound(params.proposal_id))?;
    let receipt = GovApproveReceipt {
        proposal_id: params.proposal_id,
        decision: params.decision,
        patch_hash: hash_ref_from_hex(&proposal.patch_hash)?,
        approver: params.approver,
        reason: params.reason,
    };
    let mut outcome = command_success_outcome(&receipt)?;
    outcome.journal_height = Some(hot.host.heights().head);
    outcome.manifest_hash = Some(hot.host.kernel().manifest_hash().to_hex());
    Ok(outcome)
}

fn run_gov_apply(
    hot: &mut HotWorld,
    payload: &[u8],
) -> Result<ControlCommandOutcome, LocalNodeError> {
    let params: GovApplyParams = serde_cbor::from_slice(payload)?;
    let proposal = hot
        .host
        .kernel()
        .governance()
        .proposals()
        .get(&params.proposal_id)
        .ok_or(KernelError::ProposalNotFound(params.proposal_id))?
        .clone();
    hot.host.kernel_mut().apply_proposal(params.proposal_id)?;
    let receipt = GovApplyReceipt {
        proposal_id: params.proposal_id,
        manifest_hash_new: hash_ref_from_hex(&hot.host.kernel().manifest_hash().to_hex())?,
        patch_hash: hash_ref_from_hex(&proposal.patch_hash)?,
    };
    let mut outcome = command_success_outcome(&receipt)?;
    outcome.journal_height = Some(hot.host.heights().head);
    outcome.manifest_hash = Some(hot.host.kernel().manifest_hash().to_hex());
    Ok(outcome)
}

fn run_lifecycle_command(
    store: &SqliteNodeStore,
    universe: UniverseId,
    world: WorldId,
    hot: &mut HotWorld,
    control: &CommandIngress,
    target_status: WorldAdminStatus,
    payload: &[u8],
) -> Result<ControlCommandOutcome, LocalNodeError> {
    let params: LifecycleCommandParams = serde_cbor::from_slice(payload)?;
    let now_ns = now_wallclock_ns();
    let info = store.world_runtime_info(universe, world, now_ns)?;
    let next_admin = next_admin_lifecycle(
        &info.meta.admin,
        world,
        target_status,
        &control.command_id,
        params.reason,
        now_ns,
    )?;
    store.set_world_admin_lifecycle(universe, world, next_admin.clone())?;
    let mut outcome = command_success_outcome(&next_admin)?;
    outcome.journal_height = Some(store.journal_head(universe, world)?);
    outcome.manifest_hash = info
        .meta
        .manifest_hash
        .or_else(|| Some(hot.host.kernel().manifest_hash().to_hex()));
    Ok(outcome)
}

fn prepare_manifest_patch(
    hot: &HotWorld,
    input: aos_effect_types::GovPatchInput,
    manifest_base: Option<HashRef>,
) -> Result<ManifestPatch, LocalNodeError> {
    match input {
        aos_effect_types::GovPatchInput::Hash(hash) => {
            if manifest_base.is_some() {
                return Err(KernelError::Manifest(
                    "manifest_base is not supported with patch hash input".into(),
                )
                .into());
            }
            let bytes = hot
                .host
                .store()
                .get_blob(parse_hash_ref(hash.as_str())?)
                .map_err(StoreError::from)?;
            Ok(serde_cbor::from_slice(&bytes)?)
        }
        aos_effect_types::GovPatchInput::PatchCbor(bytes) => {
            if manifest_base.is_some() {
                return Err(KernelError::Manifest(
                    "manifest_base is not supported with patch_cbor input".into(),
                )
                .into());
            }
            let patch: ManifestPatch = serde_cbor::from_slice(&bytes)?;
            canonicalize_patch(hot.host.store(), patch).map_err(LocalNodeError::Kernel)
        }
        aos_effect_types::GovPatchInput::PatchDocJson(bytes) => {
            let doc: PatchDocument = serde_json::from_slice(&bytes)?;
            if let Some(expected) = manifest_base.as_ref()
                && expected.as_str() != doc.base_manifest_hash
            {
                return Err(KernelError::Manifest(format!(
                    "manifest_base mismatch: expected {expected}, got {}",
                    doc.base_manifest_hash
                ))
                .into());
            }
            compile_patch_document(hot.host.store(), doc).map_err(LocalNodeError::Kernel)
        }
        aos_effect_types::GovPatchInput::PatchBlobRef { blob_ref, format } => {
            let bytes = hot
                .host
                .store()
                .get_blob(parse_hash_ref(blob_ref.as_str())?)
                .map_err(StoreError::from)?;
            match format.as_str() {
                "manifest_patch_cbor" => prepare_manifest_patch(
                    hot,
                    aos_effect_types::GovPatchInput::PatchCbor(bytes),
                    manifest_base,
                ),
                "patch_doc_json" => prepare_manifest_patch(
                    hot,
                    aos_effect_types::GovPatchInput::PatchDocJson(bytes),
                    manifest_base,
                ),
                other => Err(
                    KernelError::Manifest(format!("unknown patch blob format '{other}'")).into(),
                ),
            }
        }
    }
}

impl From<LocalNodeError> for ControlError {
    fn from(value: LocalNodeError) -> Self {
        match value {
            LocalNodeError::Persist(err) => ControlError::Persist(err),
            LocalNodeError::Kernel(err) => ControlError::Kernel(err),
            LocalNodeError::Store(err) => ControlError::Store(err),
            LocalNodeError::Cbor(err) => ControlError::Cbor(err),
            LocalNodeError::Json(err) => ControlError::Json(err),
            other => ControlError::invalid(other.to_string()),
        }
    }
}
