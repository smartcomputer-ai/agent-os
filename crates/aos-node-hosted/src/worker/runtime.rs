use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use aos_cbor::{Hash, to_canonical_cbor};
use aos_kernel::{Store, WorldInput};
use aos_node::{
    BlobBackend, CborPayload, CheckpointBackend, CommandIngress, CommandRecord, CreateWorldRequest,
    EffectRuntimeEvent, LocalStatePaths, PartitionCheckpoint, ReceiptIngress, SubmissionEnvelope,
    SubmissionPayload, UniverseId, WorldConfig, WorldId, partition_for_world,
};
use uuid::Uuid;

use crate::blobstore::{BlobStoreConfig, HostedCas};
use crate::config::ProjectionCommitMode;
use crate::kafka::{
    BrokerKafkaIngress, HostedKafkaBackend, KafkaConfig, PartitionLogEntry, ProjectionTopicEntry,
};

use super::commands::{
    command_submit_response, synthesize_queued_command_record, world_control_from_command_payload,
};
use super::core::SchedulerMsg;
use super::types::{
    CreateWorldAccepted, HostedWorkerCore, HostedWorkerInfra, HostedWorldSummary, WorkerError,
};
use super::util::{
    default_state_root, resolve_cbor_payload, temp_embedded_state_root, unix_time_ns,
};

#[derive(Clone)]
pub struct HostedWorkerRuntime {
    core: Arc<Mutex<HostedWorkerCore>>,
    paths: Arc<LocalStatePaths>,
    embedded_ingress_notify: Option<Arc<tokio::sync::Notify>>,
    broker_ingress: Option<Arc<Mutex<BrokerKafkaIngress>>>,
}

impl std::fmt::Debug for HostedWorkerRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedWorkerRuntime")
            .finish_non_exhaustive()
    }
}

impl HostedWorkerRuntime {
    fn submit_envelope_locked(
        core: &mut HostedWorkerCore,
        submission: SubmissionEnvelope,
    ) -> Result<u64, WorkerError> {
        let submission_offset = core.infra.kafka.submit(submission)?;
        core.dispatch_embedded_ingress_messages()?;
        Ok(submission_offset)
    }

    pub fn new(partition_count: u32) -> Result<Self, WorkerError> {
        Self::new_with_state_root_and_universe(
            partition_count,
            default_state_root()?,
            aos_node::local_universe_id(),
        )
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

        let (effect_event_tx, effect_event_rx) = tokio::sync::mpsc::channel(1024);
        let world_config = WorldConfig::from_env_with_fallback_module_cache_dir(None);
        let kafka = HostedKafkaBackend::new(partition_count, kafka_config)?;
        let embedded_ingress_notify = kafka.embedded_ingress_notify();
        let broker_ingress = kafka
            .broker_ingress_driver()
            .map(|driver| Arc::new(Mutex::new(driver)));
        let mut core = HostedWorkerCore {
            infra: HostedWorkerInfra {
                default_universe_id,
                paths: paths.clone(),
                blobstore_config: blobstore_config.clone(),
                vault: crate::vault::HostedVault::new(blobstore_config.clone()).map_err(|err| {
                    WorkerError::Build(anyhow::anyhow!("initialize hosted vault: {err}"))
                })?,
                world_config,
                kafka,
                stores_by_domain: Default::default(),
                blob_meta_by_domain: Default::default(),
            },
            state: Default::default(),
            effect_event_tx,
            effect_event_rx: Some(effect_event_rx),
            shared_effect_runtimes: Default::default(),
            scheduler_tx: None,
            flush_limits: super::core::FlushLimits {
                max_slices: 256,
                max_bytes: 1 << 20,
                max_delay: Duration::from_millis(5),
            },
            max_local_continuation_slices_per_flush: 64,
            projection_commit_mode: ProjectionCommitMode::Background,
            max_uncommitted_slices_per_world: 256,
            debug_skip_flush_commit: false,
            debug_fail_after_next_flush_commit: false,
        };

        if core.infra.kafka.is_broker()
            && !blobstore_config
                .bucket
                .as_ref()
                .is_some_and(|value: &String| !value.trim().is_empty())
        {
            return Err(WorkerError::Persist(aos_node::PersistError::validation(
                "broker-backed hosted runtime requires AOS_BLOBSTORE_BUCKET or legacy AOS_S3_BUCKET",
            )));
        }

        core.bootstrap_recovery()?;

        Ok(Self {
            core: Arc::new(Mutex::new(core)),
            paths: Arc::new(paths),
            embedded_ingress_notify,
            broker_ingress,
        })
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

        let (effect_event_tx, effect_event_rx) = tokio::sync::mpsc::channel(1024);
        let world_config = WorldConfig::from_env_with_fallback_module_cache_dir(None);
        let kafka = HostedKafkaBackend::new_embedded(partition_count, KafkaConfig::default())?;
        let embedded_ingress_notify = kafka.embedded_ingress_notify();
        let mut core = HostedWorkerCore {
            infra: HostedWorkerInfra {
                default_universe_id,
                paths: paths.clone(),
                blobstore_config: BlobStoreConfig::default(),
                vault: crate::vault::HostedVault::new(BlobStoreConfig::default()).map_err(
                    |err| WorkerError::Build(anyhow::anyhow!("initialize hosted vault: {err}")),
                )?,
                world_config,
                kafka,
                stores_by_domain: Default::default(),
                blob_meta_by_domain: Default::default(),
            },
            state: Default::default(),
            effect_event_tx,
            effect_event_rx: Some(effect_event_rx),
            shared_effect_runtimes: Default::default(),
            scheduler_tx: None,
            flush_limits: super::core::FlushLimits {
                max_slices: 256,
                max_bytes: 1 << 20,
                max_delay: Duration::from_millis(5),
            },
            max_local_continuation_slices_per_flush: 64,
            projection_commit_mode: ProjectionCommitMode::Background,
            max_uncommitted_slices_per_world: 256,
            debug_skip_flush_commit: false,
            debug_fail_after_next_flush_commit: false,
        };
        core.bootstrap_recovery()?;

        Ok(Self {
            core: Arc::new(Mutex::new(core)),
            paths: Arc::new(paths),
            embedded_ingress_notify,
            broker_ingress: None,
        })
    }

    pub fn paths(&self) -> &LocalStatePaths {
        self.paths.as_ref()
    }

    pub fn default_universe_id(&self) -> Result<UniverseId, WorkerError> {
        Ok(self.lock_core()?.infra.default_universe_id)
    }

    pub fn cas_store_for_domain(
        &self,
        universe_id: UniverseId,
    ) -> Result<Arc<HostedCas>, WorkerError> {
        let mut core = self.lock_core()?;
        core.infra.store_for_domain(universe_id)
    }

    pub fn put_blob(&self, universe_id: UniverseId, bytes: &[u8]) -> Result<Hash, WorkerError> {
        let mut core = self.lock_core()?;
        core.infra
            .store_for_domain(universe_id)?
            .put_verified(bytes)
            .map_err(WorkerError::Persist)
    }

    pub fn get_blob(&self, universe_id: UniverseId, hash: Hash) -> Result<Vec<u8>, WorkerError> {
        let mut core = self.lock_core()?;
        core.infra
            .store_for_domain(universe_id)?
            .get(hash)
            .map_err(WorkerError::Persist)
    }

    pub fn blob_metadata(&self, universe_id: UniverseId, hash: Hash) -> Result<bool, WorkerError> {
        let core = self.lock_core()?;
        core.infra
            .stores_by_domain
            .get(&universe_id)
            .map(|store| store.has_blob(hash).map_err(WorkerError::Store))
            .unwrap_or(Ok(false))
    }

    pub fn create_world(
        &self,
        universe_id: UniverseId,
        request: CreateWorldRequest,
    ) -> Result<CreateWorldAccepted, WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(universe_id)?;
        let world_id = request
            .world_id
            .unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
        core.seed_world_direct(universe_id, world_id, request)?;
        let effective_partition = partition_for_world(world_id, core.infra.kafka.partition_count());
        Ok(CreateWorldAccepted {
            submission_id: format!("seed-{world_id}"),
            submission_offset: 0,
            world_id,
            effective_partition,
        })
    }

    pub fn submit_submission(&self, submission: SubmissionEnvelope) -> Result<u64, WorkerError> {
        let mut core = self.lock_core()?;
        Self::submit_envelope_locked(&mut core, submission)
    }

    pub fn submit_event(
        &self,
        request: super::types::SubmitEventRequest,
    ) -> Result<super::types::SubmissionAccepted, WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(request.universe_id)?;
        core.ensure_registered_world(request.universe_id, request.world_id)?;
        let registered = core.state.registered_worlds.get(&request.world_id).ok_or(
            WorkerError::UnknownWorld {
                universe_id: request.universe_id,
                world_id: request.world_id,
            },
        )?;
        let registered_universe = registered.universe_id;
        let registered_world_epoch = registered.world_epoch;
        if let Some(expected_world_epoch) = request.expected_world_epoch
            && expected_world_epoch != registered_world_epoch
        {
            return Err(WorkerError::WorldEpochMismatch {
                universe_id: registered_universe,
                world_id: request.world_id,
                expected: registered_world_epoch,
                got: expected_world_epoch,
            });
        }

        let submission_id = request
            .submission_id
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let submission = SubmissionEnvelope {
            submission_id: submission_id.clone(),
            universe_id: registered_universe,
            world_id: request.world_id,
            world_epoch: registered_world_epoch,
            command: None,
            payload: SubmissionPayload::WorldInput {
                input: WorldInput::DomainEvent(aos_wasm_abi::DomainEvent {
                    schema: request.schema,
                    value: serde_cbor::to_vec(&request.value)?,
                    key: None,
                }),
            },
        };
        let submission_offset = Self::submit_envelope_locked(&mut core, submission)?;
        Ok(super::types::SubmissionAccepted {
            submission_id,
            submission_offset,
            world_epoch: registered_world_epoch,
            effective_partition: partition_for_world(
                request.world_id,
                core.infra.kafka.partition_count(),
            ),
        })
    }

    pub fn submit_receipt(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        ingress: ReceiptIngress,
    ) -> Result<super::types::SubmissionAccepted, WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(universe_id)?;
        core.ensure_registered_world(universe_id, world_id)?;
        let registered =
            core.state
                .registered_worlds
                .get(&world_id)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id,
                    world_id,
                })?;
        let registered_universe = registered.universe_id;
        let registered_world_epoch = registered.world_epoch;
        let submission_id = ingress
            .correlation_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let store = core.infra.store_for_domain(registered_universe)?;
        let submission = SubmissionEnvelope {
            submission_id: submission_id.clone(),
            universe_id: registered_universe,
            world_id,
            world_epoch: registered_world_epoch,
            command: None,
            payload: SubmissionPayload::WorldInput {
                input: WorldInput::Receipt(aos_effects::EffectReceipt {
                    intent_hash: ingress.intent_hash.clone().try_into().map_err(|_| {
                        WorkerError::LogFirst(aos_node::BackendError::InvalidIntentHashLen(
                            ingress.intent_hash.len(),
                        ))
                    })?,
                    adapter_id: ingress.adapter_id,
                    status: ingress.status,
                    payload_cbor: resolve_cbor_payload(store.as_ref(), &ingress.payload)?,
                    cost_cents: ingress.cost_cents,
                    signature: ingress.signature,
                }),
            },
        };
        let submission_offset = Self::submit_envelope_locked(&mut core, submission)?;
        Ok(super::types::SubmissionAccepted {
            submission_id,
            submission_offset,
            world_epoch: registered_world_epoch,
            effective_partition: partition_for_world(world_id, core.infra.kafka.partition_count()),
        })
    }

    pub fn get_command_record(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(universe_id)?;
        core.ensure_registered_world(universe_id, world_id)?;
        let registered_universe = core
            .state
            .registered_worlds
            .get(&world_id)
            .map(|world| world.universe_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        core.infra
            .blob_meta_for_domain_mut(registered_universe)?
            .get_command_record(world_id, command_id)?
            .ok_or_else(|| WorkerError::UnknownCommand {
                universe_id: registered_universe,
                world_id,
                command_id: command_id.to_owned(),
            })
    }

    pub fn put_command_record(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        record: CommandRecord,
    ) -> Result<(), WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(universe_id)?;
        core.infra
            .blob_meta_for_domain_mut(universe_id)?
            .put_command_record(world_id, record)?;
        Ok(())
    }

    pub fn submit_command<T: serde::Serialize>(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        payload: &T,
    ) -> Result<aos_node::api::CommandSubmitResponse, WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(universe_id)?;
        core.ensure_registered_world(universe_id, world_id)?;
        let registered =
            core.state
                .registered_worlds
                .get(&world_id)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id,
                    world_id,
                })?;
        let registered_universe = registered.universe_id;
        let registered_world_epoch = registered.world_epoch;
        let registered_store = Arc::clone(&registered.store);

        if let Some(existing_id) = command_id.as_deref()
            && let Some(existing) = core
                .infra
                .blob_meta_for_domain_mut(registered_universe)?
                .get_command_record(world_id, existing_id)?
        {
            return Ok(command_submit_response(world_id, existing));
        }

        let payload_bytes = to_canonical_cbor(payload)?;
        let world_control =
            world_control_from_command_payload(registered_store.as_ref(), command, &payload_bytes)?;
        let command_id = command_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let submitted_at_ns = unix_time_ns();
        let ingress = CommandIngress {
            command_id: command_id.clone(),
            command: command.to_owned(),
            actor,
            payload: CborPayload::inline(payload_bytes),
            submitted_at_ns,
        };
        let queued = synthesize_queued_command_record(&ingress);
        let submission = SubmissionEnvelope::world_control(
            format!("cmd-{command_id}"),
            registered_universe,
            world_id,
            registered_world_epoch,
            ingress,
            world_control,
        );
        let _ = Self::submit_envelope_locked(&mut core, submission)?;
        core.infra
            .blob_meta_for_domain_mut(registered_universe)?
            .put_command_record(world_id, queued.clone())?;
        Ok(command_submit_response(world_id, queued))
    }

    pub fn effective_partition(&self, world_id: WorldId) -> Result<u32, WorkerError> {
        let core = self.lock_core()?;
        Ok(partition_for_world(
            world_id,
            core.infra.kafka.partition_count(),
        ))
    }

    pub fn assigned_partitions(&self) -> Result<Vec<u32>, WorkerError> {
        Ok(self
            .lock_core()?
            .state
            .assigned_partitions
            .iter()
            .copied()
            .collect())
    }

    pub fn kafka_config(&self) -> Result<KafkaConfig, WorkerError> {
        Ok(self.lock_core()?.infra.kafka.config().clone())
    }

    pub fn partition_count(&self) -> Result<u32, WorkerError> {
        Ok(self.lock_core()?.infra.kafka.partition_count())
    }

    pub fn journal_topic(&self) -> Result<String, WorkerError> {
        Ok(self.lock_core()?.infra.kafka.config().journal_topic.clone())
    }

    pub fn partition_entries(&self, partition: u32) -> Result<Vec<PartitionLogEntry>, WorkerError> {
        let mut core = self.lock_core()?;
        core.infra.kafka.recover_partition_from_broker(partition)?;
        let journal_topic = core.infra.kafka.config().journal_topic.clone();
        Ok(core
            .infra
            .kafka
            .partition_entries(&journal_topic, partition)
            .to_vec())
    }

    pub fn projection_entries(
        &self,
        partition: u32,
    ) -> Result<Vec<ProjectionTopicEntry>, WorkerError> {
        let mut core = self.lock_core()?;
        core.infra.kafka.recover_partition_from_broker(partition)?;
        let projection_topic = core.infra.kafka.config().projection_topic.clone();
        Ok(core
            .infra
            .kafka
            .projection_entries(&projection_topic, partition)
            .to_vec())
    }

    pub fn refresh_materializer_source(&self) -> Result<(), WorkerError> {
        self.lock_core()?.infra.kafka.recover_from_broker()?;
        Ok(())
    }

    pub fn latest_checkpoint(
        &self,
        universe_id: UniverseId,
        partition: u32,
    ) -> Result<Option<PartitionCheckpoint>, WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(universe_id)?;
        let journal_topic = core.infra.kafka.config().journal_topic.clone();
        Ok(core
            .infra
            .blob_meta_for_domain_mut(universe_id)?
            .latest_checkpoint(&journal_topic, partition)
            .cloned())
    }

    pub fn checkpoint_partition(&self, partition: u32) -> Result<PartitionCheckpoint, WorkerError> {
        let mut core = self.lock_core()?;
        let mut profile = super::types::SupervisorRunProfile::default();
        let _ = core.drive_scheduler_until_quiescent(true, &mut profile)?;
        core.create_partition_checkpoint(partition, unix_time_ns(), 0, None, "manual")?
            .ok_or_else(|| {
                WorkerError::Persist(aos_node::PersistError::validation(format!(
                    "partition {partition} had no checkpointable changes"
                )))
            })
    }

    pub fn blobstore_config(&self) -> Result<BlobStoreConfig, WorkerError> {
        Ok(self.lock_core()?.infra.blobstore_config.clone())
    }

    pub fn vault(&self) -> Result<crate::vault::HostedVault, WorkerError> {
        Ok(self.lock_core()?.infra.vault.clone())
    }

    pub fn flush_max_delay(&self) -> Result<Duration, WorkerError> {
        Ok(self.lock_core()?.flush_limits.max_delay)
    }

    pub fn projection_commit_mode(&self) -> Result<ProjectionCommitMode, WorkerError> {
        Ok(self.lock_core()?.projection_commit_mode)
    }

    pub fn max_local_continuation_slices_per_flush(&self) -> Result<usize, WorkerError> {
        Ok(self.lock_core()?.max_local_continuation_slices_per_flush)
    }

    pub fn set_max_local_continuation_slices_per_flush(
        &self,
        max: usize,
    ) -> Result<(), WorkerError> {
        self.lock_core()?.max_local_continuation_slices_per_flush = max;
        Ok(())
    }

    pub fn set_projection_commit_mode(
        &self,
        mode: ProjectionCommitMode,
    ) -> Result<(), WorkerError> {
        self.lock_core()?.projection_commit_mode = mode;
        Ok(())
    }

    pub fn max_uncommitted_slices_per_world(&self) -> Result<usize, WorkerError> {
        Ok(self.lock_core()?.max_uncommitted_slices_per_world)
    }

    pub fn set_max_uncommitted_slices_per_world(&self, max: usize) -> Result<(), WorkerError> {
        if max == 0 {
            return Err(WorkerError::Persist(aos_node::PersistError::validation(
                "max_uncommitted_slices_per_world must be greater than zero",
            )));
        }
        self.lock_core()?.max_uncommitted_slices_per_world = max;
        Ok(())
    }

    pub fn uses_broker_kafka(&self) -> Result<bool, WorkerError> {
        Ok(self.lock_core()?.infra.kafka.is_broker())
    }

    pub(super) fn take_effect_event_rx(
        &self,
    ) -> Result<Option<tokio::sync::mpsc::Receiver<EffectRuntimeEvent<WorldId>>>, WorkerError> {
        Ok(self.lock_core()?.effect_event_rx.take())
    }

    pub(super) fn set_scheduler_tx(
        &self,
        tx: tokio::sync::mpsc::UnboundedSender<SchedulerMsg>,
    ) -> Result<(), WorkerError> {
        self.lock_core()?.scheduler_tx = Some(tx);
        Ok(())
    }

    pub(super) fn clear_scheduler_tx(&self) -> Result<(), WorkerError> {
        self.lock_core()?.scheduler_tx = None;
        Ok(())
    }

    pub(super) fn embedded_ingress_notify(&self) -> Option<Arc<tokio::sync::Notify>> {
        self.embedded_ingress_notify.clone()
    }

    pub(super) fn collect_ingress_bridge_messages(&self) -> Result<Vec<SchedulerMsg>, WorkerError> {
        let backlog_by_partition = self.lock_core()?.ingress_backlog_by_partition();
        let Some(bridge) = self.broker_ingress.as_ref() else {
            return Ok(Vec::new());
        };
        let mut ingress = bridge.lock().map_err(|_| WorkerError::RuntimePoisoned)?;
        let batch = ingress.poll(&backlog_by_partition)?;
        let mut messages = Vec::new();
        if !batch.assignment.newly_assigned.is_empty() || !batch.assignment.revoked.is_empty() {
            messages.push(SchedulerMsg::Assignment(super::core::AssignmentDelta {
                assigned: batch.assignment.assigned,
                newly_assigned: batch.assignment.newly_assigned,
                revoked: batch.assignment.revoked,
            }));
        }
        messages.extend(batch.records.into_iter().map(SchedulerMsg::Ingress));
        Ok(messages)
    }

    pub fn debug_fail_next_batch_commit(&self) -> Result<(), WorkerError> {
        let mut core = self.lock_core()?;
        core.infra.kafka.debug_fail_next_batch_commit();
        Ok(())
    }

    pub fn debug_fail_after_next_flush_commit(&self) -> Result<(), WorkerError> {
        self.lock_core()?.debug_fail_after_next_flush_commit = true;
        Ok(())
    }

    pub fn debug_skip_flush_commit(&self) -> Result<(), WorkerError> {
        self.lock_core()?.debug_skip_flush_commit = true;
        Ok(())
    }

    pub fn request_shutdown(&self) -> Result<bool, WorkerError> {
        let scheduler_tx = self.lock_core()?.scheduler_tx.clone();
        let Some(scheduler_tx) = scheduler_tx else {
            return Ok(false);
        };
        Ok(scheduler_tx.send(SchedulerMsg::Shutdown).is_ok())
    }

    pub fn get_world(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<HostedWorldSummary, WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(universe_id)?;
        core.ensure_registered_world(universe_id, world_id)?;
        core.world_summary(world_id)
    }

    pub fn state_json(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        key: Option<&str>,
    ) -> Result<Option<serde_json::Value>, WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(universe_id)?;
        core.ensure_registered_world(universe_id, world_id)?;
        if !core.state.active_worlds.contains_key(&world_id) {
            core.activate_world(world_id)?;
        }
        let key_cbor = key
            .map(|value| aos_cbor::to_canonical_cbor(&value))
            .transpose()
            .map_err(WorkerError::from)?;
        let Some(world) = core.state.active_worlds.get(&world_id) else {
            return Ok(None);
        };
        let Some(bytes) = world.state(workflow, key_cbor.as_deref()) else {
            return Ok(None);
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
    ) -> Result<Option<serde_json::Value>, WorkerError> {
        self.state_json(universe_id, world_id, workflow, key)
    }

    pub fn trace_summary(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<serde_json::Value, WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(universe_id)?;
        core.ensure_registered_world(universe_id, world_id)?;
        if !core.state.active_worlds.contains_key(&world_id) {
            core.activate_world(world_id)?;
        }
        let world = core
            .state
            .active_worlds
            .get(&world_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        let route_diagnostics = core
            .state
            .registered_worlds
            .get(&world_id)
            .map(|registered| {
                aos_kernel::TraceRouteDiagnostics::from(
                    registered.effect_runtime.route_diagnostics(),
                )
            });
        aos_kernel::workflow_trace_summary_with_routes(&world.kernel, route_diagnostics.as_ref())
            .map_err(WorkerError::Kernel)
    }

    pub fn runtime_info(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<aos_node::WorldRuntimeInfo, WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(universe_id)?;
        core.ensure_registered_world(universe_id, world_id)?;
        if !core.state.active_worlds.contains_key(&world_id) {
            core.activate_world(world_id)?;
        }
        let world = core
            .state
            .active_worlds
            .get(&world_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        Ok(super::util::runtime_info_from_world(
            world,
            core.state.async_worlds.get(&world_id),
        ))
    }

    pub fn list_worlds(
        &self,
        universe_id: UniverseId,
    ) -> Result<Vec<HostedWorldSummary>, WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(universe_id)?;
        let world_ids = core.infra.kafka.world_ids();
        let mut worlds = Vec::new();
        for world_id in world_ids {
            if core.ensure_registered_world(universe_id, world_id).is_ok()
                && let Ok(summary) = core.world_summary(world_id)
            {
                worlds.push(summary);
            }
        }
        worlds.sort_by_key(|world| world.world_id);
        Ok(worlds)
    }

    pub(super) fn lock_core(&self) -> Result<MutexGuard<'_, HostedWorkerCore>, WorkerError> {
        self.core.lock().map_err(|_| WorkerError::RuntimePoisoned)
    }
}

impl BlobBackend for HostedWorkerRuntime {
    fn put_blob(
        &self,
        universe_id: UniverseId,
        bytes: &[u8],
    ) -> Result<Hash, aos_node::BackendError> {
        HostedWorkerRuntime::put_blob(self, universe_id, bytes).map_err(|err| {
            aos_node::BackendError::Persist(aos_node::PersistError::backend(err.to_string()))
        })
    }

    fn get_blob(
        &self,
        universe_id: UniverseId,
        hash: Hash,
    ) -> Result<Vec<u8>, aos_node::BackendError> {
        HostedWorkerRuntime::get_blob(self, universe_id, hash).map_err(|err| {
            aos_node::BackendError::Persist(aos_node::PersistError::backend(err.to_string()))
        })
    }

    fn has_blob(
        &self,
        universe_id: UniverseId,
        hash: Hash,
    ) -> Result<bool, aos_node::BackendError> {
        HostedWorkerRuntime::blob_metadata(self, universe_id, hash).map_err(|err| {
            aos_node::BackendError::Persist(aos_node::PersistError::backend(err.to_string()))
        })
    }
}

impl HostedWorkerCore {
    pub(super) fn bootstrap_recovery(&mut self) -> Result<(), WorkerError> {
        self.infra.kafka.recover_from_broker()?;
        Ok(())
    }

    pub(super) fn require_default_universe(
        &self,
        _universe_id: UniverseId,
    ) -> Result<(), WorkerError> {
        Ok(())
    }
}
