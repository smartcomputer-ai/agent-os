use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};
use std::sync::MutexGuard;
use std::sync::{Arc, Mutex};

use crate::api::{
    ControlError, DefGetResponse, DefsListResponse, HeadInfoResponse, JournalEntriesResponse,
    JournalEntryResponse, ManifestResponse, RawJournalEntriesResponse, RawJournalEntryResponse,
    StateCellSummary, StateGetResponse, StateListResponse, WorkspaceApplyOp, WorkspaceApplyRequest,
    WorkspaceApplyResponse, WorkspaceResolveResponse,
};
use crate::{
    CborPayload, CommandErrorBody, CommandIngress, CommandRecord, CommandStatus,
    CreateWorldRequest, CreateWorldSource, DomainEventIngress, ForkWorldRequest, InboxSeq,
    PersistError, PlaneError, ReceiptIngress, SeedKind, SnapshotSelector, UniverseId,
    WorldCreateResult, WorldId, WorldRecord, WorldRuntimeInfo, create_plane_world_from_request,
    open_plane_world_from_checkpoint, parse_plane_hash_like, resolve_plane_cbor_payload,
    rewrite_snapshot_for_fork_policy, run_governance_plane_command,
    submission_payload_to_external_event,
};
use crate::{SubmissionEnvelope, SubmissionPayload, WorldLogFrame};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_effect_types::HashRef;
use aos_kernel::journal::JournalRecord;
use aos_kernel::{
    Kernel, KernelConfig, KernelError, LoadedManifest, ManifestLoader, Store, StoreError,
};
use aos_runtime::trace::{TraceQuery, trace_get, workflow_trace_summary_with_routes};
use aos_runtime::{HostError, WorldConfig, WorldHost, now_wallclock_ns};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use super::workspace as local_workspace;
use super::{
    FsCas, LocalBlobPlanes, LocalSqlitePlanes, LocalStatePaths, LocalStoreError,
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
    Host(#[from] HostError),
    #[error(transparent)]
    LogFirst(#[from] PlaneError),
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
            LocalRuntimeError::LogFirst(err) => ControlError::invalid(err.to_string()),
            other => ControlError::invalid(other.to_string()),
        }
    }
}

struct HotWorld {
    world_id: WorldId,
    universe_id: crate::UniverseId,
    created_at_ns: u64,
    initial_manifest_hash: String,
    world_epoch: u64,
    active_baseline: crate::SnapshotRecord,
    next_world_seq: u64,
    host: WorldHost<FsCas>,
    command_records: BTreeMap<String, CommandRecord>,
}

struct RuntimeState {
    sqlite: LocalSqlitePlanes,
    next_submission_seq: u64,
    next_frame_offset: u64,
    worlds: BTreeMap<WorldId, HotWorld>,
}

pub struct LocalKernelGuard<'a> {
    runtime: &'a LocalLogRuntime,
    inner: MutexGuard<'a, RuntimeState>,
    world_id: WorldId,
    tail_start: u64,
}

impl Deref for LocalKernelGuard<'_> {
    type Target = Kernel<FsCas>;

    fn deref(&self) -> &Self::Target {
        self.inner
            .worlds
            .get(&self.world_id)
            .expect("world exists while kernel guard is held")
            .host
            .kernel()
    }
}

impl DerefMut for LocalKernelGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner
            .worlds
            .get_mut(&self.world_id)
            .expect("world exists while kernel guard is held")
            .host
            .kernel_mut()
    }
}

impl Drop for LocalKernelGuard<'_> {
    fn drop(&mut self) {
        if let Err(err) =
            self.runtime
                .persist_world_tail_locked(&mut self.inner, self.world_id, self.tail_start)
        {
            panic!(
                "persist embedded kernel mutation for {}: {err}",
                self.world_id
            );
        }
    }
}

pub struct LocalLogRuntime {
    paths: LocalStatePaths,
    cas: Arc<FsCas>,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
    inner: Mutex<RuntimeState>,
}

impl LocalLogRuntime {
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

    pub fn open_with_config(
        paths: LocalStatePaths,
        world_config: WorldConfig,
        adapter_config: EffectAdapterConfig,
        kernel_config: KernelConfig,
    ) -> Result<Arc<Self>, LocalRuntimeError> {
        paths.ensure_root()?;
        std::fs::create_dir_all(paths.cache_root())?;
        std::fs::create_dir_all(paths.run_dir())?;
        std::fs::create_dir_all(paths.logs_dir())?;
        let blob_planes = LocalBlobPlanes::from_paths(&paths)?;
        let cas = blob_planes.cas();
        let sqlite = LocalSqlitePlanes::from_paths(&paths)?;
        let (next_submission_seq, next_frame_offset) = sqlite.load_runtime_meta()?;
        let mut inner = RuntimeState {
            sqlite,
            next_submission_seq,
            next_frame_offset,
            worlds: BTreeMap::new(),
        };
        load_hot_worlds(
            &paths,
            &mut inner,
            cas.clone(),
            world_config.clone(),
            adapter_config.clone(),
            kernel_config.clone(),
        )?;
        let runtime = Arc::new(Self {
            paths,
            cas,
            world_config,
            adapter_config,
            kernel_config,
            inner: Mutex::new(inner),
        });
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
        let submission = SubmissionEnvelope::create_world(
            format!("create-world-{world_id}"),
            local_submission_universe_id(),
            world_id,
            request,
        );
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        self.process_submission_locked(&mut inner, submission)?;
        Ok(WorldCreateResult {
            record: self.world_record_locked(&inner, world_id)?,
        })
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
        Ok(WorldCreateResult {
            record: self.world_record_locked(&inner, world_id)?,
        })
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

    pub fn build_event_submission(
        &self,
        world_id: WorldId,
        ingress: DomainEventIngress,
    ) -> Result<SubmissionEnvelope, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
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
            payload: SubmissionPayload::DomainEvent {
                schema: ingress.schema,
                value: ingress.value,
                key: ingress.key,
            },
        })
    }

    pub fn build_receipt_submission(
        &self,
        world_id: WorldId,
        ingress: ReceiptIngress,
    ) -> Result<SubmissionEnvelope, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
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
            payload: SubmissionPayload::EffectReceipt {
                intent_hash: ingress.intent_hash,
                adapter_id: ingress.adapter_id,
                status: ingress.status,
                payload: ingress.payload,
                cost_cents: ingress.cost_cents,
                signature: ingress.signature,
            },
        })
    }

    pub fn world_summary(
        &self,
        world_id: WorldId,
    ) -> Result<(WorldRuntimeInfo, crate::SnapshotRecord), LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        Ok((
            runtime_info_from_world(world),
            world.active_baseline.clone(),
        ))
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
        self.process_submission_locked(&mut inner, submission)?;
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
        let submission = self.build_event_submission(world_id, ingress)?;
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        let submit_seq = self.allocate_submission_seq_locked(&mut inner)?;
        self.process_submission_locked(&mut inner, submission)?;
        Ok(InboxSeq::from_u64(submit_seq))
    }

    pub fn enqueue_receipt(
        &self,
        world_id: WorldId,
        ingress: ReceiptIngress,
    ) -> Result<InboxSeq, LocalRuntimeError> {
        let submission = self.build_receipt_submission(world_id, ingress)?;
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        let submit_seq = self.allocate_submission_seq_locked(&mut inner)?;
        self.process_submission_locked(&mut inner, submission)?;
        Ok(InboxSeq::from_u64(submit_seq))
    }

    pub fn process_all_pending(&self) -> Result<(), LocalRuntimeError> {
        Ok(())
    }

    pub fn execute_submission(
        &self,
        submission: SubmissionEnvelope,
    ) -> Result<bool, LocalRuntimeError> {
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        self.process_submission_locked(&mut inner, submission)
    }

    pub fn kernel_mut(&self, world_id: WorldId) -> Result<LocalKernelGuard<'_>, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let tail_start = {
            let world = inner
                .worlds
                .get(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
            world.host.journal_bounds().next_seq
        };
        Ok(LocalKernelGuard {
            runtime: self,
            inner,
            world_id,
            tail_start,
        })
    }

    pub fn inspect_world_host<R>(
        &self,
        world_id: WorldId,
        inspect: impl FnOnce(&WorldHost<FsCas>) -> Result<R, HostError>,
    ) -> Result<R, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        Ok(inspect(&world.host)?)
    }

    pub fn mutate_world_host<R>(
        &self,
        world_id: WorldId,
        mutate: impl FnOnce(&mut WorldHost<FsCas>) -> Result<R, HostError>,
    ) -> Result<R, LocalRuntimeError> {
        let mut inner = self.inner.lock().expect("local runtime mutex poisoned");
        let tail_start = {
            let world = inner
                .worlds
                .get(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
            world.host.journal_bounds().next_seq
        };
        let result = {
            let world = inner
                .worlds
                .get_mut(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
            mutate(&mut world.host)?
        };
        self.persist_world_tail_locked(&mut inner, world_id, tail_start)?;
        Ok(result)
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
        Ok((
            command_id.clone(),
            SubmissionEnvelope::command(
                command_id,
                local_submission_universe_id(),
                world_id,
                world_epoch,
                ingress,
            ),
        ))
    }

    pub fn manifest(&self, world_id: WorldId) -> Result<ManifestResponse, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        let manifest_hash = world.host.kernel().manifest_hash();
        let loaded = ManifestLoader::load_from_hash(world.host.store(), manifest_hash)?;
        Ok(ManifestResponse {
            journal_head: world.host.heights().head,
            manifest_hash: manifest_hash.to_hex(),
            manifest: loaded.manifest,
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
            journal_head: world.host.heights().head,
            manifest_hash: world.host.kernel().manifest_hash().to_hex(),
            defs: world.host.list_defs(kinds.as_deref(), prefix.as_deref())?,
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
        Ok(DefGetResponse {
            journal_head: world.host.heights().head,
            manifest_hash: world.host.kernel().manifest_hash().to_hex(),
            def: world.host.get_def(name)?,
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
        let state = world.host.state(workflow, Some(&key_bytes));
        let state_hash = state.as_ref().map(|bytes| Hash::of_bytes(bytes).to_hex());
        let size = state.as_ref().map(|bytes| bytes.len() as u64).unwrap_or(0);
        let cell = state_hash.map(|state_hash| StateCellSummary {
            journal_head: world.host.heights().head,
            workflow: workflow.to_string(),
            key_hash: Hash::of_bytes(&key_bytes).as_bytes().to_vec(),
            key_bytes: key_bytes.clone(),
            state_hash,
            size,
            last_active_ns: 0,
        });
        Ok(StateGetResponse {
            journal_head: world.host.heights().head,
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
            .host
            .list_cells(workflow)?
            .into_iter()
            .map(|cell| StateCellSummary {
                journal_head: world.host.heights().head,
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
            journal_head: world.host.heights().head,
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
        Ok(trace_get(world.host.kernel(), query)?)
    }

    pub fn trace_summary(&self, world_id: WorldId) -> Result<serde_json::Value, LocalRuntimeError> {
        let inner = self.inner.lock().expect("local runtime mutex poisoned");
        let world = inner
            .worlds
            .get(&world_id)
            .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
        Ok(workflow_trace_summary_with_routes(
            world.host.kernel(),
            Some(world.host.effect_route_diagnostics()),
        )?)
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
        let history = world.host.state("sys/Workspace@1", Some(&key));
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
        let bounds = world.host.journal_bounds();
        Ok(HeadInfoResponse {
            journal_head: world.host.heights().head,
            retained_from: bounds.retained_from,
            manifest_hash: Some(world.host.kernel().manifest_hash().to_hex()),
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
        let bounds = world.host.journal_bounds();
        let from = from.max(bounds.retained_from);
        let entries = world.host.kernel().dump_journal_from(from)?;
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
        let bounds = world.host.journal_bounds();
        let from = from.max(bounds.retained_from);
        let entries = world.host.kernel().dump_journal_from(from)?;
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
            manifest_hash: world.host.kernel().manifest_hash().to_hex(),
            active_baseline: world.active_baseline.clone(),
            journal_head: world.host.heights().head,
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

    fn process_submission_locked(
        &self,
        inner: &mut RuntimeState,
        submission: SubmissionEnvelope,
    ) -> Result<bool, LocalRuntimeError> {
        if let SubmissionPayload::CreateWorld { request } = submission.payload.clone() {
            self.process_create_world_submission_locked(inner, submission, request)?;
            return Ok(true);
        }

        let mut command_updates = Vec::new();
        let mut pending_frame = None;
        let mut failure = None;

        {
            let world = inner.worlds.get_mut(&submission.world_id).ok_or_else(|| {
                LocalRuntimeError::Backend(format!("world {} not found", submission.world_id))
            })?;
            if submission.world_epoch != world.world_epoch {
                return Err(LocalRuntimeError::Backend(format!(
                    "world epoch mismatch: expected {}, got {}",
                    world.world_epoch, submission.world_epoch
                )));
            }

            if let SubmissionPayload::Command { command } = &submission.payload
                && let Some(record) = world.command_records.get_mut(&command.command_id)
            {
                record.status = CommandStatus::Running;
                record.started_at_ns = Some(record.started_at_ns.unwrap_or_else(now_wallclock_ns));
                record.finished_at_ns = None;
                record.error = None;
                command_updates.push(record.clone());
            }

            let tail_start = world.host.journal_bounds().next_seq;
            let result = match &submission.payload {
                SubmissionPayload::Command { command } => {
                    let payload = resolve_plane_cbor_payload(world.host.store(), &command.payload)?;
                    let outcome = run_command(world, command, &payload)?;
                    if let Some(record) = world.command_records.get_mut(&command.command_id) {
                        record.status = CommandStatus::Succeeded;
                        record.finished_at_ns = Some(now_wallclock_ns());
                        record.journal_height = Some(world.host.heights().head);
                        record.manifest_hash = Some(world.host.kernel().manifest_hash().to_hex());
                        record.result_payload = outcome.result_payload;
                        record.error = None;
                        command_updates.push(record.clone());
                    }
                    Ok::<(), LocalRuntimeError>(())
                }
                _ => {
                    let external = submission_payload_to_external_event(
                        world.host.store(),
                        &submission.payload,
                    )?;
                    world.host.enqueue_external(external)?;
                    world
                        .host
                        .drain()
                        .map(|_| ())
                        .map_err(LocalRuntimeError::from)
                }
            };

            if let Err(err) = result {
                if let SubmissionPayload::Command { command } = &submission.payload
                    && let Some(record) = world.command_records.get_mut(&command.command_id)
                {
                    record.status = CommandStatus::Failed;
                    record.finished_at_ns = Some(now_wallclock_ns());
                    record.journal_height = Some(world.host.heights().head);
                    record.manifest_hash = Some(world.host.kernel().manifest_hash().to_hex());
                    record.error = Some(CommandErrorBody {
                        code: "command_failed".into(),
                        message: err.to_string(),
                    });
                    command_updates.push(record.clone());
                }
                failure = Some(err);
            } else {
                pending_frame = self.build_pending_frame(world, submission.world_id, tail_start)?;
                if pending_frame.is_none()
                    && let SubmissionPayload::Command { command } = &submission.payload
                    && let Some(record) = world.command_records.get_mut(&command.command_id)
                {
                    record.journal_height = Some(world.host.heights().head);
                    record.manifest_hash = Some(world.host.kernel().manifest_hash().to_hex());
                    command_updates.push(record.clone());
                }
            }
        }

        for record in &command_updates {
            inner
                .sqlite
                .persist_command_projection(submission.world_id, record)?;
        }
        if let Some(err) = failure {
            return Err(err);
        }

        if let Some(frame) = pending_frame {
            self.append_frame_locked(inner, submission.world_id, frame)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn process_create_world_submission_locked(
        &self,
        inner: &mut RuntimeState,
        submission: SubmissionEnvelope,
        request: CreateWorldRequest,
    ) -> Result<(), LocalRuntimeError> {
        self.create_world_from_request_locked(inner, submission.world_id, request)
    }

    fn create_world_from_request_locked(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
        request: CreateWorldRequest,
    ) -> Result<(), LocalRuntimeError> {
        let request = localize_create_request(request);
        if inner.worlds.contains_key(&world_id) {
            return Err(LocalRuntimeError::Backend(format!(
                "world {world_id} already exists"
            )));
        }
        let created = create_plane_world_from_request(
            self.cas.clone(),
            &request,
            local_submission_universe_id(),
            world_id,
            1,
            self.world_config.clone(),
            self.adapter_config.clone(),
            self.kernel_config_for_create_request(&request)?,
        )?;
        let world = HotWorld {
            world_id,
            universe_id: request.universe_id,
            created_at_ns: request.created_at_ns,
            initial_manifest_hash: created.initial_manifest_hash,
            world_epoch: 1,
            active_baseline: created.active_baseline,
            next_world_seq: 0,
            host: created.host,
            command_records: BTreeMap::new(),
        };
        inner.worlds.insert(world_id, world);
        let world = inner.worlds.get(&world_id).expect("world inserted");
        inner.sqlite.persist_world_directory(
            world_id,
            world.universe_id,
            world.created_at_ns,
            &world.initial_manifest_hash,
            world.world_epoch,
        )?;
        if let Some(frame) = created.initial_frame {
            self.append_frame_locked(inner, world_id, frame)?;
        } else {
            inner.sqlite.persist_checkpoint_head(
                world_id,
                &world.active_baseline,
                world.next_world_seq,
            )?;
        }
        Ok(())
    }

    fn create_fork_seed_request_locked(
        &self,
        inner: &mut RuntimeState,
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
        )?;
        world
            .host
            .compact_journal_through(world.active_baseline.height)?;
        Ok(())
    }

    fn build_pending_frame(
        &self,
        world: &HotWorld,
        world_id: WorldId,
        tail_start: u64,
    ) -> Result<Option<WorldLogFrame>, LocalRuntimeError> {
        let tail = world.host.kernel().dump_journal_from(tail_start)?;
        if tail.is_empty() {
            return Ok(None);
        }
        let mut records = Vec::with_capacity(tail.len());
        for entry in &tail {
            records.push(serde_cbor::from_slice::<JournalRecord>(&entry.payload)?);
        }
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

    fn persist_world_tail_locked(
        &self,
        inner: &mut RuntimeState,
        world_id: WorldId,
        tail_start: u64,
    ) -> Result<bool, LocalRuntimeError> {
        let pending_frame = {
            let world = inner
                .worlds
                .get(&world_id)
                .ok_or_else(|| LocalRuntimeError::Backend(format!("world {world_id} not found")))?;
            self.build_pending_frame(world, world_id, tail_start)?
        };
        if let Some(frame) = pending_frame {
            self.append_frame_locked(inner, world_id, frame)?;
            return Ok(true);
        }
        Ok(false)
    }
}

#[derive(Default)]
struct CommandOutcome {
    result_payload: Option<CborPayload>,
}

fn load_hot_worlds(
    paths: &LocalStatePaths,
    inner: &mut RuntimeState,
    cas: Arc<FsCas>,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
) -> Result<(), LocalRuntimeError> {
    for (directory, checkpoint) in inner.sqlite.load_world_directory()? {
        debug_assert_eq!(directory.world_id, checkpoint.world_id);
        let loaded = ManifestLoader::load_from_hash(
            cas.as_ref(),
            parse_plane_hash_like(&directory.initial_manifest_hash, "manifest_hash")?,
        )?;
        let frames = inner.sqlite.load_frame_log_for_world(directory.world_id)?;
        let host = reopen_world_from_frame_log(
            paths,
            cas.clone(),
            loaded,
            &checkpoint.active_baseline,
            &frames,
            world_config.clone(),
            adapter_config.clone(),
            kernel_config.clone(),
        )?;
        let command_records = inner.sqlite.load_command_projection(directory.world_id)?;
        inner.worlds.insert(
            directory.world_id,
            HotWorld {
                world_id: directory.world_id,
                universe_id: directory.universe_id,
                created_at_ns: directory.created_at_ns,
                initial_manifest_hash: directory.initial_manifest_hash,
                world_epoch: directory.world_epoch,
                active_baseline: checkpoint.active_baseline,
                next_world_seq: checkpoint.next_world_seq,
                host,
                command_records,
            },
        );
    }
    Ok(())
}

fn reopen_world_from_frame_log(
    paths: &LocalStatePaths,
    store: Arc<FsCas>,
    loaded: LoadedManifest,
    active_baseline: &crate::SnapshotRecord,
    frames: &[WorldLogFrame],
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
) -> Result<WorldHost<FsCas>, LocalRuntimeError> {
    let mut kernel_config = kernel_config;
    if kernel_config.secret_resolver.is_none() {
        if let Some(resolver) = local_secret_resolver_for_manifest(paths, &loaded)? {
            kernel_config.secret_resolver = Some(resolver);
        }
    }
    let baseline = crate::PromotableBaselineRef {
        snapshot_ref: active_baseline.snapshot_ref.clone(),
        snapshot_manifest_ref: None,
        manifest_hash: active_baseline.manifest_hash.clone().unwrap_or_default(),
        height: active_baseline.height,
        universe_id: active_baseline.universe_id,
        logical_time_ns: active_baseline.logical_time_ns,
        receipt_horizon_height: active_baseline
            .receipt_horizon_height
            .unwrap_or(active_baseline.height),
    };
    match open_plane_world_from_checkpoint(
        store.clone(),
        loaded.clone(),
        &baseline,
        frames,
        world_config.clone(),
        adapter_config.clone(),
        kernel_config.clone(),
    ) {
        Ok(host) => Ok(host),
        Err(_) => Ok(crate::open_plane_world_from_frames(
            store,
            loaded,
            frames,
            world_config,
            adapter_config,
            kernel_config,
        )?),
    }
}

fn runtime_info_from_world(world: &HotWorld) -> WorldRuntimeInfo {
    WorldRuntimeInfo {
        world_id: world.world_id,
        universe_id: world.universe_id,
        created_at_ns: world.created_at_ns,
        manifest_hash: Some(world.host.kernel().manifest_hash().to_hex()),
        active_baseline_height: Some(world.active_baseline.height),
        notify_counter: world.next_world_seq,
        has_pending_inbox: false,
        has_pending_effects: world.host.has_pending_effects(),
        next_timer_due_at_ns: None,
        has_pending_maintenance: false,
    }
}

fn local_submission_universe_id() -> UniverseId {
    UniverseId::from(Uuid::nil())
}

fn localize_create_request(mut request: CreateWorldRequest) -> CreateWorldRequest {
    request.universe_id = crate::UniverseId::nil();
    request
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

fn run_command(
    world: &mut HotWorld,
    control: &CommandIngress,
    payload: &[u8],
) -> Result<CommandOutcome, LocalRuntimeError> {
    match control.command.as_str() {
        "gov-propose" | "gov-shadow" | "gov-approve" | "gov-apply" => {
            run_governance_plane_command(&mut world.host, control, payload)?;
            Ok(CommandOutcome::default())
        }
        other => Err(LocalRuntimeError::Backend(format!(
            "unsupported log-first command submission '{other}'"
        ))),
    }
}
