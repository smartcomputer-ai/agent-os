use std::sync::Arc;

use aos_air_types::{AirNode, DefCap, DefEffect, DefPolicy};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_effect_types::{
    HashRef, WorkspaceAnnotationsGetReceipt, WorkspaceDiffReceipt, WorkspaceListReceipt,
    WorkspaceReadRefReceipt, WorkspaceResolveReceipt,
};
use aos_fdb::{
    CborPayload, CommandIngress, CommandRecord, CommandStatus, CreateUniverseRequest,
    CreateWorldRequest, CreateWorldSeedRequest, CreateWorldSource, DomainEventIngress,
    HostedRuntimeStore, InboxItem, ReceiptIngress, SecretAuditAction, SecretAuditRecord,
    SecretBindingRecord, SecretBindingSourceKind, SecretBindingStatus, SecretStore,
    SecretVersionRecord, UniverseCreateResult, UniverseId, UniverseStore, WorldAdminStore,
    WorldCreateResult, WorldId, WorldLineage, WorldRuntimeInfo, materialization_from_snapshot,
    state_blobs_from_snapshot,
};
use aos_kernel::{DefListing, LoadedManifest, ManifestLoader};
use aos_node::control::{
    BlobPutResponse, CasBlobMetadata, CommandSubmitResponse, ControlError, CreateUniverseBody,
    DefGetResponse, DefsListResponse, HeadInfoResponse, JournalEntriesResponse,
    JournalEntryResponse, ManifestResponse, NodeControl, PatchUniverseBody, PatchWorldBody,
    PutSecretBindingBody, PutSecretValueBody, RawJournalEntriesResponse, RawJournalEntryResponse,
    SecretPutResponse, ServiceInfoResponse, StateGetResponse, StateListResponse,
    UniverseSummaryResponse, WorkspaceApplyOp, WorkspaceApplyRequest, WorkspaceApplyResponse,
    WorkspaceResolveResponse, WorldSummaryResponse,
};
use aos_node::{HostedStore, open_hosted_from_manifest_hash, sync_hosted_snapshot_state};
use aos_runtime::{WorldConfig, now_wallclock_ns};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Serialize;
use uuid::Uuid;

use crate::control::{trace, workspace};
use crate::secret::{HostedSecretConfig, HostedSecretResolver, HostedSecretService};

#[derive(Clone)]
pub struct ControlFacade<P> {
    persistence: Arc<P>,
    secret_config: HostedSecretConfig,
}

impl<P> ControlFacade<P> {
    pub fn new(persistence: Arc<P>) -> Self {
        let secret_config = HostedSecretConfig::from_env().unwrap_or_default();
        Self {
            persistence,
            secret_config,
        }
    }

    pub fn with_secret_config(persistence: Arc<P>, secret_config: HostedSecretConfig) -> Self {
        Self {
            persistence,
            secret_config,
        }
    }

    pub fn persistence(&self) -> &Arc<P> {
        &self.persistence
    }
}

impl<P> ControlFacade<P>
where
    P: HostedRuntimeStore + WorldAdminStore + UniverseStore + 'static,
{
    pub fn health(&self) -> Result<ServiceInfoResponse, ControlError> {
        Ok(ServiceInfoResponse {
            service: "aos-node-hosted",
            version: env!("CARGO_PKG_VERSION"),
        })
    }

    pub fn create_universe(
        &self,
        body: CreateUniverseBody,
    ) -> Result<UniverseCreateResult, ControlError> {
        Ok(self.persistence.create_universe(CreateUniverseRequest {
            universe_id: body.universe_id,
            handle: body.handle,
            created_at_ns: body.created_at_ns,
        })?)
    }

    pub fn get_universe(
        &self,
        universe: UniverseId,
    ) -> Result<UniverseSummaryResponse, ControlError> {
        Ok(UniverseSummaryResponse {
            record: self.persistence.get_universe(universe)?,
        })
    }

    pub fn get_universe_by_handle(
        &self,
        handle: &str,
    ) -> Result<UniverseSummaryResponse, ControlError> {
        Ok(UniverseSummaryResponse {
            record: self.persistence.get_universe_by_handle(handle)?,
        })
    }

    pub fn delete_universe(
        &self,
        universe: UniverseId,
    ) -> Result<UniverseSummaryResponse, ControlError> {
        Ok(UniverseSummaryResponse {
            record: self
                .persistence
                .delete_universe(universe, now_wallclock_ns())?,
        })
    }

    pub fn patch_universe(
        &self,
        universe: UniverseId,
        body: PatchUniverseBody,
    ) -> Result<UniverseSummaryResponse, ControlError> {
        if let Some(handle) = body.handle {
            return Ok(UniverseSummaryResponse {
                record: self.persistence.set_universe_handle(universe, handle)?,
            });
        }
        self.get_universe(universe)
    }

    pub fn list_universes(
        &self,
        after: Option<UniverseId>,
        limit: u32,
    ) -> Result<Vec<UniverseSummaryResponse>, ControlError> {
        Ok(self
            .persistence
            .list_universes(after, limit)?
            .into_iter()
            .map(|record| UniverseSummaryResponse { record })
            .collect())
    }

    pub fn fork_world(
        &self,
        universe: UniverseId,
        request: aos_fdb::ForkWorldRequest,
    ) -> Result<aos_fdb::WorldForkResult, ControlError> {
        Ok(self.persistence.world_fork(universe, request)?)
    }

    pub fn get_world(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<WorldSummaryResponse, ControlError> {
        let now_ns = now_wallclock_ns();
        Ok(WorldSummaryResponse {
            runtime: self
                .persistence
                .world_runtime_info(universe, world, now_ns)?,
            active_baseline: self.persistence.snapshot_active_baseline(universe, world)?,
        })
    }

    pub fn get_world_by_handle(
        &self,
        universe: UniverseId,
        handle: &str,
    ) -> Result<WorldSummaryResponse, ControlError> {
        let now_ns = now_wallclock_ns();
        let runtime = self
            .persistence
            .world_runtime_info_by_handle(universe, handle, now_ns)?;
        Ok(WorldSummaryResponse {
            active_baseline: self
                .persistence
                .snapshot_active_baseline(universe, runtime.world_id)?,
            runtime,
        })
    }

    pub fn list_worlds(
        &self,
        universe: UniverseId,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, ControlError> {
        Ok(self
            .persistence
            .list_worlds(universe, now_wallclock_ns(), after, limit)?)
    }

    pub fn patch_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        body: PatchWorldBody,
    ) -> Result<WorldSummaryResponse, ControlError> {
        if let Some(handle) = body.handle {
            self.persistence.set_world_handle(universe, world, handle)?;
        }
        if let Some(pin) = body.placement_pin {
            self.persistence
                .set_world_placement_pin(universe, world, pin)?;
        }
        self.get_world(universe, world)
    }

    pub fn delete_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        operation_id: Option<String>,
        reason: Option<String>,
    ) -> Result<WorldSummaryResponse, ControlError> {
        self.transition_world_admin(
            universe,
            world,
            aos_fdb::WorldAdminStatus::Deleting,
            operation_id,
            reason,
        )
    }

    pub fn archive_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        operation_id: Option<String>,
        reason: Option<String>,
    ) -> Result<WorldSummaryResponse, ControlError> {
        self.transition_world_admin(
            universe,
            world,
            aos_fdb::WorldAdminStatus::Archiving,
            operation_id,
            reason,
        )
    }

    fn transition_world_admin(
        &self,
        universe: UniverseId,
        world: WorldId,
        target_status: aos_fdb::WorldAdminStatus,
        operation_id: Option<String>,
        reason: Option<String>,
    ) -> Result<WorldSummaryResponse, ControlError> {
        let now_ns = now_wallclock_ns();
        let mut admin = self
            .persistence
            .world_runtime_info(universe, world, now_ns)?
            .meta
            .admin;
        admin.status = target_status;
        admin.updated_at_ns = now_ns;
        admin.operation_id = operation_id;
        admin.reason = reason;
        self.persistence
            .set_world_admin_lifecycle(universe, world, admin)?;
        self.get_world(universe, world)
    }
}

impl<P> ControlFacade<P>
where
    P: HostedRuntimeStore + SecretStore + WorldAdminStore + UniverseStore + 'static,
{
    pub fn create_world(
        &self,
        universe: UniverseId,
        request: CreateWorldRequest,
    ) -> Result<WorldCreateResult, ControlError> {
        if request
            .placement_pin
            .as_ref()
            .is_some_and(|pin| pin.trim().is_empty())
        {
            return Err(ControlError::invalid(
                "placement_pin must be non-empty when provided",
            ));
        }
        match request.source {
            CreateWorldSource::Seed { seed } => Ok(self.persistence.world_create_from_seed(
                universe,
                CreateWorldSeedRequest {
                    world_id: request.world_id,
                    handle: request.handle,
                    seed,
                    placement_pin: request.placement_pin,
                    created_at_ns: request.created_at_ns,
                },
            )?),
            CreateWorldSource::Manifest { manifest_hash } => self.create_world_from_manifest(
                universe,
                request.world_id,
                request.handle,
                request.placement_pin,
                request.created_at_ns,
                manifest_hash,
            ),
        }
    }

    pub fn list_secret_bindings(
        &self,
        universe: UniverseId,
        limit: u32,
    ) -> Result<Vec<SecretBindingRecord>, ControlError> {
        Ok(self.persistence.list_secret_bindings(universe, limit)?)
    }

    pub fn put_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
        body: PutSecretBindingBody,
    ) -> Result<SecretBindingRecord, ControlError> {
        let binding_id = normalize_required_string(binding_id, "binding_id")?;
        let created_at_ns = if body.created_at_ns == 0 {
            now_wallclock_ns()
        } else {
            body.created_at_ns
        };
        let updated_at_ns = if body.updated_at_ns == 0 {
            created_at_ns
        } else {
            body.updated_at_ns
        };
        let record = self.persistence.put_secret_binding(
            universe,
            SecretBindingRecord {
                binding_id: binding_id.clone(),
                source_kind: body.source_kind,
                env_var: normalize_optional_string(body.env_var, "env_var")?,
                required_placement_pin: normalize_optional_string(
                    body.required_placement_pin,
                    "required_placement_pin",
                )?,
                latest_version: self
                    .persistence
                    .get_secret_binding(universe, &binding_id)?
                    .and_then(|existing| existing.latest_version),
                created_at_ns,
                updated_at_ns,
                status: body.status.unwrap_or(SecretBindingStatus::Active),
            },
        )?;
        self.persistence.append_secret_audit(
            universe,
            SecretAuditRecord {
                ts_ns: updated_at_ns,
                action: SecretAuditAction::BindingUpserted,
                binding_id,
                version: None,
                digest: None,
                actor: body.actor,
            },
        )?;
        Ok(record)
    }

    pub fn get_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
    ) -> Result<SecretBindingRecord, ControlError> {
        let binding_id = normalize_required_string(binding_id, "binding_id")?;
        self.persistence
            .get_secret_binding(universe, &binding_id)?
            .ok_or_else(|| ControlError::not_found(format!("secret binding '{binding_id}'")))
    }

    pub fn delete_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
        actor: Option<String>,
    ) -> Result<SecretBindingRecord, ControlError> {
        let binding_id = normalize_required_string(binding_id, "binding_id")?;
        let updated_at_ns = now_wallclock_ns();
        let record =
            self.persistence
                .disable_secret_binding(universe, &binding_id, updated_at_ns)?;
        self.persistence.append_secret_audit(
            universe,
            SecretAuditRecord {
                ts_ns: updated_at_ns,
                action: SecretAuditAction::BindingDisabled,
                binding_id,
                version: None,
                digest: None,
                actor,
            },
        )?;
        Ok(record)
    }

    pub fn put_secret_value(
        &self,
        universe: UniverseId,
        binding_id: &str,
        body: PutSecretValueBody,
    ) -> Result<SecretPutResponse, ControlError> {
        let binding = self.get_secret_binding(universe, binding_id)?;
        let plaintext = BASE64_STANDARD
            .decode(body.plaintext_b64)
            .map_err(|err| ControlError::invalid(format!("invalid plaintext_b64: {err}")))?;
        let now_ns = if body.created_at_ns == 0 {
            now_wallclock_ns()
        } else {
            body.created_at_ns
        };
        let service = HostedSecretService::new(
            Arc::clone(&self.persistence),
            universe,
            self.secret_config.clone(),
        );
        let result = service
            .put_secret_value(
                &binding,
                &plaintext,
                body.expected_digest.as_deref(),
                body.actor.clone(),
                now_ns,
            )
            .map_err(ControlError::invalid)?;
        self.persistence.append_secret_audit(
            universe,
            SecretAuditRecord {
                ts_ns: now_ns,
                action: SecretAuditAction::VersionPut,
                binding_id: binding.binding_id.clone(),
                version: Some(result.version.version),
                digest: Some(result.version.digest.clone()),
                actor: body.actor,
            },
        )?;
        Ok(SecretPutResponse {
            binding_id: binding.binding_id,
            version: result.version.version,
            digest: result.version.digest,
        })
    }

    pub fn list_secret_versions(
        &self,
        universe: UniverseId,
        binding_id: &str,
        limit: u32,
    ) -> Result<Vec<SecretVersionRecord>, ControlError> {
        let binding_id = normalize_required_string(binding_id, "binding_id")?;
        Ok(self
            .persistence
            .list_secret_versions(universe, &binding_id, limit)?)
    }

    pub fn get_secret_version(
        &self,
        universe: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<SecretVersionRecord, ControlError> {
        let binding_id = normalize_required_string(binding_id, "binding_id")?;
        self.persistence
            .get_secret_version(universe, &binding_id, version)?
            .ok_or_else(|| {
                ControlError::not_found(format!("secret version '{binding_id}@{version}'"))
            })
    }
}

impl<P> ControlFacade<P>
where
    P: HostedRuntimeStore + WorldAdminStore + UniverseStore + 'static,
{
    pub fn manifest(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<ManifestResponse, ControlError> {
        let (head, manifest) = self.load_manifest(universe, world)?;
        Ok(ManifestResponse {
            journal_head: head.journal_head,
            manifest_hash: head.manifest_hash,
            manifest: manifest.manifest,
        })
    }

    pub fn defs_list(
        &self,
        universe: UniverseId,
        world: WorldId,
        kinds: Option<Vec<String>>,
        prefix: Option<String>,
    ) -> Result<DefsListResponse, ControlError> {
        let (head, manifest) = self.load_manifest(universe, world)?;
        Ok(DefsListResponse {
            journal_head: head.journal_head,
            manifest_hash: head.manifest_hash,
            defs: list_defs_from_manifest(&manifest, kinds.as_deref(), prefix.as_deref()),
        })
    }

    pub fn def_get(
        &self,
        universe: UniverseId,
        world: WorldId,
        kind: &str,
        name: &str,
    ) -> Result<DefGetResponse, ControlError> {
        let (head, manifest) = self.load_manifest(universe, world)?;
        let def = get_def_from_manifest(&manifest, name)
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
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        key: Option<Vec<u8>>,
        consistency: Option<&str>,
    ) -> Result<StateGetResponse, ControlError> {
        require_latest_durable(consistency)?;
        let head = self.require_head_projection(universe, world)?;
        let key_bytes = key.unwrap_or_default();
        let key_hash = Hash::of_bytes(&key_bytes);
        let cell = self.persistence.cell_state_projection(
            universe,
            world,
            workflow,
            key_hash.as_bytes(),
        )?;
        let state_b64 = match &cell {
            Some(cell) => {
                let state_hash = parse_hex_hash(&cell.state_hash)?;
                Some(BASE64_STANDARD.encode(self.persistence.cas_get(universe, state_hash)?))
            }
            None => None,
        };
        Ok(StateGetResponse {
            journal_head: head.journal_head,
            workflow: workflow.to_string(),
            key_b64: Some(BASE64_STANDARD.encode(key_bytes)),
            cell,
            state_b64,
        })
    }

    pub fn state_list(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        limit: u32,
        consistency: Option<&str>,
    ) -> Result<StateListResponse, ControlError> {
        require_latest_durable(consistency)?;
        let head = self.require_head_projection(universe, world)?;
        let cells = self
            .persistence
            .list_cell_state_projections(universe, world, workflow, None, limit)?;
        Ok(StateListResponse {
            journal_head: head.journal_head,
            workflow: workflow.to_string(),
            cells,
        })
    }

    pub fn enqueue_event(
        &self,
        universe: UniverseId,
        world: WorldId,
        ingress: DomainEventIngress,
    ) -> Result<aos_fdb::InboxSeq, ControlError> {
        Ok(self
            .persistence
            .enqueue_ingress(universe, world, InboxItem::DomainEvent(ingress))?)
    }

    pub fn enqueue_receipt(
        &self,
        universe: UniverseId,
        world: WorldId,
        ingress: ReceiptIngress,
    ) -> Result<aos_fdb::InboxSeq, ControlError> {
        Ok(self
            .persistence
            .enqueue_ingress(universe, world, InboxItem::Receipt(ingress))?)
    }

    pub fn submit_command<T: Serialize>(
        &self,
        universe: UniverseId,
        world: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        payload: &T,
    ) -> Result<CommandSubmitResponse, ControlError> {
        let command_id = normalize_optional_string(command_id, "command_id")?
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let actor = normalize_optional_string(actor, "actor")?;
        let command = normalize_required_string(command, "command")?;
        let submitted_at_ns = now_wallclock_ns();
        let payload = CborPayload::inline(to_canonical_cbor(payload)?);
        let record = self.persistence.submit_command(
            universe,
            world,
            CommandIngress {
                command_id: command_id.clone(),
                command: command.clone(),
                actor,
                payload,
                submitted_at_ns,
            },
            CommandRecord {
                command_id: command_id.clone(),
                command,
                status: CommandStatus::Queued,
                submitted_at_ns,
                started_at_ns: None,
                finished_at_ns: None,
                journal_height: None,
                manifest_hash: None,
                result_payload: None,
                error: None,
            },
        )?;
        Ok(CommandSubmitResponse {
            poll_url: format!("/v1/universes/{universe}/worlds/{world}/commands/{command_id}"),
            command_id: record.command_id,
            status: record.status,
        })
    }

    pub fn get_command(
        &self,
        universe: UniverseId,
        world: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, ControlError> {
        let command_id = normalize_required_string(command_id, "command_id")?;
        self.persistence
            .command_record(universe, world, &command_id)?
            .ok_or_else(|| ControlError::not_found(format!("command '{command_id}'")))
    }

    pub fn journal_head(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<HeadInfoResponse, ControlError> {
        let journal_head = self.persistence.journal_head(universe, world)?;
        let manifest_hash = self
            .persistence
            .head_projection(universe, world)?
            .map(|record| record.manifest_hash);
        Ok(HeadInfoResponse {
            journal_head,
            manifest_hash,
        })
    }

    pub fn journal_entries(
        &self,
        universe: UniverseId,
        world: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<JournalEntriesResponse, ControlError> {
        let rows = self
            .persistence
            .journal_read_range(universe, world, from, limit)?;
        let mut entries = Vec::with_capacity(rows.len());
        let mut next_from = from;
        for (seq, raw) in rows {
            let entry: aos_kernel::journal::OwnedJournalEntry = serde_cbor::from_slice(&raw)?;
            let record_value = serde_cbor::from_slice::<serde_cbor::Value>(&entry.payload)
                .ok()
                .and_then(|value| serde_json::to_value(value).ok())
                .unwrap_or_else(
                    || serde_json::json!({ "payload_b64": BASE64_STANDARD.encode(&entry.payload) }),
                );
            entries.push(JournalEntryResponse {
                seq,
                kind: format!("{:?}", entry.kind).to_lowercase(),
                record: record_value,
            });
            next_from = seq.saturating_add(1);
        }
        Ok(JournalEntriesResponse {
            from,
            next_from,
            entries,
        })
    }

    pub fn journal_entries_raw(
        &self,
        universe: UniverseId,
        world: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<RawJournalEntriesResponse, ControlError> {
        let rows = self
            .persistence
            .journal_read_range(universe, world, from, limit)?;
        let mut entries = Vec::with_capacity(rows.len());
        let mut next_from = from;
        for (seq, raw) in rows {
            entries.push(RawJournalEntryResponse {
                seq,
                entry_cbor: raw,
            });
            next_from = seq.saturating_add(1);
        }
        Ok(RawJournalEntriesResponse {
            from,
            next_from,
            entries,
        })
    }

    pub fn runtime(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<WorldRuntimeInfo, ControlError> {
        Ok(self
            .persistence
            .world_runtime_info(universe, world, now_wallclock_ns())?)
    }

    pub fn trace(
        &self,
        universe: UniverseId,
        world: WorldId,
        event_hash: Option<&str>,
        schema: Option<&str>,
        correlate_by: Option<&str>,
        correlate_value: Option<serde_json::Value>,
        window_limit: Option<u64>,
    ) -> Result<serde_json::Value, ControlError> {
        trace::build_trace(
            &self.persistence,
            universe,
            world,
            trace::TraceQuery {
                event_hash: event_hash.map(ToOwned::to_owned),
                schema: schema.map(ToOwned::to_owned),
                correlate_by: correlate_by.map(ToOwned::to_owned),
                correlate_value,
                window_limit,
            },
        )
    }

    pub fn trace_summary(
        &self,
        universe: UniverseId,
        world: WorldId,
        recent_limit: u32,
    ) -> Result<serde_json::Value, ControlError> {
        trace::build_trace_summary(&self.persistence, universe, world, recent_limit)
    }

    pub fn workers(
        &self,
        _universe: UniverseId,
        limit: u32,
    ) -> Result<Vec<aos_fdb::WorkerHeartbeat>, ControlError> {
        Ok(self
            .persistence
            .list_active_workers(now_wallclock_ns(), limit)?)
    }

    pub fn worker_worlds(
        &self,
        universe: UniverseId,
        worker_id: &str,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, ControlError> {
        Ok(self
            .persistence
            .list_worker_worlds(worker_id, now_wallclock_ns(), limit, Some(&[universe]))?
            .into_iter()
            .map(|entry| entry.info)
            .collect())
    }

    pub fn workspace_resolve(
        &self,
        universe: UniverseId,
        world: WorldId,
        workspace_name: &str,
        version: Option<u64>,
    ) -> Result<WorkspaceResolveResponse, ControlError> {
        let projection = self
            .persistence
            .workspace_projection(universe, world, workspace_name)?;
        let receipt = if let Some(projection) = projection {
            let head = Some(projection.latest_version);
            let target = version.unwrap_or(projection.latest_version);
            if let Some(entry) = projection.versions.get(&target) {
                WorkspaceResolveReceipt {
                    exists: true,
                    resolved_version: Some(target),
                    head,
                    root_hash: Some(HashRef::new(entry.root_hash.clone())?),
                }
            } else {
                WorkspaceResolveReceipt {
                    exists: false,
                    resolved_version: None,
                    head,
                    root_hash: None,
                }
            }
        } else {
            WorkspaceResolveReceipt {
                exists: false,
                resolved_version: None,
                head: None,
                root_hash: None,
            }
        };
        Ok(WorkspaceResolveResponse {
            workspace: workspace_name.to_string(),
            receipt,
        })
    }

    pub fn workspace_empty_root(&self, universe: UniverseId) -> Result<HashRef, ControlError> {
        let store = self.hosted_store(universe);
        Ok(workspace::empty_root(&store)?)
    }

    pub fn workspace_entries(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: Option<&str>,
        scope: Option<&str>,
        cursor: Option<&str>,
        limit: u64,
    ) -> Result<WorkspaceListReceipt, ControlError> {
        let store = self.hosted_store(universe);
        Ok(workspace::list(
            &store, root_hash, path, scope, cursor, limit,
        )?)
    }

    pub fn workspace_entry(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: &str,
    ) -> Result<WorkspaceReadRefReceipt, ControlError> {
        let store = self.hosted_store(universe);
        Ok(workspace::read_ref(&store, root_hash, path)?)
    }

    pub fn workspace_bytes(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Vec<u8>, ControlError> {
        let store = self.hosted_store(universe);
        Ok(workspace::read_bytes(&store, root_hash, path, range)?)
    }

    pub fn workspace_annotations(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: Option<&str>,
    ) -> Result<WorkspaceAnnotationsGetReceipt, ControlError> {
        let store = self.hosted_store(universe);
        Ok(workspace::annotations_get(&store, root_hash, path)?)
    }

    pub fn workspace_apply(
        &self,
        universe: UniverseId,
        root_hash: HashRef,
        request: WorkspaceApplyRequest,
    ) -> Result<WorkspaceApplyResponse, ControlError> {
        let store = self.hosted_store(universe);
        let base_root_hash = root_hash.clone();
        let mut current_root = root_hash;
        for op in request.operations {
            current_root = match op {
                WorkspaceApplyOp::WriteBytes {
                    path,
                    bytes_b64,
                    mode,
                } => {
                    let bytes = BASE64_STANDARD
                        .decode(bytes_b64)
                        .map_err(|err| ControlError::invalid(format!("invalid base64: {err}")))?;
                    workspace::write_bytes(&store, &current_root, &path, &bytes, mode)?
                        .new_root_hash
                }
                WorkspaceApplyOp::WriteRef {
                    path,
                    blob_hash,
                    mode,
                } => {
                    workspace::write_ref(&store, &current_root, &path, &blob_hash, mode)?
                        .new_root_hash
                }
                WorkspaceApplyOp::Remove { path } => {
                    workspace::remove(&store, &current_root, &path)?.new_root_hash
                }
                WorkspaceApplyOp::SetAnnotations {
                    path,
                    annotations_patch,
                } => {
                    workspace::annotations_set(
                        &store,
                        &current_root,
                        path.as_deref(),
                        &annotations_patch,
                    )?
                    .new_root_hash
                }
            };
        }
        Ok(WorkspaceApplyResponse {
            base_root_hash,
            new_root_hash: current_root,
        })
    }

    pub fn workspace_diff(
        &self,
        universe: UniverseId,
        root_a: &HashRef,
        root_b: &HashRef,
        prefix: Option<&str>,
    ) -> Result<WorkspaceDiffReceipt, ControlError> {
        let store = self.hosted_store(universe);
        Ok(workspace::diff(&store, root_a, root_b, prefix)?)
    }

    pub fn put_blob(
        &self,
        universe: UniverseId,
        bytes: &[u8],
        expected_hash: Option<Hash>,
    ) -> Result<BlobPutResponse, ControlError> {
        if let Some(expected_hash) = expected_hash {
            let actual = Hash::of_bytes(bytes);
            if actual != expected_hash {
                return Err(ControlError::invalid(format!(
                    "blob hash mismatch: expected {}, got {}",
                    expected_hash.to_hex(),
                    actual.to_hex()
                )));
            }
        }
        let hash = self.persistence.cas_put_verified(universe, bytes)?;
        Ok(BlobPutResponse {
            hash: hash.to_hex(),
        })
    }

    pub fn head_blob(
        &self,
        universe: UniverseId,
        hash: Hash,
    ) -> Result<CasBlobMetadata, ControlError> {
        Ok(CasBlobMetadata {
            hash: hash.to_hex(),
            exists: self.persistence.cas_has(universe, hash)?,
        })
    }

    pub fn get_blob(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, ControlError> {
        Ok(self.persistence.cas_get(universe, hash)?)
    }

    fn hosted_store(&self, universe: UniverseId) -> HostedStore {
        let persistence: Arc<dyn aos_fdb::WorldStore> = self.persistence.clone();
        HostedStore::new(persistence, universe)
    }

    fn require_head_projection(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<aos_fdb::HeadProjectionRecord, ControlError> {
        self.persistence
            .head_projection(universe, world)?
            .ok_or_else(|| ControlError::not_found(format!("head projection for world {world}")))
    }

    fn load_manifest(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<(aos_fdb::HeadProjectionRecord, LoadedManifest), ControlError> {
        let head = self.require_head_projection(universe, world)?;
        let hash = parse_hex_hash(&head.manifest_hash)?;
        let store = self.hosted_store(universe);
        let manifest = ManifestLoader::load_from_hash(&store, hash)?;
        Ok((head, manifest))
    }
}

impl<P> ControlFacade<P>
where
    P: HostedRuntimeStore + SecretStore + WorldAdminStore + UniverseStore + 'static,
{
    fn create_world_from_manifest(
        &self,
        universe: UniverseId,
        requested_world_id: Option<WorldId>,
        requested_handle: Option<String>,
        placement_pin: Option<String>,
        created_at_ns: u64,
        manifest_hash: String,
    ) -> Result<WorldCreateResult, ControlError> {
        let manifest_hash =
            parse_hash_like(&manifest_hash, "manifest_hash").map_err(ControlError::invalid)?;
        if !self.persistence.cas_has(universe, manifest_hash)? {
            return Err(ControlError::not_found(format!(
                "manifest {} in universe {}",
                manifest_hash.to_hex(),
                universe
            )));
        }
        let store = self.hosted_store(universe);
        let loaded = ManifestLoader::load_from_hash(&store, manifest_hash)?;
        self.preflight_manifest_secrets(universe, &loaded)?;

        let world_id = requested_world_id.unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
        self.persistence.world_prepare_manifest_bootstrap(
            universe,
            world_id,
            manifest_hash,
            requested_handle.unwrap_or_else(|| aos_fdb::default_world_handle(world_id)),
            placement_pin,
            created_at_ns,
            WorldLineage::Genesis { created_at_ns },
        )?;

        let open_result = open_hosted_from_manifest_hash(
            self.persistence.clone(),
            universe,
            world_id,
            manifest_hash,
            WorldConfig::default(),
            EffectAdapterConfig::default(),
            aos_kernel::KernelConfig {
                secret_resolver: Some(Arc::new(HostedSecretResolver::new(
                    Arc::clone(&self.persistence),
                    universe,
                    self.secret_config.clone(),
                ))),
                ..aos_kernel::KernelConfig::default()
            },
            None,
        );
        let mut host = match open_result {
            Ok(host) => host,
            Err(err) => {
                let _ = self
                    .persistence
                    .world_drop_manifest_bootstrap(universe, world_id);
                return Err(ControlError::invalid(err.to_string()));
            }
        };
        let persistence_world: Arc<dyn aos_fdb::WorldStore> = self.persistence.clone();
        if let Err(err) =
            sync_hosted_snapshot_state(&mut host, &persistence_world, universe, world_id)
        {
            let _ = self
                .persistence
                .world_drop_manifest_bootstrap(universe, world_id);
            return Err(ControlError::invalid(err.to_string()));
        }

        let active_baseline = self
            .persistence
            .snapshot_active_baseline(universe, world_id)?;
        let snapshot_hash = parse_hash_like(&active_baseline.snapshot_ref, "snapshot_ref")
            .map_err(ControlError::invalid)?;
        let snapshot_bytes = self.persistence.cas_get(universe, snapshot_hash)?;
        for (state_hash, state_bytes) in state_blobs_from_snapshot(&snapshot_bytes)? {
            let stored = self.persistence.cas_put_verified(universe, &state_bytes)?;
            if stored != state_hash {
                return Err(ControlError::invalid(format!(
                    "snapshot state hash mismatch: expected {}, stored {}",
                    state_hash.to_hex(),
                    stored.to_hex()
                )));
            }
        }
        let materialization =
            materialization_from_snapshot(&active_baseline, &snapshot_bytes, now_wallclock_ns())?;
        self.persistence
            .bootstrap_query_projections(universe, world_id, materialization)?;

        let runtime =
            self.persistence
                .world_runtime_info(universe, world_id, now_wallclock_ns())?;
        Ok(WorldCreateResult {
            record: aos_fdb::WorldRecord {
                world_id,
                journal_head: active_baseline.height,
                meta: runtime.meta,
                active_baseline,
            },
        })
    }

    fn preflight_manifest_secrets(
        &self,
        universe: UniverseId,
        loaded: &LoadedManifest,
    ) -> Result<(), ControlError> {
        for secret in &loaded.secrets {
            let binding = self
                .persistence
                .get_secret_binding(universe, &secret.binding_id)?
                .ok_or_else(|| {
                    ControlError::invalid(format!("secret_binding_missing: {}", secret.binding_id))
                })?;
            if !matches!(binding.status, SecretBindingStatus::Active) {
                return Err(ControlError::invalid(format!(
                    "secret_binding_disabled: {}",
                    secret.binding_id
                )));
            }
            match binding.source_kind {
                SecretBindingSourceKind::NodeSecretStore => {
                    self.persistence
                        .get_secret_version(universe, &secret.binding_id, secret.version)?
                        .ok_or_else(|| {
                            ControlError::invalid(format!(
                                "secret_version_missing: {}@{}",
                                secret.binding_id, secret.version
                            ))
                        })?;
                }
                SecretBindingSourceKind::WorkerEnv => {}
            }
        }
        Ok(())
    }
}

fn parse_hash_like(value: &str, field: &str) -> Result<Hash, String> {
    let trimmed = value.trim();
    let normalized = if trimmed.starts_with("sha256:") {
        trimmed.to_string()
    } else {
        format!("sha256:{trimmed}")
    };
    Hash::from_hex_str(&normalized).map_err(|err| format!("invalid {field} '{value}': {err}"))
}

fn parse_hex_hash(value: &str) -> Result<Hash, ControlError> {
    Hash::from_hex_str(value).map_err(|err| ControlError::invalid(format!("invalid hash: {err}")))
}

impl<P> NodeControl for ControlFacade<P>
where
    P: HostedRuntimeStore + SecretStore + WorldAdminStore + UniverseStore + 'static,
{
    fn health(&self) -> Result<ServiceInfoResponse, ControlError> {
        ControlFacade::health(self)
    }

    fn create_universe(
        &self,
        body: CreateUniverseBody,
    ) -> Result<UniverseCreateResult, ControlError> {
        ControlFacade::create_universe(self, body)
    }

    fn get_universe(&self, universe: UniverseId) -> Result<UniverseSummaryResponse, ControlError> {
        ControlFacade::get_universe(self, universe)
    }

    fn get_universe_by_handle(
        &self,
        handle: &str,
    ) -> Result<UniverseSummaryResponse, ControlError> {
        ControlFacade::get_universe_by_handle(self, handle)
    }

    fn delete_universe(
        &self,
        universe: UniverseId,
    ) -> Result<UniverseSummaryResponse, ControlError> {
        ControlFacade::delete_universe(self, universe)
    }

    fn patch_universe(
        &self,
        universe: UniverseId,
        body: PatchUniverseBody,
    ) -> Result<UniverseSummaryResponse, ControlError> {
        ControlFacade::patch_universe(self, universe, body)
    }

    fn list_universes(
        &self,
        after: Option<UniverseId>,
        limit: u32,
    ) -> Result<Vec<UniverseSummaryResponse>, ControlError> {
        ControlFacade::list_universes(self, after, limit)
    }

    fn list_secret_bindings(
        &self,
        universe: UniverseId,
        limit: u32,
    ) -> Result<Vec<SecretBindingRecord>, ControlError> {
        ControlFacade::list_secret_bindings(self, universe, limit)
    }

    fn put_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
        body: PutSecretBindingBody,
    ) -> Result<SecretBindingRecord, ControlError> {
        ControlFacade::put_secret_binding(self, universe, binding_id, body)
    }

    fn get_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
    ) -> Result<SecretBindingRecord, ControlError> {
        ControlFacade::get_secret_binding(self, universe, binding_id)
    }

    fn delete_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
        actor: Option<String>,
    ) -> Result<SecretBindingRecord, ControlError> {
        ControlFacade::delete_secret_binding(self, universe, binding_id, actor)
    }

    fn put_secret_value(
        &self,
        universe: UniverseId,
        binding_id: &str,
        body: PutSecretValueBody,
    ) -> Result<SecretPutResponse, ControlError> {
        ControlFacade::put_secret_value(self, universe, binding_id, body)
    }

    fn list_secret_versions(
        &self,
        universe: UniverseId,
        binding_id: &str,
        limit: u32,
    ) -> Result<Vec<SecretVersionRecord>, ControlError> {
        ControlFacade::list_secret_versions(self, universe, binding_id, limit)
    }

    fn get_secret_version(
        &self,
        universe: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<SecretVersionRecord, ControlError> {
        ControlFacade::get_secret_version(self, universe, binding_id, version)
    }

    fn list_worlds(
        &self,
        universe: UniverseId,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, ControlError> {
        ControlFacade::list_worlds(self, universe, after, limit)
    }

    fn create_world(
        &self,
        universe: UniverseId,
        request: CreateWorldRequest,
    ) -> Result<WorldCreateResult, ControlError> {
        ControlFacade::create_world(self, universe, request)
    }

    fn get_world(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<WorldSummaryResponse, ControlError> {
        ControlFacade::get_world(self, universe, world)
    }

    fn get_world_by_handle(
        &self,
        universe: UniverseId,
        handle: &str,
    ) -> Result<WorldSummaryResponse, ControlError> {
        ControlFacade::get_world_by_handle(self, universe, handle)
    }

    fn patch_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        body: PatchWorldBody,
    ) -> Result<WorldSummaryResponse, ControlError> {
        ControlFacade::patch_world(self, universe, world, body)
    }

    fn fork_world(
        &self,
        universe: UniverseId,
        request: aos_fdb::ForkWorldRequest,
    ) -> Result<aos_fdb::WorldForkResult, ControlError> {
        ControlFacade::fork_world(self, universe, request)
    }

    fn get_command(
        &self,
        universe: UniverseId,
        world: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, ControlError> {
        ControlFacade::get_command(self, universe, world, command_id)
    }

    fn submit_command<T: Serialize>(
        &self,
        universe: UniverseId,
        world: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        payload: &T,
    ) -> Result<CommandSubmitResponse, ControlError> {
        ControlFacade::submit_command(self, universe, world, command, command_id, actor, payload)
    }

    fn archive_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        operation_id: Option<String>,
        reason: Option<String>,
    ) -> Result<WorldSummaryResponse, ControlError> {
        ControlFacade::archive_world(self, universe, world, operation_id, reason)
    }

    fn delete_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        operation_id: Option<String>,
        reason: Option<String>,
    ) -> Result<WorldSummaryResponse, ControlError> {
        ControlFacade::delete_world(self, universe, world, operation_id, reason)
    }

    fn manifest(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<ManifestResponse, ControlError> {
        ControlFacade::manifest(self, universe, world)
    }

    fn defs_list(
        &self,
        universe: UniverseId,
        world: WorldId,
        kinds: Option<Vec<String>>,
        prefix: Option<String>,
    ) -> Result<DefsListResponse, ControlError> {
        ControlFacade::defs_list(self, universe, world, kinds, prefix)
    }

    fn def_get(
        &self,
        universe: UniverseId,
        world: WorldId,
        kind: &str,
        name: &str,
    ) -> Result<DefGetResponse, ControlError> {
        ControlFacade::def_get(self, universe, world, kind, name)
    }

    fn state_get(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        key: Option<Vec<u8>>,
        consistency: Option<&str>,
    ) -> Result<StateGetResponse, ControlError> {
        ControlFacade::state_get(self, universe, world, workflow, key, consistency)
    }

    fn state_list(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        limit: u32,
        consistency: Option<&str>,
    ) -> Result<StateListResponse, ControlError> {
        ControlFacade::state_list(self, universe, world, workflow, limit, consistency)
    }

    fn enqueue_event(
        &self,
        universe: UniverseId,
        world: WorldId,
        ingress: DomainEventIngress,
    ) -> Result<aos_fdb::InboxSeq, ControlError> {
        ControlFacade::enqueue_event(self, universe, world, ingress)
    }

    fn enqueue_receipt(
        &self,
        universe: UniverseId,
        world: WorldId,
        ingress: ReceiptIngress,
    ) -> Result<aos_fdb::InboxSeq, ControlError> {
        ControlFacade::enqueue_receipt(self, universe, world, ingress)
    }

    fn journal_head(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<HeadInfoResponse, ControlError> {
        ControlFacade::journal_head(self, universe, world)
    }

    fn journal_entries(
        &self,
        universe: UniverseId,
        world: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<aos_node::control::JournalEntriesResponse, ControlError> {
        ControlFacade::journal_entries(self, universe, world, from, limit)
    }

    fn journal_entries_raw(
        &self,
        universe: UniverseId,
        world: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<aos_node::control::RawJournalEntriesResponse, ControlError> {
        ControlFacade::journal_entries_raw(self, universe, world, from, limit)
    }

    fn runtime(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<WorldRuntimeInfo, ControlError> {
        ControlFacade::runtime(self, universe, world)
    }

    fn trace(
        &self,
        universe: UniverseId,
        world: WorldId,
        event_hash: Option<&str>,
        schema: Option<&str>,
        correlate_by: Option<&str>,
        correlate_value: Option<serde_json::Value>,
        window_limit: Option<u64>,
    ) -> Result<serde_json::Value, ControlError> {
        ControlFacade::trace(
            self,
            universe,
            world,
            event_hash,
            schema,
            correlate_by,
            correlate_value,
            window_limit,
        )
    }

    fn trace_summary(
        &self,
        universe: UniverseId,
        world: WorldId,
        recent_limit: u32,
    ) -> Result<serde_json::Value, ControlError> {
        ControlFacade::trace_summary(self, universe, world, recent_limit)
    }

    fn workspace_resolve(
        &self,
        universe: UniverseId,
        world: WorldId,
        workspace_name: &str,
        version: Option<u64>,
    ) -> Result<WorkspaceResolveResponse, ControlError> {
        ControlFacade::workspace_resolve(self, universe, world, workspace_name, version)
    }

    fn workspace_empty_root(&self, universe: UniverseId) -> Result<HashRef, ControlError> {
        ControlFacade::workspace_empty_root(self, universe)
    }

    fn workspace_entries(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: Option<&str>,
        scope: Option<&str>,
        cursor: Option<&str>,
        limit: u64,
    ) -> Result<WorkspaceListReceipt, ControlError> {
        ControlFacade::workspace_entries(self, universe, root_hash, path, scope, cursor, limit)
    }

    fn workspace_entry(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: &str,
    ) -> Result<WorkspaceReadRefReceipt, ControlError> {
        ControlFacade::workspace_entry(self, universe, root_hash, path)
    }

    fn workspace_bytes(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Vec<u8>, ControlError> {
        ControlFacade::workspace_bytes(self, universe, root_hash, path, range)
    }

    fn workspace_annotations(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: Option<&str>,
    ) -> Result<WorkspaceAnnotationsGetReceipt, ControlError> {
        ControlFacade::workspace_annotations(self, universe, root_hash, path)
    }

    fn workspace_apply(
        &self,
        universe: UniverseId,
        root_hash: HashRef,
        request: WorkspaceApplyRequest,
    ) -> Result<WorkspaceApplyResponse, ControlError> {
        ControlFacade::workspace_apply(self, universe, root_hash, request)
    }

    fn workspace_diff(
        &self,
        universe: UniverseId,
        root_a: &HashRef,
        root_b: &HashRef,
        prefix: Option<&str>,
    ) -> Result<WorkspaceDiffReceipt, ControlError> {
        ControlFacade::workspace_diff(self, universe, root_a, root_b, prefix)
    }

    fn put_blob(
        &self,
        universe: UniverseId,
        bytes: &[u8],
        expected_hash: Option<Hash>,
    ) -> Result<BlobPutResponse, ControlError> {
        ControlFacade::put_blob(self, universe, bytes, expected_hash)
    }

    fn head_blob(&self, universe: UniverseId, hash: Hash) -> Result<CasBlobMetadata, ControlError> {
        ControlFacade::head_blob(self, universe, hash)
    }

    fn get_blob(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, ControlError> {
        ControlFacade::get_blob(self, universe, hash)
    }
}

fn normalize_required_string(value: &str, field: &str) -> Result<String, ControlError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ControlError::invalid(format!("{field} must be non-empty")));
    }
    Ok(trimmed.to_string())
}

fn normalize_optional_string(
    value: Option<String>,
    field: &str,
) -> Result<Option<String>, ControlError> {
    value
        .map(|value| normalize_required_string(&value, field))
        .transpose()
}

fn require_latest_durable(consistency: Option<&str>) -> Result<(), ControlError> {
    match consistency.unwrap_or("latest_durable") {
        "latest_durable" | "head" => Ok(()),
        other => Err(ControlError::invalid(format!(
            "unsupported consistency '{other}'"
        ))),
    }
}

fn def_matches_kind(node: &AirNode, kind: &str) -> bool {
    let Some(kind) = normalize_def_kind(kind) else {
        return false;
    };
    matches!(
        (kind, node),
        ("defschema", AirNode::Defschema(_))
            | ("defmodule", AirNode::Defmodule(_))
            | ("defcap", AirNode::Defcap(_))
            | ("defeffect", AirNode::Defeffect(_))
            | ("defpolicy", AirNode::Defpolicy(_))
    )
}

fn get_def_from_manifest(manifest: &LoadedManifest, name: &str) -> Option<AirNode> {
    if let Some(def) = manifest.schemas.get(name) {
        return Some(AirNode::Defschema(def.clone()));
    }
    if let Some(def) = manifest.modules.get(name) {
        return Some(AirNode::Defmodule(def.clone()));
    }
    if let Some(def) = manifest.caps.get(name) {
        return Some(AirNode::Defcap(def.clone()));
    }
    if let Some(def) = manifest.policies.get(name) {
        return Some(AirNode::Defpolicy(def.clone()));
    }
    manifest
        .effects
        .get(name)
        .map(|def| AirNode::Defeffect(def.clone()))
}

fn list_defs_from_manifest(
    manifest: &LoadedManifest,
    kinds: Option<&[String]>,
    prefix: Option<&str>,
) -> Vec<DefListing> {
    let prefix = prefix.unwrap_or("");
    let kind_filter = kinds.map(|raw| {
        raw.iter()
            .filter_map(|kind| normalize_def_kind(kind))
            .collect::<std::collections::HashSet<_>>()
    });
    let hash_def = |node: AirNode| Hash::of_cbor(&node).expect("hash def").to_hex();

    let mut defs = Vec::new();
    push_defs(
        &mut defs,
        "defschema",
        &manifest.schemas,
        &kind_filter,
        prefix,
        |name, def| DefListing {
            kind: "defschema".into(),
            name: name.clone(),
            hash: hash_def(AirNode::Defschema(def.clone())),
            cap_type: None,
            params_schema: None,
            receipt_schema: None,
            plan_steps: None,
            policy_rules: None,
        },
    );
    push_defs(
        &mut defs,
        "defmodule",
        &manifest.modules,
        &kind_filter,
        prefix,
        |name, def| DefListing {
            kind: "defmodule".into(),
            name: name.clone(),
            hash: hash_def(AirNode::Defmodule(def.clone())),
            cap_type: None,
            params_schema: None,
            receipt_schema: None,
            plan_steps: None,
            policy_rules: None,
        },
    );
    push_defs(
        &mut defs,
        "defcap",
        &manifest.caps,
        &kind_filter,
        prefix,
        |name, def: &DefCap| DefListing {
            kind: "defcap".into(),
            name: name.clone(),
            hash: hash_def(AirNode::Defcap(def.clone())),
            cap_type: Some(def.cap_type.to_string()),
            params_schema: None,
            receipt_schema: None,
            plan_steps: None,
            policy_rules: None,
        },
    );
    push_defs(
        &mut defs,
        "defeffect",
        &manifest.effects,
        &kind_filter,
        prefix,
        |name, def: &DefEffect| DefListing {
            kind: "defeffect".into(),
            name: name.clone(),
            hash: hash_def(AirNode::Defeffect(def.clone())),
            cap_type: Some(def.cap_type.to_string()),
            params_schema: Some(def.params_schema.to_string()),
            receipt_schema: Some(def.receipt_schema.to_string()),
            plan_steps: None,
            policy_rules: None,
        },
    );
    push_defs(
        &mut defs,
        "defpolicy",
        &manifest.policies,
        &kind_filter,
        prefix,
        |name, def: &DefPolicy| DefListing {
            kind: "defpolicy".into(),
            name: name.clone(),
            hash: hash_def(AirNode::Defpolicy(def.clone())),
            cap_type: None,
            params_schema: None,
            receipt_schema: None,
            plan_steps: None,
            policy_rules: Some(def.rules.len()),
        },
    );
    defs.sort_by(|a, b| a.name.cmp(&b.name));
    defs
}

fn push_defs<T, F>(
    out: &mut Vec<DefListing>,
    kind: &str,
    defs: &std::collections::HashMap<aos_air_types::Name, T>,
    filter: &Option<std::collections::HashSet<&'static str>>,
    prefix: &str,
    build: F,
) where
    F: Fn(&aos_air_types::Name, &T) -> DefListing,
{
    if !def_kind_allowed(kind, filter.as_ref()) {
        return;
    }
    for (name, def) in defs {
        if name.as_str().starts_with(prefix) {
            out.push(build(name, def));
        }
    }
}

fn def_kind_allowed(kind: &str, filter: Option<&std::collections::HashSet<&'static str>>) -> bool {
    filter.is_none_or(|filter| filter.contains(kind))
}

fn normalize_def_kind(input: &str) -> Option<&'static str> {
    match input {
        "schema" | "defschema" => Some("defschema"),
        "module" | "defmodule" => Some("defmodule"),
        "cap" | "defcap" => Some("defcap"),
        "effect" | "defeffect" => Some("defeffect"),
        "policy" | "defpolicy" => Some("defpolicy"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::def_matches_kind;
    use aos_air_types::{AirNode, DefModule, ModuleAbi, ModuleKind, SchemaRef, WorkflowAbi};
    use aos_cbor::Hash;
    use aos_effect_types::HashRef;
    use indexmap::IndexMap;

    #[test]
    fn def_matches_kind_accepts_friendly_aliases() {
        let module = AirNode::Defmodule(DefModule {
            name: "demo/Workflow@1".into(),
            module_kind: ModuleKind::Workflow,
            wasm_hash: HashRef::new(Hash::of_bytes(b"wasm").to_hex()).expect("hash ref"),
            key_schema: None,
            abi: ModuleAbi {
                workflow: Some(WorkflowAbi {
                    state: SchemaRef::new("demo/State@1").expect("state schema"),
                    event: SchemaRef::new("demo/Event@1").expect("event schema"),
                    context: None,
                    annotations: None,
                    effects_emitted: Vec::new(),
                    cap_slots: IndexMap::new(),
                }),
                pure: None,
            },
        });

        assert!(def_matches_kind(&module, "module"));
        assert!(def_matches_kind(&module, "defmodule"));
        assert!(!def_matches_kind(&module, "schema"));
    }
}
