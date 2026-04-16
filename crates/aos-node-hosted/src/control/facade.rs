use aos_air_types::AirNode;
use aos_cbor::Hash;
use aos_effect_types::{
    GovApplyParams, GovApproveParams, GovProposeParams, GovShadowParams, HashRef,
    WorkspaceAnnotationsGetReceipt, WorkspaceDiffReceipt, WorkspaceListReceipt,
    WorkspaceReadRefReceipt, WorkspaceResolveReceipt,
};
use aos_kernel::{DefListing, LoadedManifest, TraceQuery};
use aos_node::api::http::HttpBackend;
use aos_node::api::{
    BlobPutResponse, CasBlobMetadata, CommandSubmitBody, CommandSubmitResponse, ControlError,
    CreateWorldBody, DefGetResponse, DefsListResponse, DefsQuery, ForkWorldBody, HeadInfoResponse,
    JournalEntriesResponse, JournalQuery, LimitQuery, ManifestResponse, PutSecretVersionBody,
    RawJournalEntriesResponse, ServiceInfoResponse, StateCellSummary, StateGetQuery,
    StateGetResponse, StateListResponse, SubmitEventBody, UpsertSecretBindingBody,
    WorkspaceAnnotationsQuery, WorkspaceApplyRequest, WorkspaceApplyResponse, WorkspaceBytesQuery,
    WorkspaceDiffBody, WorkspaceEntriesQuery, WorkspaceEntryQuery, WorkspaceResolveQuery,
    WorkspaceResolveResponse,
};
use aos_node::{
    CborPayload, CommandRecord, CreateWorldRequest, DomainEventIngress, ForkWorldRequest,
    ReceiptIngress, SecretBindingRecord, SecretVersionRecord, SnapshotRecord, UniverseId, WorldId,
    WorldRuntimeInfo,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Serialize;

use crate::bootstrap::ControlDeps;
use crate::materializer::{
    CellStateProjectionRecord, HeadProjectionRecord, MaterializedWorldRow, MaterializerStoreError,
    WorkspaceRegistryProjectionRecord,
};
use crate::services::HostedProjectionStore;
use crate::vault::{HostedVaultError, UpsertSecretBinding};
use crate::worker::{CreateWorldAccepted, SubmitEventRequest, WorkerError};

use super::workspace;

pub struct ControlFacade {
    deps: ControlDeps,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostedWorldRuntimeResponse {
    pub world_id: WorldId,
    pub universe_id: aos_node::UniverseId,
    #[serde(default)]
    pub notify_counter: u64,
    #[serde(default)]
    pub has_pending_inbox: bool,
    #[serde(default)]
    pub has_pending_effects: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_timer_due_at_ns: Option<u64>,
    #[serde(default)]
    pub has_pending_maintenance: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostedWorldSummaryResponse {
    pub runtime: HostedWorldRuntimeResponse,
    pub active_baseline: SnapshotRecord,
}

impl ControlFacade {
    pub fn new(deps: ControlDeps) -> Result<Self, ControlError> {
        Ok(Self { deps })
    }

    pub fn health(&self) -> Result<ServiceInfoResponse, ControlError> {
        Ok(ServiceInfoResponse {
            service: "aos-node-hosted",
            version: env!("CARGO_PKG_VERSION"),
            pid: Some(std::process::id()),
            state_root: Some(self.deps.state_root.clone()),
        })
    }

    pub fn default_universe_id(&self) -> Result<UniverseId, ControlError> {
        Ok(self.deps.default_universe_id)
    }

    pub fn list_secret_bindings(
        &self,
        universe_id: UniverseId,
    ) -> Result<Vec<SecretBindingRecord>, ControlError> {
        self.require_default_universe(universe_id)?;
        self.deps
            .secrets
            .list_bindings(universe_id)
            .map_err(control_error_from_vault)
    }

    pub fn get_secret_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<SecretBindingRecord, ControlError> {
        self.require_default_universe(universe_id)?;
        self.deps
            .secrets
            .get_binding(universe_id, binding_id)
            .map_err(control_error_from_vault)?
            .ok_or_else(|| ControlError::not_found(format!("secret binding '{binding_id}'")))
    }

    pub fn upsert_secret_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
        body: UpsertSecretBindingBody,
    ) -> Result<SecretBindingRecord, ControlError> {
        self.require_default_universe(universe_id)?;
        self.deps
            .secrets
            .upsert_binding(
                universe_id,
                binding_id,
                UpsertSecretBinding {
                    source_kind: body.source_kind,
                    env_var: body.env_var,
                    required_placement_pin: body.required_placement_pin,
                    status: body.status,
                },
            )
            .map_err(control_error_from_vault)
    }

    pub fn delete_secret_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<SecretBindingRecord, ControlError> {
        self.require_default_universe(universe_id)?;
        self.deps
            .secrets
            .delete_binding(universe_id, binding_id)
            .map_err(control_error_from_vault)
    }

    pub fn list_secret_versions(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<Vec<SecretVersionRecord>, ControlError> {
        self.require_default_universe(universe_id)?;
        self.deps
            .secrets
            .list_versions(universe_id, binding_id)
            .map_err(control_error_from_vault)
    }

    pub fn get_secret_version(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<SecretVersionRecord, ControlError> {
        self.require_default_universe(universe_id)?;
        self.deps
            .secrets
            .get_version(universe_id, binding_id, version)
            .map_err(control_error_from_vault)?
            .ok_or_else(|| {
                ControlError::not_found(format!("secret version '{binding_id}@{version}'"))
            })
    }

    pub fn put_secret_version(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
        body: PutSecretVersionBody,
    ) -> Result<SecretVersionRecord, ControlError> {
        self.require_default_universe(universe_id)?;
        let plaintext = decode_b64(&body.plaintext_b64)?;
        self.deps
            .secrets
            .put_secret_value(
                universe_id,
                binding_id,
                &plaintext,
                body.expected_digest.as_deref(),
                body.actor,
            )
            .map_err(control_error_from_vault)
    }

    pub fn create_world(&self, body: CreateWorldBody) -> Result<CreateWorldAccepted, ControlError> {
        self.deps
            .submissions
            .create_world(
                body.universe_id,
                CreateWorldRequest {
                    world_id: body.world_id,
                    universe_id: body.universe_id,
                    created_at_ns: body.created_at_ns,
                    source: body.source,
                },
            )
            .map_err(control_error_from_worker)
    }

    pub fn fork_world(
        &self,
        src_world_id: WorldId,
        body: ForkWorldBody,
    ) -> Result<CreateWorldAccepted, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.deps
            .replay
            .create_fork_seed_request(
                universe_id,
                body.new_world_id
                    .unwrap_or_else(|| WorldId::from(uuid::Uuid::new_v4())),
                &ForkWorldRequest {
                    src_world_id,
                    src_snapshot: body.src_snapshot,
                    new_world_id: body.new_world_id,
                    forked_at_ns: body.forked_at_ns,
                    pending_effect_policy: body.pending_effect_policy,
                },
            )
            .and_then(|request| {
                self.deps
                    .submissions
                    .create_world(request.universe_id, request)
            })
            .map_err(control_error_from_worker)
    }

    pub fn list_worlds(&self) -> Result<Vec<HostedWorldRuntimeResponse>, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.projections()?
            .load_world_projections_page(universe_id, None, u32::MAX)
            .map(|worlds| {
                worlds
                    .into_iter()
                    .map(map_materialized_world_runtime)
                    .collect()
            })
            .map_err(control_error_from_materializer)
    }

    pub fn list_worlds_page(
        &self,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<HostedWorldRuntimeResponse>, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.projections()?
            .load_world_projections_page(universe_id, after, limit)
            .map(|worlds| {
                worlds
                    .into_iter()
                    .map(map_materialized_world_runtime)
                    .collect()
            })
            .map_err(control_error_from_materializer)
    }

    pub fn get_world(&self, world_id: WorldId) -> Result<HostedWorldSummaryResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        let world = self.load_world_projection(universe_id, world_id)?;
        Ok(HostedWorldSummaryResponse {
            runtime: map_materialized_world_runtime(world.clone()),
            active_baseline: world.active_baseline,
        })
    }

    pub fn trace(
        &self,
        world_id: WorldId,
        event_hash: Option<&str>,
        schema: Option<&str>,
        correlate_by: Option<&str>,
        correlate_value: Option<serde_json::Value>,
        window_limit: Option<u64>,
    ) -> Result<serde_json::Value, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.deps
            .replay
            .trace(
                universe_id,
                world_id,
                TraceQuery {
                    event_hash: event_hash.map(str::to_owned),
                    schema: schema.map(str::to_owned),
                    correlate_by: correlate_by.map(str::to_owned),
                    correlate_value,
                    window_limit,
                },
            )
            .map_err(control_error_from_worker)
    }

    pub fn trace_summary(
        &self,
        world_id: WorldId,
        _recent_limit: u32,
    ) -> Result<serde_json::Value, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.deps
            .replay
            .trace_summary(universe_id, world_id)
            .map_err(control_error_from_worker)
    }

    pub fn submit_event(
        &self,
        world_id: WorldId,
        body: SubmitEventBody,
    ) -> Result<crate::worker::SubmissionAccepted, ControlError> {
        if body.value_b64.is_some() {
            return Err(ControlError::invalid(
                "hosted control does not yet accept value_b64 event payloads",
            ));
        }
        if body.key_b64.is_some() {
            return Err(ControlError::invalid(
                "hosted control does not yet accept keyed event ingress",
            ));
        }
        let value = body
            .value
            .or(body.value_json)
            .ok_or_else(|| ControlError::invalid("missing event value"))?;
        let universe_id = self.default_universe_id()?;
        self.deps
            .submissions
            .submit_event(SubmitEventRequest {
                universe_id,
                world_id,
                schema: body.schema,
                value,
                submission_id: body.submission_id,
                expected_world_epoch: body.expected_world_epoch,
            })
            .map_err(control_error_from_worker)
    }

    pub fn submit_receipt(
        &self,
        world_id: WorldId,
        body: ReceiptIngress,
    ) -> Result<crate::worker::SubmissionAccepted, ControlError> {
        let universe_id = self.default_universe_id()?;
        body.payload.validate()?;
        self.deps
            .submissions
            .submit_receipt(universe_id, world_id, body)
            .map_err(control_error_from_worker)
    }

    pub fn enqueue_event(
        &self,
        world_id: WorldId,
        ingress: DomainEventIngress,
    ) -> Result<crate::worker::SubmissionAccepted, ControlError> {
        let value = ingress.value.inline_cbor.ok_or_else(|| {
            ControlError::invalid("hosted control currently requires inline event payloads")
        })?;
        let value: serde_json::Value = serde_cbor::from_slice(&value)?;
        let correlation_id = ingress.correlation_id;
        self.submit_event(
            world_id,
            SubmitEventBody {
                schema: ingress.schema,
                value: Some(value),
                value_json: None,
                value_b64: None,
                key_b64: None,
                correlation_id: correlation_id.clone(),
                submission_id: correlation_id,
                expected_world_epoch: None,
            },
        )
    }

    pub fn get_command(
        &self,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.deps
            .submissions
            .get_command(universe_id, world_id, command_id)
            .map_err(control_error_from_worker)
    }

    pub fn submit_command<T: Serialize>(
        &self,
        world_id: WorldId,
        command: &str,
        body: CommandSubmitBody<T>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.deps
            .submissions
            .submit_command(
                universe_id,
                world_id,
                command,
                body.command_id,
                body.actor,
                &body.params,
            )
            .map_err(control_error_from_worker)
    }

    pub fn governance_propose(
        &self,
        world_id: WorldId,
        body: CommandSubmitBody<GovProposeParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        self.submit_command(world_id, "gov-propose", body)
    }

    pub fn governance_shadow(
        &self,
        world_id: WorldId,
        body: CommandSubmitBody<GovShadowParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        self.submit_command(world_id, "gov-shadow", body)
    }

    pub fn governance_approve(
        &self,
        world_id: WorldId,
        body: CommandSubmitBody<GovApproveParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        self.submit_command(world_id, "gov-approve", body)
    }

    pub fn governance_apply(
        &self,
        world_id: WorldId,
        body: CommandSubmitBody<GovApplyParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        self.submit_command(world_id, "gov-apply", body)
    }

    pub fn manifest(&self, world_id: WorldId) -> Result<ManifestResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        let head = self.load_head_projection(universe_id, world_id)?;
        let loaded = self.load_materialized_manifest(universe_id, world_id, &head.manifest_hash)?;
        Ok(ManifestResponse {
            journal_head: head.journal_head,
            manifest_hash: head.manifest_hash,
            manifest: loaded.manifest,
        })
    }

    pub fn defs_list(
        &self,
        world_id: WorldId,
        query: DefsQuery,
    ) -> Result<DefsListResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        let head = self.load_head_projection(universe_id, world_id)?;
        let loaded = self.load_materialized_manifest(universe_id, world_id, &head.manifest_hash)?;
        let kinds = query.kinds.map(|raw| {
            raw.split(',')
                .filter(|kind| !kind.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        });
        Ok(DefsListResponse {
            journal_head: head.journal_head,
            manifest_hash: head.manifest_hash,
            defs: list_defs_from_manifest(&loaded, kinds.as_deref(), query.prefix.as_deref()),
        })
    }

    pub fn def_get(
        &self,
        world_id: WorldId,
        kind: &str,
        name: &str,
    ) -> Result<DefGetResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        let head = self.load_head_projection(universe_id, world_id)?;
        let loaded = self.load_materialized_manifest(universe_id, world_id, &head.manifest_hash)?;
        let def = get_def_from_manifest(&loaded, name)
            .ok_or_else(|| ControlError::not_found(format!("definition '{name}'")))?;
        if !def_matches_kind(&def, kind) {
            return Err(ControlError::not_found(format!(
                "definition '{name}' with kind '{kind}'"
            )));
        }
        Ok(DefGetResponse {
            journal_head: head.journal_head,
            manifest_hash: head.manifest_hash,
            def,
        })
    }

    pub fn state_get(
        &self,
        world_id: WorldId,
        workflow: &str,
        query: StateGetQuery,
    ) -> Result<StateGetResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        require_latest_durable(query.consistency.as_deref())?;
        let key = query.key_b64.map(|value| decode_b64(&value)).transpose()?;
        self.state_get_materialized(universe_id, world_id, workflow, key)
    }

    pub fn state_list(
        &self,
        world_id: WorldId,
        workflow: &str,
        query: LimitQuery,
    ) -> Result<StateListResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        require_latest_durable(query.consistency.as_deref())?;
        let head = self.load_head_projection(universe_id, world_id)?;
        let rows = self
            .projections()?
            .load_cell_projections(universe_id, world_id, workflow, query.limit)
            .map_err(control_error_from_materializer)?;
        Ok(StateListResponse {
            journal_head: head.journal_head,
            workflow: workflow.to_owned(),
            cells: rows
                .into_iter()
                .map(|row| map_state_cell_summary(row.cell))
                .collect(),
        })
    }

    pub fn journal_head(&self, world_id: WorldId) -> Result<HeadInfoResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.projections()?
            .load_journal_head(universe_id, world_id)
            .map_err(control_error_from_materializer)?
            .ok_or_else(|| ControlError::not_found(format!("journal head for world {world_id}")))
    }

    pub fn journal_entries(
        &self,
        world_id: WorldId,
        query: JournalQuery,
    ) -> Result<JournalEntriesResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.projections()?
            .load_journal_entries(universe_id, world_id, query.from, query.limit)
            .map_err(control_error_from_materializer)?
            .ok_or_else(|| ControlError::not_found(format!("journal for world {world_id}")))
    }

    pub fn journal_entries_raw(
        &self,
        world_id: WorldId,
        query: JournalQuery,
    ) -> Result<RawJournalEntriesResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.projections()?
            .load_journal_entries_raw(universe_id, world_id, query.from, query.limit)
            .map_err(control_error_from_materializer)?
            .ok_or_else(|| ControlError::not_found(format!("journal for world {world_id}")))
    }

    pub fn runtime(&self, world_id: WorldId) -> Result<HostedWorldRuntimeResponse, ControlError> {
        self.load_world_projection(self.default_universe_id()?, world_id)
            .map(map_materialized_world_runtime)
    }

    pub fn workspace_resolve(
        &self,
        world_id: WorldId,
        query: WorkspaceResolveQuery,
    ) -> Result<WorkspaceResolveResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        let projection = self
            .projections()?
            .load_workspace_projection(universe_id, world_id, &query.workspace)
            .map_err(control_error_from_materializer)?;
        let receipt = workspace_receipt_from_projection(projection.as_ref(), query.version)?;
        Ok(WorkspaceResolveResponse {
            workspace: query.workspace,
            receipt,
        })
    }

    fn workspace_store(
        &self,
        universe_id: Option<aos_node::UniverseId>,
    ) -> Result<std::sync::Arc<crate::blobstore::HostedCas>, ControlError> {
        let universe_id = match universe_id {
            Some(universe_id) => universe_id,
            None => self.default_universe_id()?,
        };
        self.deps
            .cas
            .store_for_domain(universe_id)
            .map_err(control_error_from_worker)
    }

    pub fn workspace_empty_root(
        &self,
        universe_id: Option<aos_node::UniverseId>,
    ) -> Result<HashRef, ControlError> {
        let store = self.workspace_store(universe_id)?;
        workspace::empty_root(store.as_ref()).map_err(Into::into)
    }

    pub fn workspace_entries(
        &self,
        universe_id: Option<aos_node::UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceEntriesQuery,
    ) -> Result<WorkspaceListReceipt, ControlError> {
        let store = self.workspace_store(universe_id)?;
        workspace::list(
            store.as_ref(),
            root_hash,
            query.path.as_deref(),
            query.scope.as_deref(),
            query.cursor.as_deref(),
            query.limit,
        )
        .map_err(Into::into)
    }

    pub fn workspace_entry(
        &self,
        universe_id: Option<aos_node::UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceEntryQuery,
    ) -> Result<WorkspaceReadRefReceipt, ControlError> {
        let store = self.workspace_store(universe_id)?;
        workspace::read_ref(store.as_ref(), root_hash, &query.path).map_err(Into::into)
    }

    pub fn workspace_bytes(
        &self,
        universe_id: Option<aos_node::UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceBytesQuery,
    ) -> Result<Vec<u8>, ControlError> {
        let range = match (query.start, query.end) {
            (Some(start), Some(end)) => Some((start, end)),
            (None, None) => None,
            _ => {
                return Err(ControlError::invalid(
                    "start and end must be provided together",
                ));
            }
        };
        let store = self.workspace_store(universe_id)?;
        workspace::read_bytes(store.as_ref(), root_hash, &query.path, range).map_err(Into::into)
    }

    pub fn workspace_annotations(
        &self,
        universe_id: Option<aos_node::UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceAnnotationsQuery,
    ) -> Result<WorkspaceAnnotationsGetReceipt, ControlError> {
        let store = self.workspace_store(universe_id)?;
        workspace::annotations_get(store.as_ref(), root_hash, query.path.as_deref())
            .map_err(Into::into)
    }

    pub fn workspace_apply(
        &self,
        universe_id: Option<aos_node::UniverseId>,
        root_hash: HashRef,
        request: WorkspaceApplyRequest,
    ) -> Result<WorkspaceApplyResponse, ControlError> {
        let store = self.workspace_store(universe_id)?;
        let base_root_hash = root_hash;
        let mut current = base_root_hash.clone();
        for operation in request.operations {
            current = match operation {
                aos_node::api::WorkspaceApplyOp::WriteBytes {
                    path,
                    bytes_b64,
                    mode,
                } => {
                    let bytes = decode_b64(&bytes_b64)?;
                    workspace::write_bytes(store.as_ref(), &current, &path, &bytes, mode)?
                        .new_root_hash
                }
                aos_node::api::WorkspaceApplyOp::WriteRef {
                    path,
                    blob_hash,
                    mode,
                } => {
                    workspace::write_ref(store.as_ref(), &current, &path, &blob_hash, mode)?
                        .new_root_hash
                }
                aos_node::api::WorkspaceApplyOp::Remove { path } => {
                    workspace::remove(store.as_ref(), &current, &path)?.new_root_hash
                }
                aos_node::api::WorkspaceApplyOp::SetAnnotations {
                    path,
                    annotations_patch,
                } => {
                    workspace::annotations_set(
                        store.as_ref(),
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
        universe_id: Option<aos_node::UniverseId>,
        body: WorkspaceDiffBody,
    ) -> Result<WorkspaceDiffReceipt, ControlError> {
        let store = self.workspace_store(universe_id)?;
        workspace::diff(
            store.as_ref(),
            &body.root_a,
            &body.root_b,
            body.prefix.as_deref(),
        )
        .map_err(Into::into)
    }

    pub fn put_blob(
        &self,
        bytes: &[u8],
        universe_id: Option<aos_node::UniverseId>,
        expected_hash: Option<Hash>,
    ) -> Result<BlobPutResponse, ControlError> {
        let universe_id = match universe_id {
            Some(universe_id) => universe_id,
            None => self.default_universe_id()?,
        };
        let hash = self
            .deps
            .cas
            .put_blob(universe_id, bytes)
            .map_err(control_error_from_worker)?;
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

    pub fn head_blob(
        &self,
        universe_id: Option<aos_node::UniverseId>,
        hash: Hash,
    ) -> Result<CasBlobMetadata, ControlError> {
        let universe_id = match universe_id {
            Some(universe_id) => universe_id,
            None => self.default_universe_id()?,
        };
        Ok(CasBlobMetadata {
            hash: hash.to_hex(),
            exists: self
                .deps
                .cas
                .blob_metadata(universe_id, hash)
                .map_err(control_error_from_worker)?,
        })
    }

    pub fn get_blob(
        &self,
        universe_id: Option<aos_node::UniverseId>,
        hash: Hash,
    ) -> Result<Vec<u8>, ControlError> {
        let universe_id = match universe_id {
            Some(universe_id) => universe_id,
            None => self.default_universe_id()?,
        };
        self.deps
            .cas
            .get_blob(universe_id, hash)
            .map_err(control_error_from_worker)
    }

    fn state_get_materialized(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        key: Option<Vec<u8>>,
    ) -> Result<StateGetResponse, ControlError> {
        let head = self.load_head_projection(universe_id, world_id)?;
        let key_bytes = key.unwrap_or_default();
        let row = self
            .projections()?
            .load_cell_projection(universe_id, world_id, workflow, &key_bytes)
            .map_err(control_error_from_materializer)?;
        let state_b64 = row
            .as_ref()
            .map(|row| self.resolve_payload_bytes(head.universe_id, &row.state_payload))
            .transpose()?
            .map(|bytes| BASE64_STANDARD.encode(bytes));
        Ok(StateGetResponse {
            journal_head: head.journal_head,
            workflow: workflow.to_owned(),
            key_b64: Some(BASE64_STANDARD.encode(&key_bytes)),
            cell: row.map(|row| map_state_cell_summary(row.cell)),
            state_b64,
        })
    }

    fn load_materialized_manifest(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        manifest_hash: &str,
    ) -> Result<LoadedManifest, ControlError> {
        self.deps
            .cas
            .load_manifest(
                self.load_head_projection(universe_id, world_id)?
                    .universe_id,
                manifest_hash,
            )
            .map_err(control_error_from_worker)
    }

    fn load_head_projection(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<HeadProjectionRecord, ControlError> {
        self.projections()?
            .load_head_projection(universe_id, world_id)
            .map_err(control_error_from_materializer)?
            .ok_or_else(|| {
                ControlError::not_found(format!(
                    "materialized head projection for world {world_id}"
                ))
            })
    }

    fn load_world_projection(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<MaterializedWorldRow, ControlError> {
        self.projections()?
            .load_world_projection(universe_id, world_id)
            .map_err(control_error_from_materializer)?
            .ok_or_else(|| {
                ControlError::not_found(format!(
                    "materialized world projection for world {world_id}"
                ))
            })
    }

    fn resolve_payload_bytes(
        &self,
        universe_id: UniverseId,
        payload: &CborPayload,
    ) -> Result<Vec<u8>, ControlError> {
        if let Some(bytes) = &payload.inline_cbor {
            return Ok(bytes.clone());
        }
        let hash = payload
            .cbor_ref
            .as_deref()
            .ok_or_else(|| ControlError::invalid("materialized state payload is missing cbor_ref"))
            .and_then(parse_hash_ref)?;
        self.deps
            .cas
            .get_blob(universe_id, hash)
            .map_err(control_error_from_worker)
    }

    fn projections(&self) -> Result<&HostedProjectionStore, ControlError> {
        Ok(&self.deps.projections)
    }
}

impl HttpBackend for ControlFacade {
    type CreateWorldResponse = CreateWorldAccepted;
    type ForkWorldResponse = CreateWorldAccepted;
    type SubmitEventResponse = crate::SubmissionAccepted;
    type SubmitReceiptResponse = crate::SubmissionAccepted;
    type WorkspaceEntryResponse = WorkspaceReadRefReceipt;

    fn health(&self) -> Result<ServiceInfoResponse, ControlError> {
        ControlFacade::health(self)
    }

    fn list_worlds(
        &self,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, ControlError> {
        Ok(self
            .list_worlds_page(after, limit)?
            .into_iter()
            .map(map_hosted_runtime_to_world_runtime)
            .collect())
    }

    fn create_world(
        &self,
        body: CreateWorldBody,
    ) -> Result<Self::CreateWorldResponse, ControlError> {
        ControlFacade::create_world(self, body)
    }

    fn get_world(
        &self,
        world_id: WorldId,
    ) -> Result<aos_node::api::WorldSummaryResponse, ControlError> {
        let summary = ControlFacade::get_world(self, world_id)?;
        Ok(aos_node::api::WorldSummaryResponse {
            runtime: map_hosted_runtime_to_world_runtime(summary.runtime),
            active_baseline: summary.active_baseline,
        })
    }

    fn checkpoint_world(
        &self,
        world_id: WorldId,
    ) -> Result<aos_node::api::WorldSummaryResponse, ControlError> {
        let summary = self.get_world(world_id)?;
        Ok(aos_node::api::WorldSummaryResponse {
            runtime: map_hosted_runtime_to_world_runtime(summary.runtime),
            active_baseline: summary.active_baseline,
        })
    }

    fn fork_world(
        &self,
        src_world_id: WorldId,
        body: ForkWorldBody,
    ) -> Result<Self::ForkWorldResponse, ControlError> {
        ControlFacade::fork_world(self, src_world_id, body)
    }

    fn manifest(&self, world_id: WorldId) -> Result<ManifestResponse, ControlError> {
        ControlFacade::manifest(self, world_id)
    }

    fn defs_list(
        &self,
        world_id: WorldId,
        query: DefsQuery,
    ) -> Result<DefsListResponse, ControlError> {
        ControlFacade::defs_list(self, world_id, query)
    }

    fn def_get(
        &self,
        world_id: WorldId,
        kind: &str,
        name: &str,
    ) -> Result<DefGetResponse, ControlError> {
        ControlFacade::def_get(self, world_id, kind, name)
    }

    fn runtime(&self, world_id: WorldId) -> Result<WorldRuntimeInfo, ControlError> {
        Ok(map_hosted_runtime_to_world_runtime(ControlFacade::runtime(
            self, world_id,
        )?))
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
        ControlFacade::trace(
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
        ControlFacade::trace_summary(self, world_id, recent_limit)
    }

    fn journal_head(&self, world_id: WorldId) -> Result<HeadInfoResponse, ControlError> {
        ControlFacade::journal_head(self, world_id)
    }

    fn journal_entries(
        &self,
        world_id: WorldId,
        query: JournalQuery,
    ) -> Result<JournalEntriesResponse, ControlError> {
        ControlFacade::journal_entries(self, world_id, query)
    }

    fn journal_entries_raw(
        &self,
        world_id: WorldId,
        query: JournalQuery,
    ) -> Result<RawJournalEntriesResponse, ControlError> {
        ControlFacade::journal_entries_raw(self, world_id, query)
    }

    fn get_command(
        &self,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, ControlError> {
        ControlFacade::get_command(self, world_id, command_id)
    }

    fn state_get(
        &self,
        world_id: WorldId,
        workflow: &str,
        query: StateGetQuery,
    ) -> Result<StateGetResponse, ControlError> {
        ControlFacade::state_get(self, world_id, workflow, query)
    }

    fn state_list(
        &self,
        world_id: WorldId,
        workflow: &str,
        query: LimitQuery,
    ) -> Result<StateListResponse, ControlError> {
        ControlFacade::state_list(self, world_id, workflow, query)
    }

    fn workspace_resolve(
        &self,
        world_id: WorldId,
        query: WorkspaceResolveQuery,
    ) -> Result<WorkspaceResolveResponse, ControlError> {
        ControlFacade::workspace_resolve(self, world_id, query)
    }

    fn submit_event(
        &self,
        world_id: WorldId,
        body: SubmitEventBody,
    ) -> Result<Self::SubmitEventResponse, ControlError> {
        ControlFacade::submit_event(self, world_id, body)
    }

    fn submit_receipt(
        &self,
        world_id: WorldId,
        body: ReceiptIngress,
    ) -> Result<Self::SubmitReceiptResponse, ControlError> {
        ControlFacade::submit_receipt(self, world_id, body)
    }

    fn governance_propose(
        &self,
        world_id: WorldId,
        body: CommandSubmitBody<GovProposeParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        ControlFacade::governance_propose(self, world_id, body)
    }

    fn governance_shadow(
        &self,
        world_id: WorldId,
        body: CommandSubmitBody<GovShadowParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        ControlFacade::governance_shadow(self, world_id, body)
    }

    fn governance_approve(
        &self,
        world_id: WorldId,
        body: CommandSubmitBody<GovApproveParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        ControlFacade::governance_approve(self, world_id, body)
    }

    fn governance_apply(
        &self,
        world_id: WorldId,
        body: CommandSubmitBody<GovApplyParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        ControlFacade::governance_apply(self, world_id, body)
    }

    fn workspace_empty_root(
        &self,
        universe_id: Option<UniverseId>,
    ) -> Result<HashRef, ControlError> {
        ControlFacade::workspace_empty_root(self, universe_id)
    }

    fn workspace_entries(
        &self,
        universe_id: Option<UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceEntriesQuery,
    ) -> Result<WorkspaceListReceipt, ControlError> {
        ControlFacade::workspace_entries(self, universe_id, root_hash, query)
    }

    fn workspace_entry(
        &self,
        universe_id: Option<UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceEntryQuery,
    ) -> Result<Self::WorkspaceEntryResponse, ControlError> {
        ControlFacade::workspace_entry(self, universe_id, root_hash, query)
    }

    fn workspace_bytes(
        &self,
        universe_id: Option<UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceBytesQuery,
    ) -> Result<Vec<u8>, ControlError> {
        ControlFacade::workspace_bytes(self, universe_id, root_hash, query)
    }

    fn workspace_annotations(
        &self,
        universe_id: Option<UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceAnnotationsQuery,
    ) -> Result<WorkspaceAnnotationsGetReceipt, ControlError> {
        ControlFacade::workspace_annotations(self, universe_id, root_hash, query)
    }

    fn workspace_apply(
        &self,
        universe_id: Option<UniverseId>,
        root_hash: HashRef,
        request: WorkspaceApplyRequest,
    ) -> Result<WorkspaceApplyResponse, ControlError> {
        ControlFacade::workspace_apply(self, universe_id, root_hash, request)
    }

    fn workspace_diff(
        &self,
        universe_id: Option<UniverseId>,
        body: WorkspaceDiffBody,
    ) -> Result<WorkspaceDiffReceipt, ControlError> {
        ControlFacade::workspace_diff(self, universe_id, body)
    }

    fn put_blob(
        &self,
        bytes: &[u8],
        universe_id: Option<UniverseId>,
        expected_hash: Option<Hash>,
    ) -> Result<BlobPutResponse, ControlError> {
        ControlFacade::put_blob(self, bytes, universe_id, expected_hash)
    }

    fn head_blob(
        &self,
        universe_id: Option<UniverseId>,
        hash: Hash,
    ) -> Result<CasBlobMetadata, ControlError> {
        ControlFacade::head_blob(self, universe_id, hash)
    }

    fn get_blob(
        &self,
        universe_id: Option<UniverseId>,
        hash: Hash,
    ) -> Result<Vec<u8>, ControlError> {
        ControlFacade::get_blob(self, universe_id, hash)
    }

    fn list_secret_bindings(
        &self,
        universe_id: Option<UniverseId>,
    ) -> Result<Vec<SecretBindingRecord>, ControlError> {
        let universe_id = match universe_id {
            Some(universe_id) => universe_id,
            None => self.default_universe_id()?,
        };
        ControlFacade::list_secret_bindings(self, universe_id)
    }

    fn get_secret_binding(
        &self,
        universe_id: Option<UniverseId>,
        binding_id: &str,
    ) -> Result<SecretBindingRecord, ControlError> {
        let universe_id = match universe_id {
            Some(universe_id) => universe_id,
            None => self.default_universe_id()?,
        };
        ControlFacade::get_secret_binding(self, universe_id, binding_id)
    }

    fn upsert_secret_binding(
        &self,
        universe_id: Option<UniverseId>,
        binding_id: &str,
        body: UpsertSecretBindingBody,
    ) -> Result<SecretBindingRecord, ControlError> {
        let universe_id = match universe_id {
            Some(universe_id) => universe_id,
            None => self.default_universe_id()?,
        };
        ControlFacade::upsert_secret_binding(self, universe_id, binding_id, body)
    }

    fn delete_secret_binding(
        &self,
        universe_id: Option<UniverseId>,
        binding_id: &str,
    ) -> Result<SecretBindingRecord, ControlError> {
        let universe_id = match universe_id {
            Some(universe_id) => universe_id,
            None => self.default_universe_id()?,
        };
        ControlFacade::delete_secret_binding(self, universe_id, binding_id)
    }

    fn list_secret_versions(
        &self,
        universe_id: Option<UniverseId>,
        binding_id: &str,
    ) -> Result<Vec<SecretVersionRecord>, ControlError> {
        let universe_id = match universe_id {
            Some(universe_id) => universe_id,
            None => self.default_universe_id()?,
        };
        ControlFacade::list_secret_versions(self, universe_id, binding_id)
    }

    fn put_secret_version(
        &self,
        universe_id: Option<UniverseId>,
        binding_id: &str,
        body: PutSecretVersionBody,
    ) -> Result<SecretVersionRecord, ControlError> {
        let universe_id = match universe_id {
            Some(universe_id) => universe_id,
            None => self.default_universe_id()?,
        };
        ControlFacade::put_secret_version(self, universe_id, binding_id, body)
    }

    fn get_secret_version(
        &self,
        universe_id: Option<UniverseId>,
        binding_id: &str,
        version: u64,
    ) -> Result<SecretVersionRecord, ControlError> {
        let universe_id = match universe_id {
            Some(universe_id) => universe_id,
            None => self.default_universe_id()?,
        };
        ControlFacade::get_secret_version(self, universe_id, binding_id, version)
    }
}

impl ControlFacade {
    fn require_default_universe(&self, universe_id: UniverseId) -> Result<(), ControlError> {
        let _ = universe_id;
        Ok(())
    }
}

fn list_defs_from_manifest(
    loaded: &LoadedManifest,
    kinds: Option<&[String]>,
    prefix: Option<&str>,
) -> Vec<DefListing> {
    let prefix = prefix.unwrap_or("");
    let mut entries = Vec::new();
    let mut push = |kind: &str, name: &str, node: AirNode, mut entry: DefListing| {
        if !name.starts_with(prefix) {
            return;
        }
        if let Some(kinds) = kinds
            && !kinds
                .iter()
                .any(|candidate| def_kind_matches(candidate, kind))
        {
            return;
        }
        let hash = Hash::of_bytes(
            &serde_cbor::to_vec(&node).expect("AIR node serialization should succeed"),
        )
        .to_hex();
        entry.hash = hash;
        entries.push(entry);
    };

    for (name, def) in &loaded.schemas {
        push(
            "defschema",
            name.as_str(),
            AirNode::Defschema(def.clone()),
            DefListing {
                kind: "defschema".into(),
                name: name.clone(),
                hash: String::new(),
                cap_type: None,
                params_schema: None,
                receipt_schema: None,
                plan_steps: None,
                policy_rules: None,
            },
        );
    }
    for (name, def) in &loaded.modules {
        push(
            "defmodule",
            name.as_str(),
            AirNode::Defmodule(def.clone()),
            DefListing {
                kind: "defmodule".into(),
                name: name.clone(),
                hash: String::new(),
                cap_type: None,
                params_schema: None,
                receipt_schema: None,
                plan_steps: None,
                policy_rules: None,
            },
        );
    }
    for (name, def) in &loaded.caps {
        push(
            "defcap",
            name.as_str(),
            AirNode::Defcap(def.clone()),
            DefListing {
                kind: "defcap".into(),
                name: name.clone(),
                hash: String::new(),
                cap_type: Some(def.cap_type.as_str().to_owned()),
                params_schema: None,
                receipt_schema: None,
                plan_steps: None,
                policy_rules: None,
            },
        );
    }
    for (name, def) in &loaded.effects {
        push(
            "defeffect",
            name.as_str(),
            AirNode::Defeffect(def.clone()),
            DefListing {
                kind: "defeffect".into(),
                name: name.clone(),
                hash: String::new(),
                cap_type: Some(def.cap_type.as_str().to_owned()),
                params_schema: Some(def.params_schema.as_str().to_owned()),
                receipt_schema: Some(def.receipt_schema.as_str().to_owned()),
                plan_steps: None,
                policy_rules: None,
            },
        );
    }
    for (name, def) in &loaded.policies {
        push(
            "defpolicy",
            name.as_str(),
            AirNode::Defpolicy(def.clone()),
            DefListing {
                kind: "defpolicy".into(),
                name: name.clone(),
                hash: String::new(),
                cap_type: None,
                params_schema: None,
                receipt_schema: None,
                plan_steps: None,
                policy_rules: Some(def.rules.len()),
            },
        );
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

fn get_def_from_manifest(loaded: &LoadedManifest, name: &str) -> Option<AirNode> {
    if let Some(def) = loaded.schemas.get(name) {
        return Some(AirNode::Defschema(def.clone()));
    }
    if let Some(def) = loaded.modules.get(name) {
        return Some(AirNode::Defmodule(def.clone()));
    }
    if let Some(def) = loaded.caps.get(name) {
        return Some(AirNode::Defcap(def.clone()));
    }
    if let Some(def) = loaded.policies.get(name) {
        return Some(AirNode::Defpolicy(def.clone()));
    }
    loaded
        .effects
        .get(name)
        .map(|def| AirNode::Defeffect(def.clone()))
}

fn workspace_receipt_from_projection(
    projection: Option<&WorkspaceRegistryProjectionRecord>,
    version: Option<u64>,
) -> Result<WorkspaceResolveReceipt, ControlError> {
    let Some(projection) = projection else {
        return Ok(WorkspaceResolveReceipt {
            exists: false,
            resolved_version: None,
            head: None,
            root_hash: None,
        });
    };
    let head = Some(projection.latest_version);
    let target = version.unwrap_or(projection.latest_version);
    let Some(entry) = projection.versions.get(&target) else {
        return Ok(WorkspaceResolveReceipt {
            exists: false,
            resolved_version: None,
            head,
            root_hash: None,
        });
    };
    Ok(WorkspaceResolveReceipt {
        exists: true,
        resolved_version: Some(target),
        head,
        root_hash: Some(HashRef::new(entry.root_hash.clone())?),
    })
}

fn parse_hash_ref(value: &str) -> Result<Hash, ControlError> {
    Hash::from_hex_str(value)
        .map_err(|_| ControlError::invalid(format!("invalid hash ref '{value}'")))
}

fn decode_b64(value: &str) -> Result<Vec<u8>, ControlError> {
    BASE64_STANDARD
        .decode(value)
        .map_err(|err| ControlError::invalid(format!("invalid base64: {err}")))
}

fn def_kind_matches(candidate: &str, canonical: &str) -> bool {
    match candidate.trim().to_ascii_lowercase().as_str() {
        "schema" | "defschema" => canonical == "defschema",
        "module" | "defmodule" => canonical == "defmodule",
        "cap" | "defcap" => canonical == "defcap",
        "effect" | "defeffect" => canonical == "defeffect",
        "policy" | "defpolicy" => canonical == "defpolicy",
        other => other == canonical,
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

pub(crate) fn control_error_from_worker(error: WorkerError) -> ControlError {
    match error {
        WorkerError::Persist(err) => err.into(),
        WorkerError::Kernel(err) => err.into(),
        WorkerError::Store(err) => err.into(),
        WorkerError::Cbor(err) => err.into(),
        WorkerError::Json(err) => err.into(),
        WorkerError::UnknownWorld {
            universe_id,
            world_id,
        } => ControlError::not_found(format!("world {world_id} in universe {universe_id}")),
        WorkerError::UnknownCommand {
            universe_id,
            world_id,
            command_id,
        } => ControlError::not_found(format!(
            "command '{command_id}' for world {world_id} in universe {universe_id}"
        )),
        WorkerError::WorldEpochMismatch { expected, got, .. } => ControlError::invalid(format!(
            "world epoch mismatch: expected {expected}, got {got}"
        )),
        other => ControlError::invalid(other.to_string()),
    }
}

pub(crate) fn control_error_from_vault(error: HostedVaultError) -> ControlError {
    match error {
        HostedVaultError::Store(err) => {
            ControlError::Persist(aos_node::PersistError::backend(err.to_string()))
        }
        HostedVaultError::Invalid(message) => ControlError::invalid(message),
        HostedVaultError::NotFound(message) => ControlError::not_found(message),
        HostedVaultError::Poisoned => ControlError::Persist(aos_node::PersistError::backend(
            "hosted vault mutex poisoned",
        )),
    }
}

pub(crate) fn control_error_from_materializer(error: MaterializerStoreError) -> ControlError {
    match error {
        MaterializerStoreError::Cbor(err) => err.into(),
        MaterializerStoreError::Json(err) => err.into(),
        other => ControlError::invalid(other.to_string()),
    }
}

fn map_hosted_runtime_to_world_runtime(info: HostedWorldRuntimeResponse) -> WorldRuntimeInfo {
    WorldRuntimeInfo {
        world_id: info.world_id,
        universe_id: info.universe_id,
        created_at_ns: 0,
        manifest_hash: None,
        active_baseline_height: None,
        notify_counter: info.notify_counter,
        has_pending_inbox: info.has_pending_inbox,
        has_pending_effects: info.has_pending_effects,
        next_timer_due_at_ns: info.next_timer_due_at_ns,
        has_pending_maintenance: info.has_pending_maintenance,
    }
}

fn map_world_runtime(
    info: WorldRuntimeInfo,
    universe_id: aos_node::UniverseId,
) -> HostedWorldRuntimeResponse {
    HostedWorldRuntimeResponse {
        world_id: info.world_id,
        universe_id,
        notify_counter: info.notify_counter,
        has_pending_inbox: info.has_pending_inbox,
        has_pending_effects: info.has_pending_effects,
        next_timer_due_at_ns: info.next_timer_due_at_ns,
        has_pending_maintenance: info.has_pending_maintenance,
    }
}

fn map_materialized_world_runtime(summary: MaterializedWorldRow) -> HostedWorldRuntimeResponse {
    map_world_runtime(
        WorldRuntimeInfo {
            world_id: summary.world_id,
            universe_id: summary.universe_id,
            created_at_ns: 0,
            manifest_hash: Some(summary.manifest_hash),
            active_baseline_height: Some(summary.active_baseline.height),
            notify_counter: 0,
            has_pending_inbox: false,
            has_pending_effects: false,
            next_timer_due_at_ns: None,
            has_pending_maintenance: false,
        },
        summary.universe_id,
    )
}

fn map_state_cell_summary(cell: CellStateProjectionRecord) -> StateCellSummary {
    StateCellSummary {
        journal_head: cell.journal_head,
        workflow: cell.workflow,
        key_hash: cell.key_hash,
        key_bytes: cell.key_bytes,
        state_hash: cell.state_hash,
        size: cell.size,
        last_active_ns: cell.last_active_ns,
    }
}
