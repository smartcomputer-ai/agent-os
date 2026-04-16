use std::sync::{Arc, mpsc as std_mpsc};
use std::thread;

use crate::api::http::{HttpBackend, decode_b64};
use crate::api::{
    BlobPutResponse, CasBlobMetadata, CommandSubmitBody, CommandSubmitResponse, ControlError,
    CreateWorldBody, DefGetResponse, DefsListResponse, DefsQuery, ForkWorldBody, HeadInfoResponse,
    LimitQuery, ManifestResponse, PutSecretVersionBody, ServiceInfoResponse, StateGetQuery,
    StateGetResponse, StateListResponse, SubmitEventBody, UpsertSecretBindingBody,
    WorkspaceAnnotationsQuery, WorkspaceApplyRequest, WorkspaceApplyResponse, WorkspaceBytesQuery,
    WorkspaceDiffBody, WorkspaceEntriesQuery, WorkspaceEntryQuery, WorkspaceResolveQuery,
    WorkspaceResolveResponse, WorldSummaryResponse,
};
use crate::{
    CommandRecord, CreateWorldRequest, DomainEventIngress, ForkWorldRequest, ReceiptIngress,
    WorldId, WorldRuntimeInfo,
};
use aos_air_types::AirNode;
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effect_types::{
    GovApplyParams, GovApproveParams, GovProposeParams, GovShadowParams, HashRef,
    WorkspaceAnnotationsGetReceipt, WorkspaceDiffReceipt, WorkspaceListReceipt,
    WorkspaceReadRefReceipt,
};
use serde::Serialize;

use super::{LocalRuntime, LocalStatePaths};

type SchedulerJob = Box<dyn FnOnce(&Arc<LocalRuntime>) + Send + 'static>;

enum SchedulerCommand {
    Run(SchedulerJob),
}

#[derive(Clone)]
enum LocalControlMode {
    Direct(Arc<LocalRuntime>),
    Server(LocalServerControl),
}

#[derive(Clone)]
struct LocalServerControl {
    runtime: Arc<LocalRuntime>,
    scheduler_tx: std_mpsc::Sender<SchedulerCommand>,
}

#[derive(Clone)]
pub struct LocalControl {
    mode: LocalControlMode,
}

impl LocalControl {
    pub const WORKER_ID: &str = "local";

    pub fn open(state_root: &std::path::Path) -> Result<Arc<Self>, ControlError> {
        let paths = LocalStatePaths::new(state_root.to_path_buf());
        let runtime = LocalRuntime::open(paths)?;
        Self::open_server_with_runtime(runtime)
    }

    pub fn open_with_handle(
        state_root: &std::path::Path,
        edge_handle: tokio::runtime::Handle,
    ) -> Result<Arc<Self>, ControlError> {
        let paths = LocalStatePaths::new(state_root.to_path_buf());
        let runtime = LocalRuntime::open_with_handle(paths, edge_handle)?;
        Self::open_server_with_runtime(runtime)
    }

    pub fn open_batch(state_root: &std::path::Path) -> Result<Arc<Self>, ControlError> {
        let paths = LocalStatePaths::new(state_root.to_path_buf());
        let runtime = LocalRuntime::open(paths)?;
        Ok(Arc::new(Self {
            mode: LocalControlMode::Direct(runtime),
        }))
    }

    fn open_server_with_runtime(runtime: Arc<LocalRuntime>) -> Result<Arc<Self>, ControlError> {
        let (scheduler_tx, scheduler_rx) = std_mpsc::channel::<SchedulerCommand>();

        {
            let runtime = Arc::clone(&runtime);
            thread::Builder::new()
                .name("aos-local-scheduler".into())
                .spawn(move || {
                    while let Ok(command) = scheduler_rx.recv() {
                        match command {
                            SchedulerCommand::Run(job) => job(&runtime),
                        }
                    }
                })
                .map_err(|err| {
                    ControlError::invalid(format!("spawn local scheduler thread: {err}"))
                })?;
        }

        if let Some(effect_rx) = runtime.take_effect_event_rx() {
            let scheduler_tx = scheduler_tx.clone();
            thread::Builder::new()
                .name("aos-local-effect-bridge".into())
                .spawn(move || {
                    let mut effect_rx = effect_rx;
                    while let Some(event) = effect_rx.blocking_recv() {
                        if scheduler_tx
                            .send(SchedulerCommand::Run(Box::new(move |runtime| {
                                if let Err(err) = runtime.enqueue_effect_runtime_event(event) {
                                    tracing::error!(error = %err, "enqueue local effect continuation");
                                    return;
                                }
                                if let Err(err) = runtime.process_all_pending() {
                                    tracing::error!(error = %err, "process local effect continuation");
                                }
                            })))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .map_err(|err| {
                    ControlError::invalid(format!("spawn local effect bridge thread: {err}"))
                })?;
        }

        if let Some(timer_rx) = runtime.take_timer_wake_rx() {
            let scheduler_tx = scheduler_tx.clone();
            thread::Builder::new()
                .name("aos-local-timer-bridge".into())
                .spawn(move || {
                    let mut timer_rx = timer_rx;
                    while let Some(wake) = timer_rx.blocking_recv() {
                        if scheduler_tx
                            .send(SchedulerCommand::Run(Box::new(move |runtime| {
                                if let Err(err) = runtime.process_timer_wake(wake.world_id) {
                                    tracing::error!(error = %err, world_id = %wake.world_id, "enqueue local timer wake");
                                    return;
                                }
                                if let Err(err) = runtime.process_all_pending() {
                                    tracing::error!(error = %err, world_id = %wake.world_id, "process local timer wake");
                                }
                            })))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .map_err(|err| {
                    ControlError::invalid(format!("spawn local timer bridge thread: {err}"))
                })?;
        }

        let checkpoint_interval = runtime.checkpoint_interval();
        if !checkpoint_interval.is_zero() {
            let scheduler_tx = scheduler_tx.clone();
            thread::Builder::new()
                .name("aos-local-maintenance".into())
                .spawn(move || {
                    loop {
                        thread::sleep(checkpoint_interval);
                        if scheduler_tx
                            .send(SchedulerCommand::Run(Box::new(|runtime| {
                                if let Err(err) = runtime.process_all_pending() {
                                    tracing::error!(error = %err, "process local maintenance tick");
                                }
                            })))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .map_err(|err| {
                    ControlError::invalid(format!("spawn local maintenance thread: {err}"))
                })?;
        }

        Ok(Arc::new(Self {
            mode: LocalControlMode::Server(LocalServerControl {
                runtime,
                scheduler_tx,
            }),
        }))
    }

    fn local_runtime(&self) -> &Arc<LocalRuntime> {
        match &self.mode {
            LocalControlMode::Direct(runtime) => runtime,
            LocalControlMode::Server(server) => &server.runtime,
        }
    }

    fn server_call<T, F>(&self, f: F) -> Result<T, ControlError>
    where
        T: Send + 'static,
        F: FnOnce(&Arc<LocalRuntime>) -> Result<T, ControlError> + Send + 'static,
    {
        let LocalControlMode::Server(server) = &self.mode else {
            return f(self.local_runtime());
        };
        let (reply_tx, reply_rx) = std_mpsc::sync_channel(1);
        server
            .scheduler_tx
            .send(SchedulerCommand::Run(Box::new(move |runtime| {
                let _ = reply_tx.send(f(runtime));
            })))
            .map_err(|_| ControlError::invalid("local scheduler is not available"))?;
        reply_rx
            .recv()
            .map_err(|_| ControlError::invalid("local scheduler did not reply"))?
    }

    pub fn step_world(&self, world: WorldId) -> Result<WorldSummaryResponse, ControlError> {
        self.server_call(move |runtime| {
            runtime.process_all_pending().map_err(ControlError::from)?;
            let (runtime_info, active_baseline) = runtime.world_summary(world)?;
            Ok(WorldSummaryResponse {
                runtime: runtime_info,
                active_baseline,
            })
        })
    }

    pub fn workers(&self, limit: u32) -> Result<Vec<crate::WorkerHeartbeat>, ControlError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        Ok(vec![crate::WorkerHeartbeat {
            worker_id: Self::WORKER_ID.to_string(),
            pins: Vec::new(),
            last_seen_ns: 0,
            expires_at_ns: u64::MAX,
        }])
    }

    pub fn worker_worlds(
        &self,
        worker_id: &str,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, ControlError> {
        if worker_id != Self::WORKER_ID {
            return Ok(Vec::new());
        }
        self.server_call(move |runtime| runtime.worker_worlds(limit).map_err(ControlError::from))
    }

    pub fn health(&self) -> Result<ServiceInfoResponse, ControlError> {
        let state_root = match &self.mode {
            LocalControlMode::Direct(runtime) => runtime.state_root().to_path_buf(),
            LocalControlMode::Server(server) => server.runtime.state_root().to_path_buf(),
        };
        Ok(ServiceInfoResponse {
            service: "aos-node-local",
            version: env!("CARGO_PKG_VERSION"),
            pid: Some(std::process::id()),
            state_root: Some(state_root),
        })
    }

    pub fn list_worlds(
        &self,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, ControlError> {
        self.server_call(move |runtime| {
            runtime
                .list_worlds(after, limit)
                .map_err(ControlError::from)
        })
    }

    pub fn create_world(
        &self,
        request: CreateWorldRequest,
    ) -> Result<crate::WorldCreateResult, ControlError> {
        self.server_call(move |runtime| runtime.create_world(request).map_err(ControlError::from))
    }

    pub fn get_world(&self, world: WorldId) -> Result<WorldSummaryResponse, ControlError> {
        self.server_call(move |runtime| {
            let (runtime_info, active_baseline) = runtime.world_summary(world)?;
            Ok(WorldSummaryResponse {
                runtime: runtime_info,
                active_baseline,
            })
        })
    }

    pub fn checkpoint_world(&self, world: WorldId) -> Result<WorldSummaryResponse, ControlError> {
        self.server_call(move |runtime| {
            let (runtime_info, active_baseline) = runtime.checkpoint_world(world)?;
            Ok(WorldSummaryResponse {
                runtime: runtime_info,
                active_baseline,
            })
        })
    }

    pub fn fork_world(
        &self,
        request: ForkWorldRequest,
    ) -> Result<crate::WorldForkResult, ControlError> {
        self.server_call(move |runtime| runtime.fork_world(request).map_err(ControlError::from))
    }

    pub fn get_command(
        &self,
        world: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, ControlError> {
        let command_id = command_id.to_string();
        self.server_call(move |runtime| {
            runtime
                .get_command(world, &command_id)
                .map_err(ControlError::from)
        })
    }

    pub fn submit_command<T: Serialize>(
        &self,
        world: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        payload: &T,
    ) -> Result<CommandSubmitResponse, ControlError> {
        let command = command.to_string();
        let payload = to_canonical_cbor(payload)?;
        let record = self.server_call(move |runtime| {
            let payload: serde_cbor::Value = serde_cbor::from_slice(&payload)?;
            runtime
                .submit_command(world, &command, command_id, actor, &payload)
                .map_err(ControlError::from)
        })?;
        Ok(CommandSubmitResponse {
            poll_url: format!("/v1/worlds/{world}/commands/{}", record.command_id),
            command_id: record.command_id,
            status: record.status,
        })
    }

    pub fn manifest(&self, world: WorldId) -> Result<ManifestResponse, ControlError> {
        self.server_call(move |runtime| runtime.manifest(world).map_err(ControlError::from))
    }

    pub fn defs_list(
        &self,
        world: WorldId,
        kinds: Option<Vec<String>>,
        prefix: Option<String>,
    ) -> Result<DefsListResponse, ControlError> {
        self.server_call(move |runtime| {
            runtime
                .defs_list(world, kinds, prefix)
                .map_err(ControlError::from)
        })
    }

    pub fn def_get(
        &self,
        world: WorldId,
        kind: &str,
        name: &str,
    ) -> Result<DefGetResponse, ControlError> {
        let name = name.to_string();
        let lookup_name = name.clone();
        let response = self.server_call(move |runtime| {
            runtime
                .def_get(world, &lookup_name)
                .map_err(ControlError::from)
        })?;
        if !def_matches_kind(&response.def, kind) {
            return Err(ControlError::not_found(format!(
                "definition '{name}' with kind '{kind}'"
            )));
        }
        Ok(response)
    }

    pub fn state_get(
        &self,
        world: WorldId,
        workflow: &str,
        key: Option<Vec<u8>>,
        consistency: Option<&str>,
    ) -> Result<StateGetResponse, ControlError> {
        require_latest_durable(consistency)?;
        let workflow = workflow.to_string();
        self.server_call(move |runtime| {
            runtime
                .state_get(world, &workflow, key)
                .map_err(ControlError::from)
        })
    }

    pub fn state_list(
        &self,
        world: WorldId,
        workflow: &str,
        limit: u32,
        consistency: Option<&str>,
    ) -> Result<StateListResponse, ControlError> {
        require_latest_durable(consistency)?;
        let workflow = workflow.to_string();
        self.server_call(move |runtime| {
            runtime
                .state_list(world, &workflow, limit)
                .map_err(ControlError::from)
        })
    }

    pub fn enqueue_event(
        &self,
        world: WorldId,
        ingress: DomainEventIngress,
    ) -> Result<crate::InboxSeq, ControlError> {
        self.server_call(move |runtime| {
            let seq = runtime
                .enqueue_event(world, ingress)
                .map_err(ControlError::from)?;
            runtime.process_all_pending().map_err(ControlError::from)?;
            Ok(seq)
        })
    }

    pub fn enqueue_receipt(
        &self,
        world: WorldId,
        ingress: ReceiptIngress,
    ) -> Result<crate::InboxSeq, ControlError> {
        self.server_call(move |runtime| {
            let seq = runtime
                .enqueue_receipt(world, ingress)
                .map_err(ControlError::from)?;
            runtime.process_all_pending().map_err(ControlError::from)?;
            Ok(seq)
        })
    }

    pub fn journal_head(&self, world: WorldId) -> Result<HeadInfoResponse, ControlError> {
        self.server_call(move |runtime| runtime.journal_head(world).map_err(ControlError::from))
    }

    pub fn journal_entries(
        &self,
        world: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<crate::api::JournalEntriesResponse, ControlError> {
        self.server_call(move |runtime| {
            runtime
                .journal_entries(world, from, limit)
                .map_err(ControlError::from)
        })
    }

    pub fn journal_entries_raw(
        &self,
        world: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<crate::api::RawJournalEntriesResponse, ControlError> {
        self.server_call(move |runtime| {
            runtime
                .journal_entries_raw(world, from, limit)
                .map_err(ControlError::from)
        })
    }

    pub fn runtime(&self, world: WorldId) -> Result<WorldRuntimeInfo, ControlError> {
        self.server_call(move |runtime| runtime.world_runtime(world).map_err(ControlError::from))
    }

    pub fn trace(
        &self,
        world: WorldId,
        event_hash: Option<&str>,
        schema: Option<&str>,
        correlate_by: Option<&str>,
        correlate_value: Option<serde_json::Value>,
        window_limit: Option<u64>,
    ) -> Result<serde_json::Value, ControlError> {
        let event_hash = event_hash.map(ToOwned::to_owned);
        let schema = schema.map(ToOwned::to_owned);
        let correlate_by = correlate_by.map(ToOwned::to_owned);
        self.server_call(move |runtime| {
            runtime
                .trace(
                    world,
                    aos_kernel::TraceQuery {
                        event_hash,
                        schema,
                        correlate_by,
                        correlate_value,
                        window_limit,
                    },
                )
                .map_err(ControlError::from)
        })
    }

    pub fn trace_summary(
        &self,
        world: WorldId,
        _recent_limit: u32,
    ) -> Result<serde_json::Value, ControlError> {
        self.server_call(move |runtime| runtime.trace_summary(world).map_err(ControlError::from))
    }

    pub fn workspace_resolve(
        &self,
        world: WorldId,
        workspace_name: &str,
        version: Option<u64>,
    ) -> Result<WorkspaceResolveResponse, ControlError> {
        let workspace_name = workspace_name.to_string();
        self.server_call(move |runtime| {
            runtime
                .workspace_resolve(world, &workspace_name, version)
                .map_err(ControlError::from)
        })
    }

    pub fn workspace_empty_root(&self) -> Result<HashRef, ControlError> {
        self.local_runtime()
            .workspace_empty_root()
            .map_err(ControlError::from)
    }

    pub fn workspace_entries(
        &self,
        root_hash: &HashRef,
        path: Option<&str>,
        scope: Option<&str>,
        cursor: Option<&str>,
        limit: u64,
    ) -> Result<WorkspaceListReceipt, ControlError> {
        self.local_runtime()
            .workspace_entries(root_hash, path, scope, cursor, limit)
            .map_err(ControlError::from)
    }

    pub fn workspace_entry(
        &self,
        root_hash: &HashRef,
        path: &str,
    ) -> Result<WorkspaceReadRefReceipt, ControlError> {
        self.local_runtime()
            .workspace_entry(root_hash, path)
            .map_err(ControlError::from)
    }

    pub fn workspace_bytes(
        &self,
        root_hash: &HashRef,
        path: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Vec<u8>, ControlError> {
        self.local_runtime()
            .workspace_bytes(root_hash, path, range)
            .map_err(ControlError::from)
    }

    pub fn workspace_annotations(
        &self,
        root_hash: &HashRef,
        path: Option<&str>,
    ) -> Result<WorkspaceAnnotationsGetReceipt, ControlError> {
        self.local_runtime()
            .workspace_annotations(root_hash, path)
            .map_err(ControlError::from)
    }

    pub fn workspace_apply(
        &self,
        root_hash: HashRef,
        request: WorkspaceApplyRequest,
    ) -> Result<WorkspaceApplyResponse, ControlError> {
        self.local_runtime()
            .workspace_apply(root_hash, request)
            .map_err(ControlError::from)
    }

    pub fn workspace_diff(
        &self,
        root_a: &HashRef,
        root_b: &HashRef,
        prefix: Option<&str>,
    ) -> Result<WorkspaceDiffReceipt, ControlError> {
        self.local_runtime()
            .workspace_diff(root_a, root_b, prefix)
            .map_err(ControlError::from)
    }

    pub fn put_blob(
        &self,
        bytes: &[u8],
        expected_hash: Option<Hash>,
    ) -> Result<BlobPutResponse, ControlError> {
        let hash = self.local_runtime().put_blob(bytes)?;
        if let Some(expected_hash) = expected_hash
            && expected_hash != hash
        {
            return Err(ControlError::invalid(format!(
                "blob hash mismatch: expected {}, got {}",
                expected_hash.to_hex(),
                hash.to_hex()
            )));
        }
        Ok(BlobPutResponse {
            hash: hash.to_hex(),
        })
    }

    pub fn head_blob(&self, hash: Hash) -> Result<CasBlobMetadata, ControlError> {
        Ok(CasBlobMetadata {
            hash: hash.to_hex(),
            exists: self.local_runtime().blob_metadata(hash)?,
        })
    }

    pub fn get_blob(&self, hash: Hash) -> Result<Vec<u8>, ControlError> {
        self.local_runtime()
            .get_blob(hash)
            .map_err(ControlError::from)
    }
}

impl HttpBackend for LocalControl {
    type CreateWorldResponse = crate::WorldCreateResult;
    type ForkWorldResponse = crate::WorldForkResult;
    type SubmitEventResponse = serde_json::Value;
    type SubmitReceiptResponse = serde_json::Value;
    type WorkspaceEntryResponse = WorkspaceReadRefReceipt;

    fn health(&self) -> Result<ServiceInfoResponse, ControlError> {
        LocalControl::health(self)
    }

    fn list_worlds(
        &self,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, ControlError> {
        LocalControl::list_worlds(self, after, limit)
    }

    fn create_world(
        &self,
        body: CreateWorldBody,
    ) -> Result<Self::CreateWorldResponse, ControlError> {
        LocalControl::create_world(
            self,
            CreateWorldRequest {
                world_id: body.world_id,
                universe_id: body.universe_id,
                created_at_ns: body.created_at_ns,
                source: body.source,
            },
        )
    }

    fn get_world(&self, world_id: WorldId) -> Result<WorldSummaryResponse, ControlError> {
        LocalControl::get_world(self, world_id)
    }

    fn checkpoint_world(&self, world_id: WorldId) -> Result<WorldSummaryResponse, ControlError> {
        LocalControl::checkpoint_world(self, world_id)
    }

    fn fork_world(
        &self,
        src_world_id: WorldId,
        body: ForkWorldBody,
    ) -> Result<Self::ForkWorldResponse, ControlError> {
        LocalControl::fork_world(
            self,
            ForkWorldRequest {
                src_world_id,
                src_snapshot: body.src_snapshot,
                new_world_id: body.new_world_id,
                forked_at_ns: body.forked_at_ns,
                pending_effect_policy: body.pending_effect_policy,
            },
        )
    }

    fn manifest(&self, world_id: WorldId) -> Result<ManifestResponse, ControlError> {
        LocalControl::manifest(self, world_id)
    }

    fn defs_list(
        &self,
        world_id: WorldId,
        query: DefsQuery,
    ) -> Result<DefsListResponse, ControlError> {
        LocalControl::defs_list(
            self,
            world_id,
            query
                .kinds
                .map(|kinds| kinds.split(',').map(str::to_owned).collect()),
            query.prefix,
        )
    }

    fn def_get(
        &self,
        world_id: WorldId,
        kind: &str,
        name: &str,
    ) -> Result<DefGetResponse, ControlError> {
        LocalControl::def_get(self, world_id, kind, name)
    }

    fn runtime(&self, world_id: WorldId) -> Result<WorldRuntimeInfo, ControlError> {
        LocalControl::runtime(self, world_id)
    }

    fn trace(
        &self,
        world_id: WorldId,
        event_hash: Option<&str>,
        schema: Option<&str>,
        correlate_by: Option<&str>,
        correlate_value: Option<serde_json::Value>,
        window_limit: Option<u64>,
    ) -> Result<serde_json::Value, ControlError> {
        LocalControl::trace(
            self,
            world_id,
            event_hash,
            schema,
            correlate_by,
            correlate_value,
            window_limit,
        )
    }

    fn trace_summary(
        &self,
        world_id: WorldId,
        recent_limit: u32,
    ) -> Result<serde_json::Value, ControlError> {
        LocalControl::trace_summary(self, world_id, recent_limit)
    }

    fn journal_head(&self, world_id: WorldId) -> Result<HeadInfoResponse, ControlError> {
        LocalControl::journal_head(self, world_id)
    }

    fn journal_entries(
        &self,
        world_id: WorldId,
        query: crate::api::JournalQuery,
    ) -> Result<crate::api::JournalEntriesResponse, ControlError> {
        LocalControl::journal_entries(self, world_id, query.from, query.limit)
    }

    fn journal_entries_raw(
        &self,
        world_id: WorldId,
        query: crate::api::JournalQuery,
    ) -> Result<crate::api::RawJournalEntriesResponse, ControlError> {
        LocalControl::journal_entries_raw(self, world_id, query.from, query.limit)
    }

    fn get_command(
        &self,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, ControlError> {
        LocalControl::get_command(self, world_id, command_id)
    }

    fn state_get(
        &self,
        world_id: WorldId,
        workflow: &str,
        query: StateGetQuery,
    ) -> Result<StateGetResponse, ControlError> {
        LocalControl::state_get(
            self,
            world_id,
            workflow,
            query.key_b64.as_deref().map(decode_b64).transpose()?,
            query.consistency.as_deref(),
        )
    }

    fn state_list(
        &self,
        world_id: WorldId,
        workflow: &str,
        query: LimitQuery,
    ) -> Result<StateListResponse, ControlError> {
        LocalControl::state_list(
            self,
            world_id,
            workflow,
            query.limit,
            query.consistency.as_deref(),
        )
    }

    fn workspace_resolve(
        &self,
        world_id: WorldId,
        query: WorkspaceResolveQuery,
    ) -> Result<WorkspaceResolveResponse, ControlError> {
        LocalControl::workspace_resolve(self, world_id, &query.workspace, query.version)
    }

    fn submit_event(
        &self,
        world_id: WorldId,
        body: SubmitEventBody,
    ) -> Result<Self::SubmitEventResponse, ControlError> {
        let value_bytes = if let Some(value_b64) = body.value_b64 {
            decode_b64(&value_b64)?
        } else {
            serde_cbor::to_vec(
                body.value
                    .as_ref()
                    .or(body.value_json.as_ref())
                    .ok_or_else(|| ControlError::invalid("missing event value"))?,
            )?
        };
        let seq = LocalControl::enqueue_event(
            self,
            world_id,
            crate::DomainEventIngress {
                schema: body.schema,
                value: crate::CborPayload::inline(value_bytes),
                key: body.key_b64.as_deref().map(decode_b64).transpose()?,
                correlation_id: body.correlation_id,
            },
        )?;
        Ok(serde_json::json!({ "inbox_seq": seq }))
    }

    fn submit_receipt(
        &self,
        world_id: WorldId,
        body: ReceiptIngress,
    ) -> Result<Self::SubmitReceiptResponse, ControlError> {
        body.payload.validate()?;
        let seq = LocalControl::enqueue_receipt(self, world_id, body)?;
        Ok(serde_json::json!({ "inbox_seq": seq }))
    }

    fn governance_propose(
        &self,
        world_id: WorldId,
        body: CommandSubmitBody<GovProposeParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        LocalControl::submit_command(
            self,
            world_id,
            "gov-propose",
            body.command_id,
            body.actor,
            &body.params,
        )
    }

    fn governance_shadow(
        &self,
        world_id: WorldId,
        body: CommandSubmitBody<GovShadowParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        LocalControl::submit_command(
            self,
            world_id,
            "gov-shadow",
            body.command_id,
            body.actor,
            &body.params,
        )
    }

    fn governance_approve(
        &self,
        world_id: WorldId,
        body: CommandSubmitBody<GovApproveParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        LocalControl::submit_command(
            self,
            world_id,
            "gov-approve",
            body.command_id,
            body.actor,
            &body.params,
        )
    }

    fn governance_apply(
        &self,
        world_id: WorldId,
        body: CommandSubmitBody<GovApplyParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        LocalControl::submit_command(
            self,
            world_id,
            "gov-apply",
            body.command_id,
            body.actor,
            &body.params,
        )
    }

    fn workspace_empty_root(
        &self,
        _universe_id: Option<crate::UniverseId>,
    ) -> Result<HashRef, ControlError> {
        LocalControl::workspace_empty_root(self)
    }

    fn workspace_entries(
        &self,
        _universe_id: Option<crate::UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceEntriesQuery,
    ) -> Result<WorkspaceListReceipt, ControlError> {
        LocalControl::workspace_entries(
            self,
            root_hash,
            query.path.as_deref(),
            query.scope.as_deref(),
            query.cursor.as_deref(),
            query.limit,
        )
    }

    fn workspace_entry(
        &self,
        _universe_id: Option<crate::UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceEntryQuery,
    ) -> Result<Self::WorkspaceEntryResponse, ControlError> {
        LocalControl::workspace_entry(self, root_hash, &query.path)
    }

    fn workspace_bytes(
        &self,
        _universe_id: Option<crate::UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceBytesQuery,
    ) -> Result<Vec<u8>, ControlError> {
        LocalControl::workspace_bytes(self, root_hash, &query.path, query.start.zip(query.end))
    }

    fn workspace_annotations(
        &self,
        _universe_id: Option<crate::UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceAnnotationsQuery,
    ) -> Result<WorkspaceAnnotationsGetReceipt, ControlError> {
        LocalControl::workspace_annotations(self, root_hash, query.path.as_deref())
    }

    fn workspace_apply(
        &self,
        _universe_id: Option<crate::UniverseId>,
        root_hash: HashRef,
        request: WorkspaceApplyRequest,
    ) -> Result<WorkspaceApplyResponse, ControlError> {
        LocalControl::workspace_apply(self, root_hash, request)
    }

    fn workspace_diff(
        &self,
        _universe_id: Option<crate::UniverseId>,
        body: WorkspaceDiffBody,
    ) -> Result<WorkspaceDiffReceipt, ControlError> {
        LocalControl::workspace_diff(self, &body.root_a, &body.root_b, body.prefix.as_deref())
    }

    fn put_blob(
        &self,
        bytes: &[u8],
        _universe_id: Option<crate::UniverseId>,
        expected_hash: Option<Hash>,
    ) -> Result<BlobPutResponse, ControlError> {
        LocalControl::put_blob(self, bytes, expected_hash)
    }

    fn head_blob(
        &self,
        _universe_id: Option<crate::UniverseId>,
        hash: Hash,
    ) -> Result<CasBlobMetadata, ControlError> {
        LocalControl::head_blob(self, hash)
    }

    fn get_blob(
        &self,
        _universe_id: Option<crate::UniverseId>,
        hash: Hash,
    ) -> Result<Vec<u8>, ControlError> {
        LocalControl::get_blob(self, hash)
    }

    fn list_secret_bindings(
        &self,
        _universe_id: Option<crate::UniverseId>,
    ) -> Result<Vec<crate::SecretBindingRecord>, ControlError> {
        Err(ControlError::not_implemented("local node secret vault"))
    }

    fn get_secret_binding(
        &self,
        _universe_id: Option<crate::UniverseId>,
        _binding_id: &str,
    ) -> Result<crate::SecretBindingRecord, ControlError> {
        Err(ControlError::not_implemented("local node secret vault"))
    }

    fn upsert_secret_binding(
        &self,
        _universe_id: Option<crate::UniverseId>,
        _binding_id: &str,
        _body: UpsertSecretBindingBody,
    ) -> Result<crate::SecretBindingRecord, ControlError> {
        Err(ControlError::not_implemented("local node secret vault"))
    }

    fn delete_secret_binding(
        &self,
        _universe_id: Option<crate::UniverseId>,
        _binding_id: &str,
    ) -> Result<crate::SecretBindingRecord, ControlError> {
        Err(ControlError::not_implemented("local node secret vault"))
    }

    fn list_secret_versions(
        &self,
        _universe_id: Option<crate::UniverseId>,
        _binding_id: &str,
    ) -> Result<Vec<crate::SecretVersionRecord>, ControlError> {
        Err(ControlError::not_implemented("local node secret vault"))
    }

    fn put_secret_version(
        &self,
        _universe_id: Option<crate::UniverseId>,
        _binding_id: &str,
        _body: PutSecretVersionBody,
    ) -> Result<crate::SecretVersionRecord, ControlError> {
        Err(ControlError::not_implemented("local node secret vault"))
    }

    fn get_secret_version(
        &self,
        _universe_id: Option<crate::UniverseId>,
        _binding_id: &str,
        _version: u64,
    ) -> Result<crate::SecretVersionRecord, ControlError> {
        Err(ControlError::not_implemented("local node secret vault"))
    }
}

fn require_latest_durable(consistency: Option<&str>) -> Result<(), ControlError> {
    match consistency {
        None | Some("latest") | Some("latest_durable") => Ok(()),
        Some(other) => Err(ControlError::invalid(format!(
            "unsupported consistency '{other}'"
        ))),
    }
}

fn def_matches_kind(def: &AirNode, kind: &str) -> bool {
    matches!(
        (def, kind),
        (AirNode::Defschema(_), "defschema")
            | (AirNode::Defmodule(_), "defmodule")
            | (AirNode::Defcap(_), "defcap")
            | (AirNode::Defpolicy(_), "defpolicy")
            | (AirNode::Defsecret(_), "defsecret")
            | (AirNode::Defeffect(_), "defeffect")
            | (AirNode::Manifest(_), "manifest")
    )
}
