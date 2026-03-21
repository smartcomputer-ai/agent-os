use std::net::SocketAddr;
use std::sync::Arc;

use aos_air_types::AirNode;
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effect_types::{
    HashRef, WorkspaceAnnotationsGetReceipt, WorkspaceDiffReceipt, WorkspaceListReceipt,
    WorkspaceReadRefReceipt,
};
use aos_node::control::{
    BlobPutResponse, CasBlobMetadata, CommandSubmitResponse, ControlError, CreateUniverseBody,
    DefGetResponse, DefsListResponse, HeadInfoResponse, JournalEntriesResponse,
    JournalEntryResponse, ManifestResponse, NodeControl, PatchUniverseBody, PatchWorldBody,
    PutSecretBindingBody, PutSecretValueBody, RawJournalEntriesResponse, RawJournalEntryResponse,
    SecretPutResponse, ServiceInfoResponse, StateGetResponse, StateListResponse,
    UniverseSummaryResponse, WorkspaceApplyOp, WorkspaceApplyRequest, WorkspaceApplyResponse,
    WorkspaceResolveResponse, WorldSummaryResponse,
};
use aos_node::{
    CborPayload, CommandIngress, CommandRecord, CommandStatus, CommandStore, CreateUniverseRequest,
    CreateWorldRequest, CreateWorldSeedRequest, CreateWorldSource, DomainEventIngress,
    ForkWorldRequest, NodeCatalog, SecretAuditAction, SecretAuditRecord, SecretBindingRecord,
    SecretBindingStatus, SecretStore, UniverseCreateResult, UniverseId, UniverseStore,
    WorkerHeartbeat, WorldAdminStore, WorldId, WorldIngressStore, WorldLineage, WorldRuntimeInfo,
    WorldStore, default_world_handle, open_hosted_from_manifest_hash, snapshot_hosted_world,
};
use aos_runtime::{WorldConfig, now_wallclock_ns};
use aos_sqlite::{
    LocalSecretConfig, LocalSecretResolver, LocalSecretService, LocalStatePaths, SqliteNodeStore,
};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::supervisor::{LocalSupervisor, LocalSupervisorConfig};
use crate::workspace;

#[derive(Debug, Clone)]
pub struct LocalHttpConfig {
    pub bind_addr: SocketAddr,
}

impl Default for LocalHttpConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 9080)),
        }
    }
}

pub async fn serve(config: LocalHttpConfig, control: Arc<LocalControl>) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(
        bind = %local_addr,
        health = %format!("http://{local_addr}/v1/health"),
        roles = "supervisor,control",
        "aos-node-local listening"
    );
    axum::serve(listener, router(control)).await?;
    Ok(())
}

pub fn router(control: Arc<LocalControl>) -> Router {
    Router::new()
        .merge(aos_node::control::router::<LocalControl>())
        .route("/v1/universes/{universe_id}/workers", get(workers))
        .route(
            "/v1/universes/{universe_id}/workers/{worker_id}/worlds",
            get(worker_worlds),
        )
        .with_state(control)
}

#[derive(Clone)]
pub struct LocalControl {
    store: Arc<SqliteNodeStore>,
    supervisor: Arc<LocalSupervisor>,
    secret_config: LocalSecretConfig,
    paths: LocalStatePaths,
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
        let store = Arc::new(SqliteNodeStore::open_with_paths(&paths)?);
        let secret_config = LocalSecretConfig::from_env().map_err(ControlError::invalid)?;
        let supervisor = LocalSupervisor::new(
            Arc::clone(&store),
            LocalSupervisorConfig::default(),
            secret_config.clone(),
            state_root,
        );
        if start_supervisor {
            supervisor.start();
        }
        Ok(Arc::new(Self {
            store,
            supervisor,
            secret_config,
            paths,
        }))
    }

    pub fn local_universe_id(&self) -> UniverseId {
        self.store.local_universe_id()
    }

    pub fn step_world(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<WorldSummaryResponse, ControlError> {
        self.supervisor.run_ingress_once(universe, world)?;
        self.get_world(universe, world)
    }

    pub fn workers(
        &self,
        universe: UniverseId,
        limit: u32,
    ) -> Result<Vec<WorkerHeartbeat>, ControlError> {
        if universe != self.local_universe_id() {
            return Err(ControlError::not_found(format!("universe {universe}")));
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        Ok(vec![self.supervisor.worker_heartbeat(Self::WORKER_ID)])
    }

    pub fn worker_worlds(
        &self,
        universe: UniverseId,
        worker_id: &str,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, ControlError> {
        self.supervisor
            .worker_worlds(universe, worker_id, limit, Self::WORKER_ID)
            .map_err(ControlError::from)
    }

    fn hosted_store(&self, universe: UniverseId) -> aos_node::HostedStore {
        let persistence: Arc<dyn aos_node::WorldStore> = self.store.clone();
        aos_node::HostedStore::new(persistence, universe)
    }

    fn get_live_def(
        &self,
        universe: UniverseId,
        world: WorldId,
        name: &str,
    ) -> Result<(u64, String, AirNode), ControlError> {
        self.supervisor
            .def_get(universe, world, name)
            .map_err(ControlError::from)
    }

    fn create_world_from_manifest(
        &self,
        universe: UniverseId,
        requested_world_id: Option<WorldId>,
        requested_handle: Option<String>,
        placement_pin: Option<String>,
        created_at_ns: u64,
        manifest_hash: String,
    ) -> Result<aos_node::WorldCreateResult, ControlError> {
        let manifest_hash = parse_hash_like(&manifest_hash, "manifest_hash")?;
        if !self.store.cas_has(universe, manifest_hash)? {
            return Err(ControlError::not_found(format!(
                "manifest {} in universe {}",
                manifest_hash.to_hex(),
                universe
            )));
        }
        let world_id = requested_world_id.unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
        self.store.world_prepare_manifest_bootstrap(
            universe,
            world_id,
            manifest_hash,
            requested_handle.unwrap_or_else(|| default_world_handle(world_id)),
            placement_pin,
            created_at_ns,
            WorldLineage::Genesis { created_at_ns },
        )?;

        let persistence: Arc<dyn aos_node::WorldStore> = self.store.clone();
        let mut world_config = WorldConfig::from_env_with_fallback_module_cache_dir(Some(
            self.paths.wasmtime_cache_dir(),
        ));
        world_config.eager_module_load = true;
        let mut host = match open_hosted_from_manifest_hash(
            Arc::clone(&persistence),
            universe,
            world_id,
            manifest_hash,
            world_config,
            aos_effect_adapters::config::EffectAdapterConfig::default(),
            aos_kernel::KernelConfig {
                secret_resolver: Some(Arc::new(LocalSecretResolver::new(
                    Arc::clone(&self.store),
                    universe,
                    self.secret_config.clone(),
                ))),
                ..aos_kernel::KernelConfig::default()
            },
            None,
        ) {
            Ok(host) => host,
            Err(err) => {
                let _ = self.store.world_drop_manifest_bootstrap(universe, world_id);
                return Err(ControlError::invalid(err.to_string()));
            }
        };
        if let Err(err) = snapshot_hosted_world(&mut host, &persistence, universe, world_id) {
            let _ = self.store.world_drop_manifest_bootstrap(universe, world_id);
            return Err(ControlError::invalid(err.to_string()));
        }
        self.supervisor
            .ensure_hot(universe, world_id)
            .map_err(ControlError::from)?;
        Ok(aos_node::WorldCreateResult {
            record: aos_node::WorldRecord {
                world_id,
                meta: self.supervisor.runtime_info(universe, world_id)?.meta,
                active_baseline: self.store.snapshot_active_baseline(universe, world_id)?,
                journal_head: self.store.journal_head(universe, world_id)?,
            },
        })
    }
}

impl NodeControl for LocalControl {
    fn health(&self) -> Result<ServiceInfoResponse, ControlError> {
        Ok(ServiceInfoResponse {
            service: "aos-node-local",
            version: env!("CARGO_PKG_VERSION"),
        })
    }

    fn create_universe(
        &self,
        body: CreateUniverseBody,
    ) -> Result<UniverseCreateResult, ControlError> {
        Ok(self.store.create_universe(CreateUniverseRequest {
            universe_id: body.universe_id,
            handle: body.handle,
            created_at_ns: body.created_at_ns,
        })?)
    }

    fn get_universe(&self, universe: UniverseId) -> Result<UniverseSummaryResponse, ControlError> {
        Ok(UniverseSummaryResponse {
            record: self.store.get_universe(universe)?,
        })
    }

    fn get_universe_by_handle(
        &self,
        handle: &str,
    ) -> Result<UniverseSummaryResponse, ControlError> {
        Ok(UniverseSummaryResponse {
            record: self.store.get_universe_by_handle(handle)?,
        })
    }

    fn delete_universe(
        &self,
        universe: UniverseId,
    ) -> Result<UniverseSummaryResponse, ControlError> {
        Ok(UniverseSummaryResponse {
            record: self.store.delete_universe(universe, now_wallclock_ns())?,
        })
    }

    fn patch_universe(
        &self,
        universe: UniverseId,
        body: PatchUniverseBody,
    ) -> Result<UniverseSummaryResponse, ControlError> {
        if let Some(handle) = body.handle {
            return Ok(UniverseSummaryResponse {
                record: self.store.set_universe_handle(universe, handle)?,
            });
        }
        self.get_universe(universe)
    }

    fn list_universes(
        &self,
        after: Option<UniverseId>,
        limit: u32,
    ) -> Result<Vec<UniverseSummaryResponse>, ControlError> {
        Ok(self
            .store
            .list_universes(after, limit)?
            .into_iter()
            .map(|record| UniverseSummaryResponse { record })
            .collect())
    }

    fn list_secret_bindings(
        &self,
        universe: UniverseId,
        limit: u32,
    ) -> Result<Vec<SecretBindingRecord>, ControlError> {
        Ok(self.store.list_secret_bindings(universe, limit)?)
    }

    fn put_secret_binding(
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
        let record = self.store.put_secret_binding(
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
                    .store
                    .get_secret_binding(universe, &binding_id)?
                    .and_then(|existing| existing.latest_version),
                created_at_ns,
                updated_at_ns,
                status: body.status.unwrap_or(SecretBindingStatus::Active),
            },
        )?;
        self.store.append_secret_audit(
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

    fn get_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
    ) -> Result<SecretBindingRecord, ControlError> {
        let binding_id = normalize_required_string(binding_id, "binding_id")?;
        self.store
            .get_secret_binding(universe, &binding_id)?
            .ok_or_else(|| ControlError::not_found(format!("secret binding '{binding_id}'")))
    }

    fn delete_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
        actor: Option<String>,
    ) -> Result<SecretBindingRecord, ControlError> {
        let binding_id = normalize_required_string(binding_id, "binding_id")?;
        let updated_at_ns = now_wallclock_ns();
        let record = self
            .store
            .disable_secret_binding(universe, &binding_id, updated_at_ns)?;
        self.store.append_secret_audit(
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

    fn put_secret_value(
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
        let service = LocalSecretService::new(
            Arc::clone(&self.store),
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
        self.store.append_secret_audit(
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

    fn list_secret_versions(
        &self,
        universe: UniverseId,
        binding_id: &str,
        limit: u32,
    ) -> Result<Vec<aos_node::SecretVersionRecord>, ControlError> {
        let binding_id = normalize_required_string(binding_id, "binding_id")?;
        Ok(self
            .store
            .list_secret_versions(universe, &binding_id, limit)?)
    }

    fn get_secret_version(
        &self,
        universe: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<aos_node::SecretVersionRecord, ControlError> {
        let binding_id = normalize_required_string(binding_id, "binding_id")?;
        self.store
            .get_secret_version(universe, &binding_id, version)?
            .ok_or_else(|| {
                ControlError::not_found(format!("secret version '{binding_id}@{version}'"))
            })
    }

    fn list_worlds(
        &self,
        universe: UniverseId,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, ControlError> {
        self.supervisor
            .list_worlds(universe, after, limit)
            .map_err(ControlError::from)
    }

    fn create_world(
        &self,
        universe: UniverseId,
        request: CreateWorldRequest,
    ) -> Result<aos_node::WorldCreateResult, ControlError> {
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
            CreateWorldSource::Seed { seed } => {
                let result = self.store.world_create_from_seed(
                    universe,
                    CreateWorldSeedRequest {
                        world_id: request.world_id,
                        handle: request.handle,
                        seed,
                        placement_pin: request.placement_pin,
                        created_at_ns: request.created_at_ns,
                    },
                )?;
                self.supervisor
                    .ensure_hot(universe, result.record.world_id)
                    .map_err(ControlError::from)?;
                Ok(result)
            }
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

    fn get_world(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<WorldSummaryResponse, ControlError> {
        self.supervisor
            .world_summary(universe, world)
            .map_err(ControlError::from)
    }

    fn get_world_by_handle(
        &self,
        universe: UniverseId,
        handle: &str,
    ) -> Result<WorldSummaryResponse, ControlError> {
        let runtime =
            self.store
                .world_runtime_info_by_handle(universe, handle, now_wallclock_ns())?;
        self.get_world(universe, runtime.world_id)
    }

    fn patch_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        body: PatchWorldBody,
    ) -> Result<WorldSummaryResponse, ControlError> {
        if let Some(handle) = body.handle {
            self.store.set_world_handle(universe, world, handle)?;
        }
        if let Some(pin) = body.placement_pin {
            self.store.set_world_placement_pin(universe, world, pin)?;
        }
        self.get_world(universe, world)
    }

    fn fork_world(
        &self,
        universe: UniverseId,
        request: ForkWorldRequest,
    ) -> Result<aos_node::WorldForkResult, ControlError> {
        let result = self.store.world_fork(universe, request)?;
        self.supervisor
            .ensure_hot(universe, result.record.world_id)
            .map_err(ControlError::from)?;
        Ok(result)
    }

    fn get_command(
        &self,
        universe: UniverseId,
        world: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, ControlError> {
        let command_id = normalize_required_string(command_id, "command_id")?;
        self.store
            .command_record(universe, world, &command_id)?
            .ok_or_else(|| ControlError::not_found(format!("command '{command_id}'")))
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
        let command_id = normalize_optional_string(command_id, "command_id")?
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let actor = normalize_optional_string(actor, "actor")?;
        let command = normalize_required_string(command, "command")?;
        let submitted_at_ns = now_wallclock_ns();
        let payload = CborPayload::inline(to_canonical_cbor(payload)?);
        let ingress = CommandIngress {
            command_id: command_id.clone(),
            command: command.clone(),
            actor,
            payload,
            submitted_at_ns,
        };
        let record = self.store.submit_command(
            universe,
            world,
            ingress.clone(),
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

    fn archive_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        operation_id: Option<String>,
        reason: Option<String>,
    ) -> Result<WorldSummaryResponse, ControlError> {
        let now_ns = now_wallclock_ns();
        let mut admin = self
            .store
            .world_runtime_info(universe, world, now_ns)?
            .meta
            .admin;
        admin.status = aos_node::WorldAdminStatus::Archiving;
        admin.updated_at_ns = now_ns;
        admin.operation_id = operation_id;
        admin.reason = reason;
        self.store
            .set_world_admin_lifecycle(universe, world, admin)?;
        self.get_world(universe, world)
    }

    fn delete_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        operation_id: Option<String>,
        reason: Option<String>,
    ) -> Result<WorldSummaryResponse, ControlError> {
        let now_ns = now_wallclock_ns();
        let mut admin = self
            .store
            .world_runtime_info(universe, world, now_ns)?
            .meta
            .admin;
        admin.status = aos_node::WorldAdminStatus::Deleting;
        admin.updated_at_ns = now_ns;
        admin.operation_id = operation_id;
        admin.reason = reason;
        self.store
            .set_world_admin_lifecycle(universe, world, admin)?;
        self.get_world(universe, world)
    }

    fn manifest(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<ManifestResponse, ControlError> {
        let (head, manifest_hash, loaded) = self
            .supervisor
            .manifest(universe, world)
            .map_err(ControlError::from)?;
        Ok(ManifestResponse {
            journal_head: head,
            manifest_hash,
            manifest: loaded.manifest,
        })
    }

    fn defs_list(
        &self,
        universe: UniverseId,
        world: WorldId,
        kinds: Option<Vec<String>>,
        prefix: Option<String>,
    ) -> Result<DefsListResponse, ControlError> {
        let (journal_head, manifest_hash, defs) = self
            .supervisor
            .defs_list(universe, world, kinds.as_deref(), prefix.as_deref())
            .map_err(ControlError::from)?;
        Ok(DefsListResponse {
            journal_head,
            manifest_hash,
            defs,
        })
    }

    fn def_get(
        &self,
        universe: UniverseId,
        world: WorldId,
        kind: &str,
        name: &str,
    ) -> Result<DefGetResponse, ControlError> {
        let (journal_head, manifest_hash, def) = self.get_live_def(universe, world, name)?;
        if !def_matches_kind(&def, kind) {
            return Err(ControlError::not_found(format!(
                "definition '{name}' with kind '{kind}'"
            )));
        }
        Ok(DefGetResponse {
            journal_head,
            manifest_hash,
            def,
        })
    }

    fn state_get(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        key: Option<Vec<u8>>,
        consistency: Option<&str>,
    ) -> Result<StateGetResponse, ControlError> {
        require_latest_durable(consistency)?;
        self.supervisor
            .state_get(universe, world, workflow, key)
            .map_err(ControlError::from)
    }

    fn state_list(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        limit: u32,
        consistency: Option<&str>,
    ) -> Result<StateListResponse, ControlError> {
        require_latest_durable(consistency)?;
        self.supervisor
            .state_list(universe, world, workflow, limit)
            .map_err(ControlError::from)
    }

    fn enqueue_event(
        &self,
        universe: UniverseId,
        world: WorldId,
        ingress: DomainEventIngress,
    ) -> Result<aos_node::InboxSeq, ControlError> {
        let seq = self.store.enqueue_ingress(
            universe,
            world,
            aos_node::InboxItem::DomainEvent(ingress),
        )?;
        Ok(seq)
    }

    fn enqueue_receipt(
        &self,
        universe: UniverseId,
        world: WorldId,
        ingress: aos_node::ReceiptIngress,
    ) -> Result<aos_node::InboxSeq, ControlError> {
        let seq =
            self.store
                .enqueue_ingress(universe, world, aos_node::InboxItem::Receipt(ingress))?;
        Ok(seq)
    }

    fn journal_head(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<HeadInfoResponse, ControlError> {
        let runtime = self
            .supervisor
            .runtime_info(universe, world)
            .map_err(ControlError::from)?;
        Ok(HeadInfoResponse {
            journal_head: self.store.journal_head(universe, world)?,
            manifest_hash: runtime.meta.manifest_hash,
        })
    }

    fn journal_entries(
        &self,
        universe: UniverseId,
        world: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<JournalEntriesResponse, ControlError> {
        let rows = self
            .store
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

    fn journal_entries_raw(
        &self,
        universe: UniverseId,
        world: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<RawJournalEntriesResponse, ControlError> {
        let rows = self
            .store
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

    fn runtime(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<WorldRuntimeInfo, ControlError> {
        self.supervisor
            .runtime_info(universe, world)
            .map_err(ControlError::from)
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
        self.supervisor
            .trace(
                universe,
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

    fn trace_summary(
        &self,
        universe: UniverseId,
        world: WorldId,
        _recent_limit: u32,
    ) -> Result<serde_json::Value, ControlError> {
        self.supervisor
            .trace_summary(universe, world)
            .map_err(ControlError::from)
    }

    fn workspace_resolve(
        &self,
        universe: UniverseId,
        world: WorldId,
        workspace_name: &str,
        version: Option<u64>,
    ) -> Result<WorkspaceResolveResponse, ControlError> {
        self.supervisor
            .workspace_resolve(universe, world, workspace_name, version)
            .map_err(ControlError::from)
    }

    fn workspace_empty_root(&self, universe: UniverseId) -> Result<HashRef, ControlError> {
        let store = self.hosted_store(universe);
        Ok(workspace::empty_root(&store)?)
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
        let store = self.hosted_store(universe);
        Ok(workspace::list(
            &store, root_hash, path, scope, cursor, limit,
        )?)
    }

    fn workspace_entry(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: &str,
    ) -> Result<WorkspaceReadRefReceipt, ControlError> {
        let store = self.hosted_store(universe);
        Ok(workspace::read_ref(&store, root_hash, path)?)
    }

    fn workspace_bytes(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Vec<u8>, ControlError> {
        let store = self.hosted_store(universe);
        Ok(workspace::read_bytes(&store, root_hash, path, range)?)
    }

    fn workspace_annotations(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: Option<&str>,
    ) -> Result<WorkspaceAnnotationsGetReceipt, ControlError> {
        let store = self.hosted_store(universe);
        Ok(workspace::annotations_get(&store, root_hash, path)?)
    }

    fn workspace_apply(
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

    fn workspace_diff(
        &self,
        universe: UniverseId,
        root_a: &HashRef,
        root_b: &HashRef,
        prefix: Option<&str>,
    ) -> Result<WorkspaceDiffReceipt, ControlError> {
        let store = self.hosted_store(universe);
        Ok(workspace::diff(&store, root_a, root_b, prefix)?)
    }

    fn put_blob(
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
        let hash = self.store.cas_put_verified(universe, bytes)?;
        Ok(BlobPutResponse {
            hash: hash.to_hex(),
        })
    }

    fn head_blob(&self, universe: UniverseId, hash: Hash) -> Result<CasBlobMetadata, ControlError> {
        Ok(CasBlobMetadata {
            hash: hash.to_hex(),
            exists: self.store.cas_has(universe, hash)?,
        })
    }

    fn get_blob(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, ControlError> {
        Ok(self.store.cas_get(universe, hash)?)
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

fn parse_hash_like(value: &str, field: &str) -> Result<Hash, ControlError> {
    let trimmed = value.trim();
    let normalized = if trimmed.starts_with("sha256:") {
        trimmed.to_string()
    } else {
        format!("sha256:{trimmed}")
    };
    Hash::from_hex_str(&normalized)
        .map_err(|err| ControlError::invalid(format!("invalid {field} '{value}': {err}")))
}

#[derive(Debug, Deserialize)]
struct LimitQuery {
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    100
}

async fn workers(
    State(control): State<Arc<LocalControl>>,
    Path(universe_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.workers(
        aos_node::control::parse_universe_id(&universe_id)?,
        query.limit,
    )?))
}

async fn worker_worlds(
    State(control): State<Arc<LocalControl>>,
    Path((universe_id, worker_id)): Path<(String, String)>,
    Query(query): Query<LimitQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.worker_worlds(
        aos_node::control::parse_universe_id(&universe_id)?,
        &worker_id,
        query.limit,
    )?))
}

fn def_matches_kind(node: &AirNode, kind: &str) -> bool {
    let normalized = kind.trim();
    matches!(
        (normalized, node),
        ("caps" | "cap", AirNode::Defcap(_))
            | ("effects" | "effect", AirNode::Defeffect(_))
            | ("policies" | "policy", AirNode::Defpolicy(_))
            | ("schemas" | "schema", AirNode::Defschema(_))
            | ("modules" | "module", AirNode::Defmodule(_))
            | ("manifests" | "manifest", AirNode::Manifest(_))
            | ("secrets" | "secret", AirNode::Defsecret(_))
    )
}
