use std::sync::Arc;

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
use aos_cbor::Hash;
use aos_effect_types::{
    GovApplyParams, GovApproveParams, GovProposeParams, GovShadowParams, HashRef,
    WorkspaceAnnotationsGetReceipt, WorkspaceDiffReceipt, WorkspaceListReceipt,
    WorkspaceReadRefReceipt,
};
use serde::Serialize;

use super::{
    LocalLogRuntime, LocalStatePaths, LocalSupervisor, LocalSupervisorConfig, def_matches_kind,
};

#[derive(Clone)]
pub struct LocalControl {
    runtime: Arc<LocalLogRuntime>,
    supervisor: Arc<LocalSupervisor>,
    mode: LocalExecutionMode,
}

#[derive(Clone, Copy)]
enum LocalExecutionMode {
    Worker,
    Direct,
}

impl LocalControl {
    pub const WORKER_ID: &str = "local";

    pub fn open(state_root: &std::path::Path) -> Result<Arc<Self>, ControlError> {
        Self::open_with_supervisor(state_root, true)
    }

    pub fn open_batch(state_root: &std::path::Path) -> Result<Arc<Self>, ControlError> {
        Self::open_with_supervisor(state_root, false)
    }

    fn open_with_supervisor(
        state_root: &std::path::Path,
        start_supervisor: bool,
    ) -> Result<Arc<Self>, ControlError> {
        let paths = LocalStatePaths::new(state_root.to_path_buf());
        let runtime = LocalLogRuntime::open(paths.clone())?;
        let supervisor = LocalSupervisor::new(runtime.clone(), LocalSupervisorConfig::default());
        if start_supervisor {
            supervisor.start();
        }
        Ok(Arc::new(Self {
            runtime,
            supervisor,
            mode: if start_supervisor {
                LocalExecutionMode::Worker
            } else {
                LocalExecutionMode::Direct
            },
        }))
    }

    pub fn step_world(&self, world: WorldId) -> Result<WorldSummaryResponse, ControlError> {
        if matches!(self.mode, LocalExecutionMode::Worker) {
            let _ = self.supervisor.run_once()?;
        }
        self.get_world(world)
    }

    pub fn workers(&self, limit: u32) -> Result<Vec<crate::WorkerHeartbeat>, ControlError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        Ok(vec![self.supervisor.worker_heartbeat(Self::WORKER_ID)])
    }

    pub fn worker_worlds(
        &self,
        worker_id: &str,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, ControlError> {
        self.supervisor
            .worker_worlds(worker_id, limit, Self::WORKER_ID)
            .map_err(ControlError::from)
    }

    pub fn health(&self) -> Result<ServiceInfoResponse, ControlError> {
        Ok(ServiceInfoResponse {
            service: "aos-node-local",
            version: env!("CARGO_PKG_VERSION"),
        })
    }

    pub fn list_worlds(
        &self,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, ControlError> {
        self.supervisor
            .list_worlds(after, limit)
            .map_err(ControlError::from)
    }

    pub fn create_world(
        &self,
        request: CreateWorldRequest,
    ) -> Result<crate::WorldCreateResult, ControlError> {
        self.runtime
            .create_world(request)
            .map_err(ControlError::from)
    }

    pub fn get_world(&self, world: WorldId) -> Result<WorldSummaryResponse, ControlError> {
        self.supervisor
            .world_summary(world)
            .map_err(ControlError::from)
    }

    pub fn fork_world(
        &self,
        request: ForkWorldRequest,
    ) -> Result<crate::WorldForkResult, ControlError> {
        self.runtime.fork_world(request).map_err(ControlError::from)
    }

    pub fn get_command(
        &self,
        world: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, ControlError> {
        self.supervisor
            .get_command(world, command_id)
            .map_err(ControlError::from)
    }

    pub fn submit_command<T: Serialize>(
        &self,
        world: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        payload: &T,
    ) -> Result<CommandSubmitResponse, ControlError> {
        let record = match self.mode {
            LocalExecutionMode::Worker => self
                .supervisor
                .submit_command(world, command, command_id, actor, payload)
                .map_err(ControlError::from)?,
            LocalExecutionMode::Direct => self
                .runtime
                .submit_command(world, command, command_id, actor, payload)
                .map_err(ControlError::from)?,
        };
        Ok(CommandSubmitResponse {
            poll_url: format!("/v1/worlds/{world}/commands/{}", record.command_id),
            command_id: record.command_id,
            status: record.status,
        })
    }

    pub fn manifest(&self, world: WorldId) -> Result<ManifestResponse, ControlError> {
        self.supervisor.manifest(world).map_err(ControlError::from)
    }

    pub fn defs_list(
        &self,
        world: WorldId,
        kinds: Option<Vec<String>>,
        prefix: Option<String>,
    ) -> Result<DefsListResponse, ControlError> {
        self.supervisor
            .defs_list(world, kinds, prefix)
            .map_err(ControlError::from)
    }

    pub fn def_get(
        &self,
        world: WorldId,
        kind: &str,
        name: &str,
    ) -> Result<DefGetResponse, ControlError> {
        let response = self
            .supervisor
            .def_get(world, name)
            .map_err(ControlError::from)?;
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
        self.supervisor
            .state_get(world, workflow, key)
            .map_err(ControlError::from)
    }

    pub fn state_list(
        &self,
        world: WorldId,
        workflow: &str,
        limit: u32,
        consistency: Option<&str>,
    ) -> Result<StateListResponse, ControlError> {
        require_latest_durable(consistency)?;
        self.supervisor
            .state_list(world, workflow, limit)
            .map_err(ControlError::from)
    }

    pub fn enqueue_event(
        &self,
        world: WorldId,
        ingress: DomainEventIngress,
    ) -> Result<crate::InboxSeq, ControlError> {
        match self.mode {
            LocalExecutionMode::Worker => self
                .supervisor
                .enqueue_event(world, ingress)
                .map_err(ControlError::from),
            LocalExecutionMode::Direct => self
                .runtime
                .enqueue_event(world, ingress)
                .map_err(ControlError::from),
        }
    }

    pub fn enqueue_receipt(
        &self,
        world: WorldId,
        ingress: ReceiptIngress,
    ) -> Result<crate::InboxSeq, ControlError> {
        match self.mode {
            LocalExecutionMode::Worker => self
                .supervisor
                .enqueue_receipt(world, ingress)
                .map_err(ControlError::from),
            LocalExecutionMode::Direct => self
                .runtime
                .enqueue_receipt(world, ingress)
                .map_err(ControlError::from),
        }
    }

    pub fn journal_head(&self, world: WorldId) -> Result<HeadInfoResponse, ControlError> {
        self.supervisor
            .journal_head(world)
            .map_err(ControlError::from)
    }

    pub fn journal_entries(
        &self,
        world: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<crate::api::JournalEntriesResponse, ControlError> {
        self.supervisor
            .journal_entries(world, from, limit)
            .map_err(ControlError::from)
    }

    pub fn journal_entries_raw(
        &self,
        world: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<crate::api::RawJournalEntriesResponse, ControlError> {
        self.supervisor
            .journal_entries_raw(world, from, limit)
            .map_err(ControlError::from)
    }

    pub fn runtime(&self, world: WorldId) -> Result<WorldRuntimeInfo, ControlError> {
        self.supervisor
            .runtime_info(world)
            .map_err(ControlError::from)
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
        self.supervisor
            .trace(
                world,
                aos_runtime::trace::TraceQuery {
                    event_hash: event_hash.map(ToOwned::to_owned),
                    schema: schema.map(ToOwned::to_owned),
                    correlate_by: correlate_by.map(ToOwned::to_owned),
                    correlate_value,
                    window_limit,
                },
            )
            .map_err(ControlError::from)
    }

    pub fn trace_summary(
        &self,
        world: WorldId,
        _recent_limit: u32,
    ) -> Result<serde_json::Value, ControlError> {
        self.supervisor
            .trace_summary(world)
            .map_err(ControlError::from)
    }

    pub fn workspace_resolve(
        &self,
        world: WorldId,
        workspace_name: &str,
        version: Option<u64>,
    ) -> Result<WorkspaceResolveResponse, ControlError> {
        self.supervisor
            .workspace_resolve(world, workspace_name, version)
            .map_err(ControlError::from)
    }

    pub fn workspace_empty_root(&self) -> Result<HashRef, ControlError> {
        self.runtime
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
        self.runtime
            .workspace_entries(root_hash, path, scope, cursor, limit)
            .map_err(ControlError::from)
    }

    pub fn workspace_entry(
        &self,
        root_hash: &HashRef,
        path: &str,
    ) -> Result<WorkspaceReadRefReceipt, ControlError> {
        self.runtime
            .workspace_entry(root_hash, path)
            .map_err(ControlError::from)
    }

    pub fn workspace_bytes(
        &self,
        root_hash: &HashRef,
        path: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Vec<u8>, ControlError> {
        self.runtime
            .workspace_bytes(root_hash, path, range)
            .map_err(ControlError::from)
    }

    pub fn workspace_annotations(
        &self,
        root_hash: &HashRef,
        path: Option<&str>,
    ) -> Result<WorkspaceAnnotationsGetReceipt, ControlError> {
        self.runtime
            .workspace_annotations(root_hash, path)
            .map_err(ControlError::from)
    }

    pub fn workspace_apply(
        &self,
        root_hash: HashRef,
        request: WorkspaceApplyRequest,
    ) -> Result<WorkspaceApplyResponse, ControlError> {
        self.runtime
            .workspace_apply(root_hash, request)
            .map_err(ControlError::from)
    }

    pub fn workspace_diff(
        &self,
        root_a: &HashRef,
        root_b: &HashRef,
        prefix: Option<&str>,
    ) -> Result<WorkspaceDiffReceipt, ControlError> {
        self.runtime
            .workspace_diff(root_a, root_b, prefix)
            .map_err(ControlError::from)
    }

    pub fn put_blob(
        &self,
        bytes: &[u8],
        expected_hash: Option<Hash>,
    ) -> Result<BlobPutResponse, ControlError> {
        let hash = self.runtime.put_blob(bytes)?;
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
            exists: self.runtime.blob_metadata(hash)?,
        })
    }

    pub fn get_blob(&self, hash: Hash) -> Result<Vec<u8>, ControlError> {
        self.runtime.get_blob(hash).map_err(ControlError::from)
    }
}

impl Drop for LocalControl {
    fn drop(&mut self) {
        self.supervisor.stop();
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
