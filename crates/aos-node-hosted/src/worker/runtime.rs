use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use aos_cbor::Hash;
use aos_effect_types::HashRef;
use aos_kernel::SharedSecretResolver;
use aos_kernel::{ManifestLoader, Store};
use aos_node::api::{
    DefGetResponse, DefsListResponse, ManifestResponse, StateCellSummary, StateGetResponse,
    StateListResponse, WorkspaceResolveResponse, WorldSummaryResponse,
};
use aos_node::{
    BlobPlane, CborPayload, CheckpointPlane, CommandIngress, CommandRecord, CommandStatus,
    CreateWorldRequest, ForkWorldRequest, FsCas, LocalStatePaths, ReceiptIngress,
    SubmissionEnvelope, SubmissionPayload, UniverseId, WorldId, WorldRuntimeInfo,
    partition_for_world,
};
use aos_runtime::trace::{TraceQuery as RuntimeTraceQuery, trace_get};
use aos_runtime::{WorldConfig, WorldHost};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::blobstore::{
    BlobStoreConfig, HostedBlobMetaStore, HostedCas, RemoteCasStore, scoped_blobstore_config,
};
use crate::kafka::{HostedKafkaBackend, KafkaConfig};
use crate::vault::HostedVault;

use super::commands::command_submit_response;
use super::types::{
    CreateWorldAccepted, HostedWorkerInfra, HostedWorkerRuntimeInner, HostedWorldSummary,
    SubmissionAccepted, SubmitEventRequest, WorkerError,
};
use super::util::{default_state_root, temp_embedded_state_root, unix_time_ns};

#[derive(Clone)]
pub struct HostedWorkerRuntime {
    inner: Arc<Mutex<HostedWorkerRuntimeInner>>,
    paths: Arc<LocalStatePaths>,
}

impl std::fmt::Debug for HostedWorkerRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedWorkerRuntime")
            .finish_non_exhaustive()
    }
}

impl HostedWorkerRuntime {
    pub fn new(partition_count: u32) -> Result<Self, WorkerError> {
        Self::new_with_state_root_and_universe(
            partition_count,
            default_state_root()?,
            aos_node::local_universe_id(),
        )
    }

    pub fn new_broker_with_state_root(
        partition_count: u32,
        state_root: impl Into<PathBuf>,
        kafka_config: KafkaConfig,
        blobstore_config: BlobStoreConfig,
    ) -> Result<Self, WorkerError> {
        Self::new_broker_with_state_root_and_universe(
            partition_count,
            state_root,
            aos_node::local_universe_id(),
            kafka_config,
            blobstore_config,
        )
    }

    pub fn new_broker_with_state_root_and_universe(
        partition_count: u32,
        state_root: impl Into<PathBuf>,
        default_universe_id: UniverseId,
        kafka_config: KafkaConfig,
        blobstore_config: BlobStoreConfig,
    ) -> Result<Self, WorkerError> {
        let paths = LocalStatePaths::new(state_root.into());
        paths.ensure_root().map_err(|err| {
            WorkerError::Persist(aos_node::PersistError::backend(err.to_string()))
        })?;
        let world_config = WorldConfig::from_env_with_fallback_module_cache_dir(None);
        let mut inner = HostedWorkerRuntimeInner {
            infra: super::types::HostedWorkerInfra {
                default_universe_id,
                paths: paths.clone(),
                blobstore_config: blobstore_config.clone(),
                vault: HostedVault::new(blobstore_config.clone()).map_err(|err| {
                    WorkerError::Build(anyhow::anyhow!("initialize hosted vault: {err}"))
                })?,
                world_config,
                kafka: HostedKafkaBackend::new(partition_count, kafka_config)?,
                stores_by_domain: BTreeMap::new(),
                blob_meta_by_domain: BTreeMap::new(),
            },
            state: super::types::HostedWorkerState::default(),
        };
        if inner.infra.kafka.is_broker()
            && !blobstore_config
                .bucket
                .as_ref()
                .is_some_and(|value: &String| !value.trim().is_empty())
        {
            return Err(WorkerError::Persist(aos_node::PersistError::validation(
                "broker-backed hosted runtime requires AOS_BLOBSTORE_BUCKET or legacy AOS_S3_BUCKET",
            )));
        }
        inner.bootstrap_recovery()?;
        if !inner
            .infra
            .kafka
            .config()
            .direct_assigned_partitions
            .is_empty()
        {
            let _ = inner.infra.kafka.sync_assignments_and_poll()?;
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
            paths: Arc::new(paths),
        })
    }

    pub fn new_with_state_root(
        partition_count: u32,
        state_root: impl Into<PathBuf>,
    ) -> Result<Self, WorkerError> {
        Self::new_with_state_root_and_universe(
            partition_count,
            state_root,
            aos_node::local_universe_id(),
        )
    }

    pub fn new_with_state_root_and_universe(
        partition_count: u32,
        state_root: impl Into<PathBuf>,
        default_universe_id: UniverseId,
    ) -> Result<Self, WorkerError> {
        Self::new_broker_with_state_root_and_universe(
            partition_count,
            state_root,
            default_universe_id,
            KafkaConfig::default(),
            BlobStoreConfig::default(),
        )
    }

    pub fn new_embedded(partition_count: u32) -> Result<Self, WorkerError> {
        Self::new_embedded_with_state_root_and_universe(
            partition_count,
            temp_embedded_state_root(),
            aos_node::local_universe_id(),
        )
    }

    pub fn new_embedded_with_state_root(
        partition_count: u32,
        state_root: impl Into<PathBuf>,
    ) -> Result<Self, WorkerError> {
        Self::new_embedded_with_state_root_and_universe(
            partition_count,
            state_root,
            aos_node::local_universe_id(),
        )
    }

    pub fn new_embedded_with_state_root_and_universe(
        partition_count: u32,
        state_root: impl Into<PathBuf>,
        default_universe_id: UniverseId,
    ) -> Result<Self, WorkerError> {
        let paths = LocalStatePaths::new(state_root.into());
        paths.ensure_root().map_err(|err| {
            WorkerError::Persist(aos_node::PersistError::backend(err.to_string()))
        })?;
        let world_config = WorldConfig::from_env_with_fallback_module_cache_dir(None);
        let mut inner = HostedWorkerRuntimeInner {
            infra: super::types::HostedWorkerInfra {
                default_universe_id,
                paths: paths.clone(),
                blobstore_config: BlobStoreConfig::default(),
                vault: HostedVault::new(BlobStoreConfig::default()).map_err(|err| {
                    WorkerError::Build(anyhow::anyhow!("initialize hosted vault: {err}"))
                })?,
                world_config,
                kafka: HostedKafkaBackend::new_embedded(partition_count, KafkaConfig::default())?,
                stores_by_domain: BTreeMap::new(),
                blob_meta_by_domain: BTreeMap::new(),
            },
            state: super::types::HostedWorkerState::default(),
        };
        inner.bootstrap_recovery()?;
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
            paths: Arc::new(paths),
        })
    }

    pub fn paths(&self) -> &LocalStatePaths {
        self.paths.as_ref()
    }

    pub fn default_universe_id(&self) -> Result<UniverseId, WorkerError> {
        Ok(self.lock_inner()?.infra.default_universe_id)
    }

    pub fn vault(&self) -> Result<HostedVault, WorkerError> {
        Ok(self.lock_inner()?.infra.vault.clone())
    }

    pub fn secret_resolver(
        &self,
        universe_id: UniverseId,
    ) -> Result<SharedSecretResolver, WorkerError> {
        let inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        Ok(Arc::new(
            inner.infra.vault.resolver_for_universe(universe_id),
        ))
    }

    pub fn create_world(
        &self,
        universe_id: UniverseId,
        request: CreateWorldRequest,
    ) -> Result<CreateWorldAccepted, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        let world_id = request
            .world_id
            .unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
        inner.submit_create_world(universe_id, world_id, request)
    }

    pub fn submit_submission(&self, submission: SubmissionEnvelope) -> Result<u64, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner
            .infra
            .kafka
            .submit(submission)
            .map_err(WorkerError::from)
    }

    pub fn seed_world(
        &self,
        universe_id: UniverseId,
        request: CreateWorldRequest,
        publish_checkpoint: bool,
    ) -> Result<HostedWorldSummary, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        let world_id = request
            .world_id
            .unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
        inner.seed_world_direct(universe_id, world_id, request, publish_checkpoint)?;
        inner
            .world_summary(universe_id, world_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })
    }

    pub fn fork_world(
        &self,
        universe_id: UniverseId,
        request: ForkWorldRequest,
    ) -> Result<CreateWorldAccepted, WorkerError> {
        aos_node::validate_fork_world_request(&request)?;
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        let world_id = request
            .new_world_id
            .unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
        let create_request = inner.create_fork_seed_request(universe_id, world_id, &request)?;
        inner.submit_create_world(universe_id, world_id, create_request)
    }

    pub fn fork_create_request(
        &self,
        universe_id: UniverseId,
        new_world_id: WorldId,
        request: &ForkWorldRequest,
    ) -> Result<CreateWorldRequest, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.create_fork_seed_request(universe_id, new_world_id, request)
    }

    pub fn list_worlds(&self) -> Result<Vec<HostedWorldSummary>, WorkerError> {
        let mut inner = self.lock_inner()?;
        let universe_id = inner.infra.default_universe_id;
        let world_ids = inner
            .infra
            .kafka
            .world_ids()
            .into_iter()
            .collect::<Vec<_>>();
        for world_id in &world_ids {
            let _ = inner.ensure_registered_world(universe_id, *world_id);
        }
        let mut worlds = world_ids
            .into_iter()
            .filter_map(|world_id| inner.world_summary(universe_id, world_id))
            .collect::<Vec<_>>();
        worlds.sort_by_key(|world| world.world_id);
        Ok(worlds)
    }

    pub fn get_world(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<HostedWorldSummary, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.ensure_registered_world(universe_id, world_id)?;
        inner
            .world_summary(universe_id, world_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })
    }

    pub fn get_command(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.ensure_registered_world(universe_id, world_id)?;
        let universe_id = inner
            .state
            .registered_worlds
            .get(&world_id)
            .map(|world| world.universe_id)
            .ok_or_else(|| WorkerError::UnknownCommand {
                universe_id,
                world_id,
                command_id: command_id.to_owned(),
            })?;
        inner
            .infra
            .blob_meta_for_domain_mut(universe_id)?
            .get_command_record(world_id, command_id)?
            .ok_or_else(|| WorkerError::UnknownCommand {
                universe_id,
                world_id,
                command_id: command_id.to_owned(),
            })
    }

    pub fn get_command_record(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<Option<CommandRecord>, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner
            .infra
            .blob_meta_for_domain_mut(universe_id)?
            .get_command_record(world_id, command_id)
            .map_err(WorkerError::from)
    }

    pub fn put_command_record(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        record: CommandRecord,
    ) -> Result<(), WorkerError> {
        let mut inner = self.lock_inner()?;
        inner
            .infra
            .blob_meta_for_domain_mut(universe_id)?
            .put_command_record(world_id, record)
            .map_err(WorkerError::from)
    }

    pub fn latest_checkpoint(
        &self,
        universe_id: UniverseId,
        partition: u32,
    ) -> Result<Option<aos_node::PartitionCheckpoint>, WorkerError> {
        let mut inner = self.lock_inner()?;
        let journal_topic = inner.infra.kafka.config().journal_topic.clone();
        Ok(inner
            .infra
            .blob_meta_for_domain_mut(universe_id)?
            .latest_checkpoint(&journal_topic, partition)
            .cloned())
    }

    pub fn submit_command<T: Serialize>(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        payload: &T,
    ) -> Result<aos_node::api::CommandSubmitResponse, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.ensure_registered_world(universe_id, world_id)?;
        let universe_id = inner
            .state
            .registered_worlds
            .get(&world_id)
            .map(|world| world.universe_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        if let Some(existing_id) = command_id.as_deref()
            && let Some(existing) = inner
                .infra
                .blob_meta_for_domain_mut(universe_id)?
                .get_command_record(world_id, existing_id)?
        {
            return Ok(command_submit_response(world_id, existing));
        }

        let world_epoch = inner
            .state
            .registered_worlds
            .get(&world_id)
            .map(|world| world.world_epoch)
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        let command_id = command_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let submitted_at_ns = unix_time_ns();
        let ingress = CommandIngress {
            command_id: command_id.clone(),
            command: command.to_owned(),
            actor,
            payload: CborPayload::inline(aos_cbor::to_canonical_cbor(payload)?),
            submitted_at_ns,
        };
        let submission = SubmissionEnvelope::command(
            command_id.clone(),
            universe_id,
            world_id,
            world_epoch,
            ingress,
        );
        inner.infra.kafka.submit(submission)?;

        let queued = CommandRecord {
            command_id: command_id.clone(),
            command: command.to_owned(),
            status: CommandStatus::Queued,
            submitted_at_ns,
            started_at_ns: None,
            finished_at_ns: None,
            journal_height: None,
            manifest_hash: None,
            result_payload: None,
            error: None,
        };
        let record = match inner
            .infra
            .blob_meta_for_domain_mut(universe_id)?
            .get_command_record(world_id, &command_id)?
        {
            Some(existing) if existing.status != CommandStatus::Queued => existing,
            _ => {
                inner
                    .infra
                    .blob_meta_for_domain_mut(universe_id)?
                    .put_command_record(world_id, queued.clone())?;
                queued
            }
        };
        Ok(command_submit_response(world_id, record))
    }

    pub fn submit_event(
        &self,
        request: SubmitEventRequest,
    ) -> Result<SubmissionAccepted, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(request.universe_id)?;
        inner.ensure_registered_world(request.universe_id, request.world_id)?;
        let (registered_universe_id, world_epoch) = inner
            .state
            .registered_worlds
            .get(&request.world_id)
            .map(|world| (world.universe_id, world.world_epoch))
            .ok_or(WorkerError::UnknownWorld {
                universe_id: request.universe_id,
                world_id: request.world_id,
            })?;
        if let Some(expected_world_epoch) = request.expected_world_epoch
            && expected_world_epoch != world_epoch
        {
            return Err(WorkerError::WorldEpochMismatch {
                universe_id: request.universe_id,
                world_id: request.world_id,
                expected: world_epoch,
                got: expected_world_epoch,
            });
        }

        let payload = SubmissionPayload::DomainEvent {
            schema: request.schema,
            value: aos_node::CborPayload::inline(serde_cbor::to_vec(&request.value)?),
            key: None,
        };
        let submission_id = request
            .submission_id
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let submission = SubmissionEnvelope {
            submission_id: submission_id.clone(),
            universe_id: registered_universe_id,
            world_id: request.world_id,
            world_epoch,
            payload,
        };
        let submission_offset = inner.infra.kafka.submit(submission)?;
        let effective_partition =
            partition_for_world(request.world_id, inner.infra.kafka.partition_count());

        Ok(SubmissionAccepted {
            submission_id,
            submission_offset,
            world_epoch,
            effective_partition,
        })
    }

    pub fn submit_receipt(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        ingress: ReceiptIngress,
    ) -> Result<SubmissionAccepted, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.ensure_registered_world(universe_id, world_id)?;
        let (universe_id, world_epoch) = inner
            .state
            .registered_worlds
            .get(&world_id)
            .map(|world| (world.universe_id, world.world_epoch))
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        let submission_id = ingress
            .correlation_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let submission = SubmissionEnvelope {
            submission_id: submission_id.clone(),
            universe_id,
            world_id,
            world_epoch,
            payload: SubmissionPayload::EffectReceipt {
                intent_hash: ingress.intent_hash,
                adapter_id: ingress.adapter_id,
                status: ingress.status,
                payload: ingress.payload,
                cost_cents: ingress.cost_cents,
                signature: ingress.signature,
            },
        };
        let submission_offset = inner.infra.kafka.submit(submission)?;
        let effective_partition =
            partition_for_world(world_id, inner.infra.kafka.partition_count());
        Ok(SubmissionAccepted {
            submission_id,
            submission_offset,
            world_epoch,
            effective_partition,
        })
    }

    pub fn checkpoint_partition(
        &self,
        partition: u32,
    ) -> Result<aos_node::PartitionCheckpoint, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.create_partition_checkpoint(partition, unix_time_ns(), "manual")
    }

    pub fn state_json(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        key: Option<&str>,
    ) -> Result<Option<JsonValue>, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.ensure_registered_world(universe_id, world_id)?;
        if let Some(world) = inner.state.active_worlds.get(&world_id) {
            let key_cbor = key
                .map(|value| aos_cbor::to_canonical_cbor(&value))
                .transpose()
                .map_err(WorkerError::from)?;
            let bytes = match world.host.state(workflow, key_cbor.as_deref()) {
                Some(bytes) => bytes,
                None => return Ok(None),
            };
            let cbor_value: serde_cbor::Value = serde_cbor::from_slice(&bytes)?;
            return Ok(Some(serde_json::to_value(cbor_value)?));
        }
        let key_cbor = key
            .map(|value| aos_cbor::to_canonical_cbor(&value))
            .transpose()
            .map_err(WorkerError::from)?;
        let reopened = inner.reopen_registered_world_host(universe_id, world_id)?;
        let bytes = match reopened.state(workflow, key_cbor.as_deref()) {
            Some(bytes) => bytes,
            None => return Ok(None),
        };
        let cbor_value: serde_cbor::Value = serde_cbor::from_slice(&bytes)?;
        Ok(Some(serde_json::to_value(cbor_value)?))
    }

    pub fn active_state_json(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        key: Option<&str>,
    ) -> Result<Option<JsonValue>, WorkerError> {
        let inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        let Some(world) = inner.state.active_worlds.get(&world_id) else {
            return Ok(None);
        };
        let key_cbor = key
            .map(|value| aos_cbor::to_canonical_cbor(&value))
            .transpose()
            .map_err(WorkerError::from)?;
        let bytes = match world.host.state(workflow, key_cbor.as_deref()) {
            Some(bytes) => bytes,
            None => return Ok(None),
        };
        let cbor_value: serde_cbor::Value = serde_cbor::from_slice(&bytes)?;
        Ok(Some(serde_json::to_value(cbor_value)?))
    }

    pub fn is_world_active(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<bool, WorkerError> {
        let inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        Ok(inner.state.active_worlds.contains_key(&world_id))
    }

    pub fn manifest(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<ManifestResponse, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.with_world_host_for_read(
            universe_id,
            world_id,
            |host: &WorldHost<crate::blobstore::HostedCas>| {
                let manifest_hash = host.kernel().manifest_hash();
                let loaded = ManifestLoader::load_from_hash(host.store(), manifest_hash)?;
                Ok(ManifestResponse {
                    journal_head: host.heights().head,
                    manifest_hash: manifest_hash.to_hex(),
                    manifest: loaded.manifest,
                })
            },
        )
    }

    pub fn defs_list(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        kinds: Option<Vec<String>>,
        prefix: Option<String>,
    ) -> Result<DefsListResponse, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.with_world_host_for_read(
            universe_id,
            world_id,
            |host: &WorldHost<crate::blobstore::HostedCas>| {
                Ok(DefsListResponse {
                    journal_head: host.heights().head,
                    manifest_hash: host.kernel().manifest_hash().to_hex(),
                    defs: host.list_defs(kinds.as_deref(), prefix.as_deref())?,
                })
            },
        )
    }

    pub fn def_get(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        name: &str,
    ) -> Result<DefGetResponse, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.with_world_host_for_read(
            universe_id,
            world_id,
            |host: &WorldHost<crate::blobstore::HostedCas>| {
                Ok(DefGetResponse {
                    journal_head: host.heights().head,
                    manifest_hash: host.kernel().manifest_hash().to_hex(),
                    def: host.get_def(name)?,
                })
            },
        )
    }

    pub fn state_get(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        key: Option<Vec<u8>>,
    ) -> Result<StateGetResponse, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.with_world_host_for_read(
            universe_id,
            world_id,
            |host: &WorldHost<crate::blobstore::HostedCas>| {
                let key_bytes = key.unwrap_or_default();
                let state = host.state(workflow, Some(&key_bytes));
                let state_hash = state
                    .as_ref()
                    .map(|bytes: &Vec<u8>| Hash::of_bytes(bytes).to_hex());
                let size = state
                    .as_ref()
                    .map(|bytes: &Vec<u8>| bytes.len() as u64)
                    .unwrap_or(0);
                let cell = state_hash.map(|state_hash| StateCellSummary {
                    journal_head: host.heights().head,
                    workflow: workflow.to_owned(),
                    key_hash: Hash::of_bytes(&key_bytes).as_bytes().to_vec(),
                    key_bytes: key_bytes.clone(),
                    state_hash,
                    size,
                    last_active_ns: 0,
                });
                Ok(StateGetResponse {
                    journal_head: host.heights().head,
                    workflow: workflow.to_owned(),
                    key_b64: Some(BASE64_STANDARD.encode(&key_bytes)),
                    cell,
                    state_b64: state.map(|bytes| BASE64_STANDARD.encode(bytes)),
                })
            },
        )
    }

    pub fn state_list(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        limit: u32,
    ) -> Result<StateListResponse, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.with_world_host_for_read(
            universe_id,
            world_id,
            |host: &WorldHost<crate::blobstore::HostedCas>| {
                let mut cells = host
                    .list_cells(workflow)?
                    .into_iter()
                    .map(|cell| StateCellSummary {
                        journal_head: host.heights().head,
                        workflow: workflow.to_owned(),
                        key_hash: cell.key_hash.to_vec(),
                        key_bytes: cell.key_bytes,
                        state_hash: Hash::from(cell.state_hash).to_hex(),
                        size: cell.size,
                        last_active_ns: cell.last_active_ns,
                    })
                    .collect::<Vec<_>>();
                cells.truncate(limit as usize);
                Ok(StateListResponse {
                    journal_head: host.heights().head,
                    workflow: workflow.to_owned(),
                    cells,
                })
            },
        )
    }

    pub fn trace_summary(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<JsonValue, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.with_world_host_for_read(
            universe_id,
            world_id,
            |host: &WorldHost<crate::blobstore::HostedCas>| Ok(host.trace_summary()?),
        )
    }

    pub fn trace(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        query: RuntimeTraceQuery,
    ) -> Result<JsonValue, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.with_world_host_for_read(
            universe_id,
            world_id,
            |host: &WorldHost<crate::blobstore::HostedCas>| Ok(trace_get(host.kernel(), query)?),
        )
    }

    pub fn load_manifest(
        &self,
        universe_id: UniverseId,
        manifest_hash: &str,
    ) -> Result<aos_kernel::LoadedManifest, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.load_manifest_into_local_cas(universe_id, manifest_hash)
    }

    pub fn universe_id_for_world(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<UniverseId, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.ensure_registered_world(universe_id, world_id)?;
        inner
            .state
            .registered_worlds
            .get(&world_id)
            .map(|world| world.universe_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })
    }

    pub fn cas_store(&self) -> Result<Arc<HostedCas>, WorkerError> {
        let universe_id = self.default_universe_id()?;
        self.cas_store_for_domain(universe_id)
    }

    pub fn cas_store_for_domain(
        &self,
        universe_id: UniverseId,
    ) -> Result<Arc<HostedCas>, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.infra.store_for_domain(universe_id)
    }

    pub fn blob_metadata(&self, universe_id: UniverseId, hash: Hash) -> Result<bool, WorkerError> {
        let inner = self.lock_inner()?;
        inner
            .infra
            .stores_by_domain
            .get(&universe_id)
            .map(|store: &Arc<HostedCas>| store.has_blob(hash).map_err(WorkerError::Store))
            .unwrap_or(Ok(false))
    }

    pub fn get_blob(&self, universe_id: UniverseId, hash: Hash) -> Result<Vec<u8>, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner
            .infra
            .store_for_domain(universe_id)?
            .get(hash)
            .map_err(WorkerError::Persist)
    }

    pub fn partition_entries(
        &self,
        partition: u32,
    ) -> Result<Vec<crate::kafka::PartitionLogEntry>, WorkerError> {
        let inner = self.lock_inner()?;
        Ok(inner
            .infra
            .kafka
            .partition_entries(&inner.infra.kafka.config().journal_topic, partition)
            .to_vec())
    }

    pub fn projection_entries(
        &self,
        partition: u32,
    ) -> Result<Vec<crate::kafka::ProjectionTopicEntry>, WorkerError> {
        let inner = self.lock_inner()?;
        Ok(inner
            .infra
            .kafka
            .projection_entries(&inner.infra.kafka.config().projection_topic, partition)
            .to_vec())
    }

    pub fn effective_partition(&self, world_id: WorldId) -> Result<u32, WorkerError> {
        let inner = self.lock_inner()?;
        Ok(partition_for_world(
            world_id,
            inner.infra.kafka.partition_count(),
        ))
    }

    pub fn journal_topic(&self) -> Result<String, WorkerError> {
        Ok(self
            .lock_inner()?
            .infra
            .kafka
            .config()
            .journal_topic
            .clone())
    }

    pub fn projection_topic(&self) -> Result<String, WorkerError> {
        Ok(self
            .lock_inner()?
            .infra
            .kafka
            .config()
            .projection_topic
            .clone())
    }

    pub fn refresh_materializer_source(&self) -> Result<(), WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.infra.kafka.recover_from_broker()?;
        Ok(())
    }

    pub fn world_active_baseline(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<aos_node::SnapshotRecord, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.select_source_snapshot(
            universe_id,
            world_id,
            &aos_node::SnapshotSelector::ActiveBaseline,
        )
    }

    pub fn workspace_resolve(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workspace: &str,
        version: Option<u64>,
    ) -> Result<WorkspaceResolveResponse, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.with_world_host_for_read(
            universe_id,
            world_id,
            |host: &WorldHost<crate::blobstore::HostedCas>| {
                #[derive(Debug, Default, Deserialize)]
                struct WorkspaceHistoryState {
                    latest: u64,
                    versions: BTreeMap<u64, WorkspaceCommitMetaState>,
                }
                #[derive(Debug, Deserialize)]
                struct WorkspaceCommitMetaState {
                    root_hash: String,
                }

                let key = serde_cbor::to_vec(&workspace.to_string())?;
                let history = host.state("sys/Workspace@1", Some(&key));
                let receipt = if let Some(bytes) = history {
                    let history: WorkspaceHistoryState = serde_cbor::from_slice(&bytes)?;
                    let head = Some(history.latest);
                    let target = version.unwrap_or(history.latest);
                    if let Some(entry) = history.versions.get(&target) {
                        aos_effect_types::WorkspaceResolveReceipt {
                            exists: true,
                            resolved_version: Some(target),
                            head,
                            root_hash: Some(HashRef::new(entry.root_hash.clone()).map_err(
                                |err| {
                                    WorkerError::Persist(aos_node::PersistError::validation(
                                        err.to_string(),
                                    ))
                                },
                            )?),
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
                    workspace: workspace.to_owned(),
                    receipt,
                })
            },
        )
    }

    pub fn partition_count(&self) -> Result<u32, WorkerError> {
        Ok(self.lock_inner()?.infra.kafka.partition_count())
    }

    pub fn uses_broker_kafka(&self) -> Result<bool, WorkerError> {
        Ok(self.lock_inner()?.infra.kafka.is_broker())
    }

    pub fn kafka_config(&self) -> Result<KafkaConfig, WorkerError> {
        Ok(self.lock_inner()?.infra.kafka.config().clone())
    }

    pub fn blobstore_config(&self) -> Result<BlobStoreConfig, WorkerError> {
        Ok(self.lock_inner()?.infra.blobstore_config.clone())
    }

    pub fn assigned_partitions(&self) -> Result<Vec<u32>, WorkerError> {
        Ok(self.lock_inner()?.infra.kafka.assigned_partitions())
    }

    pub fn put_blob(&self, universe_id: UniverseId, bytes: &[u8]) -> Result<Hash, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner
            .infra
            .store_for_domain(universe_id)?
            .put_verified(bytes)
            .map_err(WorkerError::Persist)
    }

    pub fn debug_fail_next_batch_commit(&self) -> Result<(), WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.infra.kafka.debug_fail_next_batch_commit();
        Ok(())
    }

    pub fn world_summary_response(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<WorldSummaryResponse, WorkerError> {
        let mut inner = self.lock_inner()?;
        inner.require_default_universe(universe_id)?;
        inner.world_summary_response(universe_id, world_id)
    }

    pub fn list_world_runtime_infos(
        &self,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, WorkerError> {
        let mut inner = self.lock_inner()?;
        let universe_id = inner.infra.default_universe_id;
        let world_ids = inner
            .infra
            .kafka
            .world_ids()
            .into_iter()
            .filter(|world_id| after.is_none_or(|after| *world_id > after))
            .collect::<Vec<_>>();
        let mut infos = Vec::new();
        for world_id in world_ids {
            let _ = inner.ensure_registered_world(universe_id, world_id);
            if let Ok(summary) = inner.world_summary_response(universe_id, world_id) {
                infos.push(summary.runtime);
            }
        }
        infos.sort_by_key(|world| world.world_id);
        infos.truncate(limit as usize);
        Ok(infos)
    }

    pub(super) fn lock_inner(
        &self,
    ) -> Result<MutexGuard<'_, HostedWorkerRuntimeInner>, WorkerError> {
        self.inner.lock().map_err(|_| WorkerError::RuntimePoisoned)
    }
}

impl BlobPlane for HostedWorkerRuntime {
    fn put_blob(
        &self,
        universe_id: UniverseId,
        bytes: &[u8],
    ) -> Result<Hash, aos_node::PlaneError> {
        HostedWorkerRuntime::put_blob(self, universe_id, bytes).map_err(|err| {
            aos_node::PlaneError::Persist(aos_node::PersistError::backend(err.to_string()))
        })
    }

    fn get_blob(
        &self,
        universe_id: UniverseId,
        hash: Hash,
    ) -> Result<Vec<u8>, aos_node::PlaneError> {
        HostedWorkerRuntime::get_blob(self, universe_id, hash).map_err(|err| {
            aos_node::PlaneError::Persist(aos_node::PersistError::backend(err.to_string()))
        })
    }

    fn has_blob(&self, universe_id: UniverseId, hash: Hash) -> Result<bool, aos_node::PlaneError> {
        HostedWorkerRuntime::blob_metadata(self, universe_id, hash).map_err(|err| {
            aos_node::PlaneError::Persist(aos_node::PersistError::backend(err.to_string()))
        })
    }
}

impl HostedWorkerInfra {
    pub(super) fn domain_paths(&self, universe_id: UniverseId) -> LocalStatePaths {
        self.paths.for_universe(universe_id)
    }

    pub(super) fn world_config_for_domain(
        &self,
        universe_id: UniverseId,
    ) -> Result<WorldConfig, WorkerError> {
        let mut config = self.world_config.clone();
        let domain_paths = self.domain_paths(universe_id);
        domain_paths.ensure_root().map_err(|err| {
            WorkerError::Persist(aos_node::PersistError::backend(err.to_string()))
        })?;
        std::fs::create_dir_all(domain_paths.cache_root()).map_err(|err| {
            WorkerError::Persist(aos_node::PersistError::backend(format!(
                "create hosted domain cache dir: {err}"
            )))
        })?;
        config.module_cache_dir = Some(domain_paths.wasmtime_cache_dir());
        Ok(config)
    }

    pub(super) fn store_for_domain(
        &mut self,
        universe_id: UniverseId,
    ) -> Result<Arc<HostedCas>, WorkerError> {
        if let Some(store) = self.stores_by_domain.get(&universe_id) {
            return Ok(Arc::clone(store));
        }
        let domain_paths = self.domain_paths(universe_id);
        domain_paths.ensure_root().map_err(|err| {
            WorkerError::Persist(aos_node::PersistError::backend(err.to_string()))
        })?;
        std::fs::create_dir_all(domain_paths.cache_root()).map_err(|err| {
            WorkerError::Persist(aos_node::PersistError::backend(format!(
                "create hosted domain cache dir: {err}"
            )))
        })?;
        let local_cas = Arc::new(FsCas::open_with_paths(&domain_paths)?);
        let remote = Arc::new(RemoteCasStore::new(scoped_blobstore_config(
            &self.blobstore_config,
            universe_id,
        ))?);
        let hosted = Arc::new(HostedCas::new(local_cas, remote));
        self.stores_by_domain
            .insert(universe_id, Arc::clone(&hosted));
        Ok(hosted)
    }

    pub(super) fn blob_meta_for_domain_mut(
        &mut self,
        universe_id: UniverseId,
    ) -> Result<&mut HostedBlobMetaStore, WorkerError> {
        if !self.blob_meta_by_domain.contains_key(&universe_id) {
            let scoped = scoped_blobstore_config(&self.blobstore_config, universe_id);
            let mut plane = HostedBlobMetaStore::new(scoped)?;
            plane.prime_latest_checkpoints(
                &self.kafka.config().journal_topic,
                self.kafka.partition_count(),
            )?;
            self.blob_meta_by_domain.insert(universe_id, plane);
        }
        self.blob_meta_by_domain
            .get_mut(&universe_id)
            .ok_or(WorkerError::RuntimePoisoned)
    }
}
