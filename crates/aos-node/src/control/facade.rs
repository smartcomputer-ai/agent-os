use aos_air_types::AirNode;
use aos_cbor::Hash;
use aos_effect_types::{
    GovApplyParams, GovApproveParams, GovProposeParams, GovShadowParams, HashRef,
    WorkspaceAnnotationsGetReceipt, WorkspaceDiffReceipt, WorkspaceListReceipt,
    WorkspaceReadRefReceipt,
};
use aos_kernel::TraceQuery;
use aos_node::control::{
    AcceptWaitQuery, BlobPutResponse, CasBlobMetadata, CommandSubmitBody, CommandSubmitResponse,
    ControlError, CreateWorldBody, DefGetResponse, DefsListResponse, DefsQuery, ForkWorldBody,
    HeadInfoResponse, HttpBackend, JournalEntriesResponse, JournalQuery, LimitQuery,
    ManifestResponse, PutSecretVersionBody, RawJournalEntriesResponse, ServiceInfoResponse,
    StateGetQuery, StateGetResponse, StateListResponse, SubmitEventBody, UpsertSecretBindingBody,
    WorkspaceAnnotationsQuery, WorkspaceApplyRequest, WorkspaceApplyResponse, WorkspaceBytesQuery,
    WorkspaceDiffBody, WorkspaceEntriesQuery, WorkspaceEntryQuery, WorkspaceResolveQuery,
    WorkspaceResolveResponse,
};
use aos_node::{
    CommandRecord, CreateWorldRequest, DomainEventIngress, ForkWorldRequest, ReceiptIngress,
    SecretBindingRecord, SecretVersionRecord, SnapshotRecord, UniverseId, WorldId,
    WorldRuntimeInfo,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Serialize;

use crate::bootstrap::ControlDeps;
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
            service: "aos-node",
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
        self.create_world_with_wait(AcceptWaitQuery::default(), body)
    }

    pub fn create_world_with_wait(
        &self,
        wait: AcceptWaitQuery,
        body: CreateWorldBody,
    ) -> Result<CreateWorldAccepted, ControlError> {
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
                wait,
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
                self.deps.submissions.create_world(
                    request.universe_id,
                    request,
                    AcceptWaitQuery::default(),
                )
            })
            .map_err(control_error_from_worker)
    }

    pub fn list_worlds(&self) -> Result<Vec<HostedWorldRuntimeResponse>, ControlError> {
        self.list_worlds_page(None, u32::MAX)
    }

    pub fn list_worlds_page(
        &self,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<HostedWorldRuntimeResponse>, ControlError> {
        let universe_id = self.default_universe_id()?;
        let runtime = self.runtime_for_reads()?;
        let mut worlds = runtime
            .list_worlds(universe_id)
            .map_err(control_error_from_worker)?
            .into_iter()
            .filter(|world| after.is_none_or(|after| world.world_id > after))
            .map(|world| runtime.runtime_info(universe_id, world.world_id))
            .collect::<Result<Vec<_>, _>>()
            .map_err(control_error_from_worker)?;
        worlds.sort_by_key(|world| world.world_id);
        worlds.truncate(limit as usize);
        Ok(worlds
            .into_iter()
            .map(|world| map_world_runtime(world.clone(), world.universe_id))
            .collect())
    }

    pub fn get_world(&self, world_id: WorldId) -> Result<HostedWorldSummaryResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        let runtime = self.runtime_for_reads()?;
        let runtime_info = runtime
            .runtime_info(universe_id, world_id)
            .map_err(control_error_from_worker)?;
        let active_baseline = runtime
            .active_baseline(universe_id, world_id)
            .map_err(control_error_from_worker)?;
        Ok(HostedWorldSummaryResponse {
            runtime: map_world_runtime(runtime_info, universe_id),
            active_baseline,
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
        self.submit_event_with_wait(world_id, AcceptWaitQuery::default(), body)
    }

    pub fn submit_event_with_wait(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: SubmitEventBody,
    ) -> Result<crate::worker::SubmissionAccepted, ControlError> {
        if body.value_b64.is_some() {
            return Err(ControlError::invalid(
                "node control does not yet accept value_b64 event payloads",
            ));
        }
        if body.key_b64.is_some() {
            return Err(ControlError::invalid(
                "node control does not yet accept keyed event ingress",
            ));
        }
        let value = body
            .value
            .or(body.value_json)
            .ok_or_else(|| ControlError::invalid("missing event value"))?;
        let universe_id = self.default_universe_id()?;
        self.deps
            .submissions
            .submit_event(
                SubmitEventRequest {
                    universe_id,
                    world_id,
                    schema: body.schema,
                    value,
                    submission_id: body.submission_id,
                    expected_world_epoch: body.expected_world_epoch,
                },
                wait,
            )
            .map_err(control_error_from_worker)
    }

    pub fn submit_receipt(
        &self,
        world_id: WorldId,
        body: ReceiptIngress,
    ) -> Result<crate::worker::SubmissionAccepted, ControlError> {
        self.submit_receipt_with_wait(world_id, AcceptWaitQuery::default(), body)
    }

    pub fn submit_receipt_with_wait(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: ReceiptIngress,
    ) -> Result<crate::worker::SubmissionAccepted, ControlError> {
        let universe_id = self.default_universe_id()?;
        body.payload.validate()?;
        self.deps
            .submissions
            .submit_receipt(universe_id, world_id, body, wait)
            .map_err(control_error_from_worker)
    }

    pub fn enqueue_event(
        &self,
        world_id: WorldId,
        ingress: DomainEventIngress,
    ) -> Result<crate::worker::SubmissionAccepted, ControlError> {
        let value = ingress.value.inline_cbor.ok_or_else(|| {
            ControlError::invalid("node control currently requires inline event payloads")
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
        self.submit_command_with_wait(world_id, command, AcceptWaitQuery::default(), body)
    }

    pub fn submit_command_with_wait<T: Serialize>(
        &self,
        world_id: WorldId,
        command: &str,
        wait: AcceptWaitQuery,
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
                wait,
            )
            .map_err(control_error_from_worker)
    }

    pub fn governance_propose(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: CommandSubmitBody<GovProposeParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        self.submit_command_with_wait(world_id, "gov-propose", wait, body)
    }

    pub fn governance_shadow(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: CommandSubmitBody<GovShadowParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        self.submit_command_with_wait(world_id, "gov-shadow", wait, body)
    }

    pub fn governance_approve(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: CommandSubmitBody<GovApproveParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        self.submit_command_with_wait(world_id, "gov-approve", wait, body)
    }

    pub fn governance_apply(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: CommandSubmitBody<GovApplyParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        self.submit_command_with_wait(world_id, "gov-apply", wait, body)
    }

    pub fn manifest(&self, world_id: WorldId) -> Result<ManifestResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.runtime_for_reads()?
            .manifest(universe_id, world_id)
            .map_err(control_error_from_worker)
    }

    pub fn defs_list(
        &self,
        world_id: WorldId,
        query: DefsQuery,
    ) -> Result<DefsListResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        let kinds = query.kinds.map(|raw| {
            raw.split(',')
                .filter(|kind| !kind.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        });
        self.runtime_for_reads()?
            .defs_list(universe_id, world_id, kinds, query.prefix)
            .map_err(control_error_from_worker)
    }

    pub fn def_get(
        &self,
        world_id: WorldId,
        kind: &str,
        name: &str,
    ) -> Result<DefGetResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        let response = self
            .runtime_for_reads()?
            .def_get(universe_id, world_id, name)
            .map_err(control_error_from_worker)?;
        if !def_matches_kind(&response.def, kind) {
            return Err(ControlError::not_found(format!(
                "definition '{name}' with kind '{kind}'"
            )));
        }
        Ok(response)
    }

    pub fn state_get(
        &self,
        world_id: WorldId,
        workflow: &str,
        query: StateGetQuery,
    ) -> Result<StateGetResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        require_hot_read_consistency(query.consistency.as_deref())?;
        let key = query.key_b64.map(|value| decode_b64(&value)).transpose()?;
        self.runtime_for_reads()?
            .state_get(universe_id, world_id, workflow, key)
            .map_err(control_error_from_worker)
    }

    pub fn state_list(
        &self,
        world_id: WorldId,
        workflow: &str,
        query: LimitQuery,
    ) -> Result<StateListResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        require_hot_read_consistency(query.consistency.as_deref())?;
        self.runtime_for_reads()?
            .state_list(universe_id, world_id, workflow, query.limit)
            .map_err(control_error_from_worker)
    }

    pub fn journal_head(&self, world_id: WorldId) -> Result<HeadInfoResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.runtime_for_reads()?
            .journal_head(universe_id, world_id)
            .map_err(control_error_from_worker)
    }

    pub fn journal_entries(
        &self,
        world_id: WorldId,
        query: JournalQuery,
    ) -> Result<JournalEntriesResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.runtime_for_reads()?
            .journal_entries(universe_id, world_id, query.from, query.limit)
            .map_err(control_error_from_worker)
    }

    pub fn journal_entries_raw(
        &self,
        world_id: WorldId,
        query: JournalQuery,
    ) -> Result<RawJournalEntriesResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.runtime_for_reads()?
            .journal_entries_raw(universe_id, world_id, query.from, query.limit)
            .map_err(control_error_from_worker)
    }

    pub fn runtime(&self, world_id: WorldId) -> Result<HostedWorldRuntimeResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        let world = self
            .runtime_for_reads()?
            .runtime_info(universe_id, world_id)
            .map_err(control_error_from_worker)?;
        Ok(map_world_runtime(world.clone(), world.universe_id))
    }

    pub fn workspace_resolve(
        &self,
        world_id: WorldId,
        query: WorkspaceResolveQuery,
    ) -> Result<WorkspaceResolveResponse, ControlError> {
        let universe_id = self.default_universe_id()?;
        self.runtime_for_reads()?
            .workspace_resolve(universe_id, world_id, &query.workspace, query.version)
            .map_err(control_error_from_worker)
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
                aos_node::control::WorkspaceApplyOp::WriteBytes {
                    path,
                    bytes_b64,
                    mode,
                } => {
                    let bytes = decode_b64(&bytes_b64)?;
                    workspace::write_bytes(store.as_ref(), &current, &path, &bytes, mode)?
                        .new_root_hash
                }
                aos_node::control::WorkspaceApplyOp::WriteRef {
                    path,
                    blob_hash,
                    mode,
                } => {
                    workspace::write_ref(store.as_ref(), &current, &path, &blob_hash, mode)?
                        .new_root_hash
                }
                aos_node::control::WorkspaceApplyOp::Remove { path } => {
                    workspace::remove(store.as_ref(), &current, &path)?.new_root_hash
                }
                aos_node::control::WorkspaceApplyOp::SetAnnotations {
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
    fn runtime_for_reads(&self) -> Result<&crate::worker::HostedWorkerRuntime, ControlError> {
        Ok(&self.deps.runtime)
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
        wait: AcceptWaitQuery,
        body: CreateWorldBody,
    ) -> Result<Self::CreateWorldResponse, ControlError> {
        ControlFacade::create_world_with_wait(self, wait, body)
    }

    fn get_world(
        &self,
        world_id: WorldId,
    ) -> Result<aos_node::control::WorldSummaryResponse, ControlError> {
        let summary = ControlFacade::get_world(self, world_id)?;
        Ok(aos_node::control::WorldSummaryResponse {
            runtime: map_hosted_runtime_to_world_runtime(summary.runtime),
            active_baseline: summary.active_baseline,
        })
    }

    fn checkpoint_world(
        &self,
        world_id: WorldId,
    ) -> Result<aos_node::control::WorldSummaryResponse, ControlError> {
        let summary = self.get_world(world_id)?;
        Ok(aos_node::control::WorldSummaryResponse {
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
        wait: AcceptWaitQuery,
        body: SubmitEventBody,
    ) -> Result<Self::SubmitEventResponse, ControlError> {
        ControlFacade::submit_event_with_wait(self, world_id, wait, body)
    }

    fn submit_receipt(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: ReceiptIngress,
    ) -> Result<Self::SubmitReceiptResponse, ControlError> {
        ControlFacade::submit_receipt_with_wait(self, world_id, wait, body)
    }

    fn governance_propose(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: CommandSubmitBody<GovProposeParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        ControlFacade::governance_propose(self, world_id, wait, body)
    }

    fn governance_shadow(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: CommandSubmitBody<GovShadowParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        ControlFacade::governance_shadow(self, world_id, wait, body)
    }

    fn governance_approve(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: CommandSubmitBody<GovApproveParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        ControlFacade::governance_approve(self, world_id, wait, body)
    }

    fn governance_apply(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: CommandSubmitBody<GovApplyParams>,
    ) -> Result<CommandSubmitResponse, ControlError> {
        ControlFacade::governance_apply(self, world_id, wait, body)
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

fn decode_b64(value: &str) -> Result<Vec<u8>, ControlError> {
    BASE64_STANDARD
        .decode(value)
        .map_err(|err| ControlError::invalid(format!("invalid base64: {err}")))
}

fn require_hot_read_consistency(consistency: Option<&str>) -> Result<(), ControlError> {
    match consistency {
        None | Some("latest") => Ok(()),
        Some("latest_durable") => Err(ControlError::invalid(
            "node runtime does not provide latest_durable reads; use latest hot reads or wait_for_flush on ingress",
        )),
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
        WorkerError::WaitForFlushTimedOut {
            accept_token,
            timeout_ms,
        } => ControlError::timeout(format!(
            "accept token {accept_token} did not durably flush within {timeout_ms}ms"
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
        HostedVaultError::Poisoned => {
            ControlError::Persist(aos_node::PersistError::backend("node vault mutex poisoned"))
        }
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
