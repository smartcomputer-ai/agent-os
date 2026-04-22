use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use aos_air_types::{AirNode, Manifest, OpKind};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effect_types::HashRef;
use aos_kernel::{Consistency, StateReader, Store, WorldInput};
use aos_node::control::{
    AcceptWaitQuery, DefGetResponse, DefsListResponse, HeadInfoResponse, JournalEntriesResponse,
    JournalEntryResponse, ManifestResponse, ManifestSummary, RawJournalEntriesResponse,
    RawJournalEntryResponse, RouteSummary, StateCellSummary, StateGetResponse, StateListResponse,
    WorkspaceResolveResponse,
};
use aos_node::{
    BlobBackend, CborPayload, CheckpointBackend, CommandIngress, CommandRecord, CreateWorldRequest,
    EffectRuntimeEvent, HostControl, JournalBackend, LocalStatePaths, ReceiptIngress,
    SubmissionEnvelope, SubmissionPayload, UniverseId, WorldConfig, WorldId, WorldInventoryBackend,
    WorldLogFrame, validate_create_world_request,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Deserialize;
use uuid::Uuid;

use crate::blobstore::{BlobStoreConfig, HostedCas};
use crate::kafka::{KafkaConfig, PartitionLogEntry};

use super::commands::{
    command_submit_response, synthesize_queued_command_record, world_control_from_command_payload,
};
use super::core::{AcceptedSubmission, AckRef, SchedulerMsg};
use super::types::{
    AcceptFlushWaiter, CreateWorldAccepted, HostedJournalInfra, HostedWorkerCore,
    HostedWorkerInfra, HostedWorldSummary, WorkerError,
};
use super::util::{
    default_state_root, resolve_cbor_payload, temp_embedded_state_root, unix_time_ns,
};

#[derive(Clone)]
pub struct HostedWorkerRuntime {
    core: Arc<Mutex<HostedWorkerCore>>,
    paths: Arc<LocalStatePaths>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnedWorldActivationSummary {
    pub attempted: usize,
    pub opened: usize,
    pub failed: usize,
}

impl std::fmt::Debug for HostedWorkerRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedWorkerRuntime")
            .finish_non_exhaustive()
    }
}

impl HostedWorkerRuntime {
    fn with_active_world<T>(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        f: impl FnOnce(&super::types::ActiveWorld) -> Result<T, WorkerError>,
    ) -> Result<T, WorkerError> {
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
        f(world)
    }

    fn submit_envelope_locked(
        core: &mut HostedWorkerCore,
        submission: SubmissionEnvelope,
    ) -> Result<u64, WorkerError> {
        let accept_token = Self::next_accept_token_locked(core);
        Self::submit_envelope_with_accept_token_locked(core, submission, accept_token)?;
        Ok(accept_token)
    }

    fn submit_envelope_with_accept_token_locked(
        core: &mut HostedWorkerCore,
        submission: SubmissionEnvelope,
        accept_token: u64,
    ) -> Result<(), WorkerError> {
        let accepted = AcceptedSubmission {
            ack_ref: AckRef::DirectAccept { accept_token },
            envelope: submission,
        };
        if let Some(scheduler_tx) = core.scheduler_tx.clone() {
            if scheduler_tx
                .send(SchedulerMsg::Accepted(accepted.clone()))
                .is_ok()
            {
                return Ok(());
            }
        }
        let _ = core.handle_accepted_submission(accepted)?;
        Ok(())
    }

    fn next_accept_token_locked(core: &mut HostedWorkerCore) -> u64 {
        core.state.next_accept_token = core.state.next_accept_token.saturating_add(1);
        core.state.next_accept_token
    }

    fn submit_envelope_inline_with_accept_token_locked(
        core: &mut HostedWorkerCore,
        submission: SubmissionEnvelope,
        accept_token: u64,
    ) -> Result<(), WorkerError> {
        let accepted = AcceptedSubmission {
            ack_ref: AckRef::DirectAccept { accept_token },
            envelope: submission,
        };
        let _ = core.handle_accepted_submission(accepted)?;
        Ok(())
    }

    fn register_accept_waiter_locked(
        core: &mut HostedWorkerCore,
        accept_token: u64,
    ) -> Arc<AcceptFlushWaiter> {
        let waiter = Arc::new(AcceptFlushWaiter::default());
        core.state
            .accept_waiters
            .insert(accept_token, Arc::clone(&waiter));
        waiter
    }

    fn remove_accept_waiter_locked(core: &mut HostedWorkerCore, accept_token: u64) {
        core.state.accept_waiters.remove(&accept_token);
    }

    fn wait_for_flush_handle(
        &self,
        accept_token: u64,
        wait: &AcceptWaitQuery,
        waiter: &Arc<AcceptFlushWaiter>,
    ) -> Result<(), WorkerError> {
        if !wait.wait_for_flush {
            return Ok(());
        }
        if waiter.wait(wait.timeout()) {
            return Ok(());
        }
        let mut core = self.lock_core()?;
        if core
            .state
            .accept_waiters
            .get(&accept_token)
            .is_some_and(|pending| Arc::ptr_eq(pending, waiter))
        {
            core.state.accept_waiters.remove(&accept_token);
        }
        Err(WorkerError::WaitForFlushTimedOut {
            accept_token,
            timeout_ms: wait.wait_timeout_ms.unwrap_or(u64::MAX),
        })
    }

    pub fn new_kafka(partition_count: u32) -> Result<Self, WorkerError> {
        Self::new_kafka_with_default_blobstore(
            partition_count,
            default_state_root()?,
            aos_node::UniverseId::nil(),
        )
    }

    pub fn new_kafka_with_default_universe(
        partition_count: u32,
        state_root: impl Into<PathBuf>,
    ) -> Result<Self, WorkerError> {
        Self::new_kafka_with_default_blobstore(
            partition_count,
            state_root,
            aos_node::UniverseId::nil(),
        )
    }

    pub fn new_kafka_with_default_blobstore(
        partition_count: u32,
        state_root: impl Into<PathBuf>,
        default_universe_id: UniverseId,
    ) -> Result<Self, WorkerError> {
        Self::new_kafka_with_state_root_and_universe(
            partition_count,
            state_root,
            default_universe_id,
            KafkaConfig::default(),
            BlobStoreConfig::default(),
        )
    }

    pub fn new_kafka_with_state_root(
        partition_count: u32,
        state_root: impl Into<PathBuf>,
        kafka_config: KafkaConfig,
        blobstore_config: BlobStoreConfig,
    ) -> Result<Self, WorkerError> {
        Self::new_kafka_with_state_root_and_universe(
            partition_count,
            state_root,
            aos_node::UniverseId::nil(),
            kafka_config,
            blobstore_config,
        )
    }

    pub fn new_kafka_with_state_root_and_universe(
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
        let journal = HostedJournalInfra::new(partition_count, kafka_config)?;
        let mut core = HostedWorkerCore {
            infra: HostedWorkerInfra {
                default_universe_id,
                paths: paths.clone(),
                blobstore_config: blobstore_config.clone(),
                vault: crate::vault::HostedVault::new_persistent(blobstore_config.clone(), &paths)
                    .map_err(|err| {
                        WorkerError::Build(anyhow::anyhow!("initialize node vault: {err}"))
                    })?,
                world_config,
                journal,
                stores_by_domain: Default::default(),
                checkpoints: Default::default(),
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
            max_uncommitted_slices_per_world: 256,
            debug_skip_flush_commit: false,
            debug_fail_after_next_flush_commit: false,
        };

        if core.infra.journal.is_broker()
            && !blobstore_config
                .bucket
                .as_ref()
                .is_some_and(|value: &String| !value.trim().is_empty())
        {
            return Err(WorkerError::Persist(aos_node::PersistError::validation(
                "Kafka journal mode requires AOS_BLOBSTORE_BUCKET or AOS_S3_BUCKET",
            )));
        }

        core.bootstrap_recovery()?;

        Ok(Self {
            core: Arc::new(Mutex::new(core)),
            paths: Arc::new(paths),
        })
    }

    pub fn new_embedded_kafka(partition_count: u32) -> Result<Self, WorkerError> {
        Self::new_embedded_kafka_with_state_root_and_universe(
            partition_count,
            temp_embedded_state_root(),
            aos_node::UniverseId::nil(),
        )
    }

    pub fn new_embedded_kafka_with_state_root(
        partition_count: u32,
        state_root: impl Into<PathBuf>,
    ) -> Result<Self, WorkerError> {
        Self::new_embedded_kafka_with_state_root_and_universe(
            partition_count,
            state_root,
            aos_node::UniverseId::nil(),
        )
    }

    pub fn new_embedded_kafka_with_state_root_and_universe(
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
        let journal = HostedJournalInfra::new_embedded(partition_count, KafkaConfig::default())?;
        let mut core = HostedWorkerCore {
            infra: HostedWorkerInfra {
                default_universe_id,
                paths: paths.clone(),
                blobstore_config: BlobStoreConfig::default(),
                vault: crate::vault::HostedVault::new_persistent(
                    BlobStoreConfig::default(),
                    &paths,
                )
                .map_err(|err| {
                    WorkerError::Build(anyhow::anyhow!("initialize node vault: {err}"))
                })?,
                world_config,
                journal,
                stores_by_domain: Default::default(),
                checkpoints: Default::default(),
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
            max_uncommitted_slices_per_world: 256,
            debug_skip_flush_commit: false,
            debug_fail_after_next_flush_commit: false,
        };
        core.bootstrap_recovery()?;

        Ok(Self {
            core: Arc::new(Mutex::new(core)),
            paths: Arc::new(paths),
        })
    }

    pub fn new_sqlite_with_state_root(state_root: impl Into<PathBuf>) -> Result<Self, WorkerError> {
        Self::new_sqlite_with_state_root_and_universe(
            state_root,
            aos_node::UniverseId::nil(),
            BlobStoreConfig::default(),
        )
    }

    pub fn new_sqlite_with_state_root_and_universe(
        state_root: impl Into<PathBuf>,
        default_universe_id: UniverseId,
        blobstore_config: BlobStoreConfig,
    ) -> Result<Self, WorkerError> {
        let paths = LocalStatePaths::new(state_root.into());
        paths.ensure_root().map_err(|err| {
            WorkerError::Persist(aos_node::PersistError::backend(err.to_string()))
        })?;

        let (effect_event_tx, effect_event_rx) = tokio::sync::mpsc::channel(1024);
        let world_config = WorldConfig::from_env_with_fallback_module_cache_dir(None);
        let journal = HostedJournalInfra::new_sqlite(&paths)?;
        let mut core = HostedWorkerCore {
            infra: HostedWorkerInfra {
                default_universe_id,
                paths: paths.clone(),
                blobstore_config: blobstore_config.clone(),
                vault: crate::vault::HostedVault::new_persistent(blobstore_config.clone(), &paths)
                    .map_err(|err| {
                        WorkerError::Build(anyhow::anyhow!("initialize node vault: {err}"))
                    })?,
                world_config,
                journal,
                stores_by_domain: Default::default(),
                checkpoints: Default::default(),
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
            max_uncommitted_slices_per_world: 256,
            debug_skip_flush_commit: false,
            debug_fail_after_next_flush_commit: false,
        };
        core.bootstrap_recovery()?;

        Ok(Self {
            core: Arc::new(Mutex::new(core)),
            paths: Arc::new(paths),
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
        self.create_world_with_wait(
            universe_id,
            request,
            AcceptWaitQuery {
                wait_for_flush: true,
                wait_timeout_ms: None,
            },
        )
    }

    pub fn create_world_with_wait(
        &self,
        universe_id: UniverseId,
        request: CreateWorldRequest,
        wait: AcceptWaitQuery,
    ) -> Result<CreateWorldAccepted, WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(universe_id)?;
        validate_create_world_request(&request)?;
        let world_id = request
            .world_id
            .unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
        let request = CreateWorldRequest {
            world_id: Some(world_id),
            ..request
        };
        let submission_id = format!("create-{world_id}-{}", Uuid::new_v4());
        let submission = SubmissionEnvelope::host_control(
            submission_id.clone(),
            universe_id,
            world_id,
            HostControl::CreateWorld { request },
        );
        let accept_token = if wait.wait_for_flush {
            let accept_token = Self::next_accept_token_locked(&mut core);
            let waiter = Self::register_accept_waiter_locked(&mut core, accept_token);
            if let Err(err) = Self::submit_envelope_inline_with_accept_token_locked(
                &mut core,
                submission,
                accept_token,
            ) {
                Self::remove_accept_waiter_locked(&mut core, accept_token);
                return Err(err);
            }
            let mut profile = super::types::SupervisorRunProfile::default();
            if let Err(err) = core.drive_scheduler_until_quiescent(true, &mut profile) {
                Self::remove_accept_waiter_locked(&mut core, accept_token);
                return Err(err);
            }
            drop(core);
            self.wait_for_flush_handle(accept_token, &wait, &waiter)?;
            accept_token
        } else {
            Self::submit_envelope_locked(&mut core, submission)?
        };
        Ok(CreateWorldAccepted {
            submission_id,
            accept_token,
            world_id,
        })
    }

    pub fn submit_submission(&self, submission: SubmissionEnvelope) -> Result<u64, WorkerError> {
        let mut core = self.lock_core()?;
        Self::submit_envelope_locked(&mut core, submission)
    }

    pub fn drive_until_quiescent(
        &self,
        force_flush: bool,
    ) -> Result<super::types::SupervisorRunProfile, WorkerError> {
        let mut core = self.lock_core()?;
        let mut profile = super::types::SupervisorRunProfile::default();
        core.drive_scheduler_until_quiescent(force_flush, &mut profile)?;
        Ok(profile)
    }

    pub fn submit_event(
        &self,
        request: super::types::SubmitEventRequest,
    ) -> Result<super::types::SubmissionAccepted, WorkerError> {
        self.submit_event_with_wait(request, AcceptWaitQuery::default())
    }

    pub fn submit_event_with_wait(
        &self,
        request: super::types::SubmitEventRequest,
        wait: AcceptWaitQuery,
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
        let accept_token = if wait.wait_for_flush {
            let accept_token = Self::next_accept_token_locked(&mut core);
            let waiter = Self::register_accept_waiter_locked(&mut core, accept_token);
            let has_background_scheduler = core.scheduler_tx.is_some();
            if let Err(err) =
                Self::submit_envelope_with_accept_token_locked(&mut core, submission, accept_token)
            {
                Self::remove_accept_waiter_locked(&mut core, accept_token);
                return Err(err);
            }
            if !has_background_scheduler {
                let mut profile = super::types::SupervisorRunProfile::default();
                if let Err(err) = core.drive_scheduler_until_quiescent(true, &mut profile) {
                    Self::remove_accept_waiter_locked(&mut core, accept_token);
                    return Err(err);
                }
            }
            drop(core);
            self.wait_for_flush_handle(accept_token, &wait, &waiter)?;
            accept_token
        } else {
            Self::submit_envelope_locked(&mut core, submission)?
        };
        Ok(super::types::SubmissionAccepted {
            submission_id,
            accept_token,
            world_epoch: registered_world_epoch,
        })
    }

    pub fn submit_receipt(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        ingress: ReceiptIngress,
    ) -> Result<super::types::SubmissionAccepted, WorkerError> {
        self.submit_receipt_with_wait(universe_id, world_id, ingress, AcceptWaitQuery::default())
    }

    pub fn submit_receipt_with_wait(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        ingress: ReceiptIngress,
        wait: AcceptWaitQuery,
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
                    status: ingress.status,
                    payload_cbor: resolve_cbor_payload(store.as_ref(), &ingress.payload)?,
                    cost_cents: ingress.cost_cents,
                    signature: ingress.signature,
                }),
            },
        };
        let accept_token = if wait.wait_for_flush {
            let accept_token = Self::next_accept_token_locked(&mut core);
            let waiter = Self::register_accept_waiter_locked(&mut core, accept_token);
            let has_background_scheduler = core.scheduler_tx.is_some();
            if let Err(err) =
                Self::submit_envelope_with_accept_token_locked(&mut core, submission, accept_token)
            {
                Self::remove_accept_waiter_locked(&mut core, accept_token);
                return Err(err);
            }
            if !has_background_scheduler {
                let mut profile = super::types::SupervisorRunProfile::default();
                if let Err(err) = core.drive_scheduler_until_quiescent(true, &mut profile) {
                    Self::remove_accept_waiter_locked(&mut core, accept_token);
                    return Err(err);
                }
            }
            drop(core);
            self.wait_for_flush_handle(accept_token, &wait, &waiter)?;
            accept_token
        } else {
            Self::submit_envelope_locked(&mut core, submission)?
        };
        Ok(super::types::SubmissionAccepted {
            submission_id,
            accept_token,
            world_epoch: registered_world_epoch,
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
            .checkpoint_backend_for_domain_mut(registered_universe)?
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
            .checkpoint_backend_for_domain_mut(universe_id)?
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
    ) -> Result<aos_node::control::CommandSubmitResponse, WorkerError> {
        self.submit_command_with_wait(
            universe_id,
            world_id,
            command,
            command_id,
            actor,
            payload,
            AcceptWaitQuery::default(),
        )
    }

    pub fn submit_command_with_wait<T: serde::Serialize>(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        payload: &T,
        wait: AcceptWaitQuery,
    ) -> Result<aos_node::control::CommandSubmitResponse, WorkerError> {
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
                .checkpoint_backend_for_domain_mut(registered_universe)?
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
        if wait.wait_for_flush {
            let accept_token = Self::next_accept_token_locked(&mut core);
            let waiter = Self::register_accept_waiter_locked(&mut core, accept_token);
            let has_background_scheduler = core.scheduler_tx.is_some();
            if let Err(err) =
                Self::submit_envelope_with_accept_token_locked(&mut core, submission, accept_token)
            {
                Self::remove_accept_waiter_locked(&mut core, accept_token);
                return Err(err);
            }
            if let Err(err) = core
                .infra
                .checkpoint_backend_for_domain_mut(registered_universe)?
                .put_command_record(world_id, queued.clone())
            {
                Self::remove_accept_waiter_locked(&mut core, accept_token);
                return Err(WorkerError::LogFirst(err));
            }
            if !has_background_scheduler {
                let mut profile = super::types::SupervisorRunProfile::default();
                if let Err(err) = core.drive_scheduler_until_quiescent(true, &mut profile) {
                    Self::remove_accept_waiter_locked(&mut core, accept_token);
                    return Err(err);
                }
            }
            drop(core);
            self.wait_for_flush_handle(accept_token, &wait, &waiter)?;
            let record = self.get_command_record(registered_universe, world_id, &command_id)?;
            return Ok(command_submit_response(world_id, record));
        }
        let _ = Self::submit_envelope_locked(&mut core, submission)?;
        core.infra
            .checkpoint_backend_for_domain_mut(registered_universe)?
            .put_command_record(world_id, queued.clone())?;
        Ok(command_submit_response(world_id, queued))
    }

    pub fn owned_worlds(&self) -> Result<Vec<WorldId>, WorkerError> {
        Ok(self
            .lock_core()?
            .state
            .owned_worlds
            .iter()
            .copied()
            .collect())
    }

    pub fn scheduler_attached(&self) -> Result<bool, WorkerError> {
        Ok(self.lock_core()?.scheduler_tx.is_some())
    }

    pub fn activate_owned_worlds_best_effort(
        &self,
    ) -> Result<OwnedWorldActivationSummary, WorkerError> {
        let (universe_id_hint, world_ids) = {
            let core = self.lock_core()?;
            (
                core.infra.default_universe_id,
                core.state.owned_worlds.iter().copied().collect::<Vec<_>>(),
            )
        };
        let attempted = world_ids.len();
        let mut opened = 0usize;
        let mut failed = 0usize;

        for world_id in world_ids {
            let result = (|| {
                let mut core = self.lock_core()?;
                core.ensure_registered_world(universe_id_hint, world_id)?;
                core.activate_world(world_id)
            })();
            match result {
                Ok(()) => {
                    opened = opened.saturating_add(1);
                }
                Err(err) => {
                    failed = failed.saturating_add(1);
                    tracing::warn!(
                        world_id = %world_id,
                        error = %err,
                        "aos-node owned world warmup failed"
                    );
                }
            }
        }

        Ok(OwnedWorldActivationSummary {
            attempted,
            opened,
            failed,
        })
    }

    pub fn configure_owned_worlds(
        &self,
        owned_worlds: impl IntoIterator<Item = WorldId>,
    ) -> Result<(), WorkerError> {
        let mut core = self.lock_core()?;
        core.configure_owned_worlds(owned_worlds.into_iter().collect());
        Ok(())
    }

    pub fn kafka_config(&self) -> Result<KafkaConfig, WorkerError> {
        self.lock_core()?
            .infra
            .journal
            .kafka_config()
            .cloned()
            .ok_or_else(|| {
                WorkerError::Persist(aos_node::PersistError::validation(
                    "kafka config is only available when Kafka is the selected journal backend",
                ))
            })
    }

    pub(crate) fn partition_count(&self) -> Result<u32, WorkerError> {
        Ok(self.lock_core()?.infra.journal.partition_count())
    }

    pub(crate) fn journal_topic(&self) -> Result<String, WorkerError> {
        self.lock_core()?
            .infra
            .journal
            .kafka_config()
            .map(|config| config.journal_topic.clone())
            .ok_or_else(|| {
                WorkerError::Persist(aos_node::PersistError::validation(
                    "journal topic is only available when Kafka is the selected journal backend",
                ))
            })
    }

    pub(crate) fn partition_entries(
        &self,
        partition: u32,
    ) -> Result<Vec<PartitionLogEntry>, WorkerError> {
        let mut core = self.lock_core()?;
        let Some(journal_topic) = core
            .infra
            .journal
            .kafka_config()
            .map(|config| config.journal_topic.clone())
        else {
            return Err(WorkerError::Persist(aos_node::PersistError::validation(
                "partition entries are only available when Kafka is the selected journal backend",
            )));
        };
        core.infra
            .journal
            .recover_partition_from_broker(partition)?;
        Ok(core
            .infra
            .journal
            .partition_entries(&journal_topic, partition)
            .to_vec())
    }

    pub(crate) fn recover_partition(&self, partition: u32) -> Result<(), WorkerError> {
        let mut core = self.lock_core()?;
        if core.infra.journal.kafka_config().is_none() {
            return Err(WorkerError::Persist(aos_node::PersistError::validation(
                "partition recovery is only available when Kafka is the selected journal backend",
            )));
        }
        core.infra
            .journal
            .recover_partition_from_broker(partition)
            .map_err(WorkerError::LogFirst)
    }

    pub fn world_frames(&self, world_id: WorldId) -> Result<Vec<WorldLogFrame>, WorkerError> {
        let mut core = self.lock_core()?;
        JournalBackend::refresh_world(&mut core.infra.journal, world_id)
            .map_err(WorkerError::LogFirst)?;
        JournalBackend::world_frames(&core.infra.journal, world_id).map_err(WorkerError::LogFirst)
    }

    pub fn world_tail_frames(
        &self,
        world_id: WorldId,
        after_world_seq: u64,
        cursor: Option<aos_node::WorldJournalCursor>,
    ) -> Result<Vec<WorldLogFrame>, WorkerError> {
        let mut core = self.lock_core()?;
        JournalBackend::refresh_world(&mut core.infra.journal, world_id)
            .map_err(WorkerError::LogFirst)?;
        JournalBackend::world_tail_frames(
            &core.infra.journal,
            world_id,
            after_world_seq,
            cursor.as_ref(),
        )
        .map_err(WorkerError::LogFirst)
    }

    pub fn refresh_journal_source(&self) -> Result<(), WorkerError> {
        let mut core = self.lock_core()?;
        JournalBackend::refresh_all(&mut core.infra.journal).map_err(WorkerError::LogFirst)?;
        Ok(())
    }

    pub fn latest_world_checkpoint(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<Option<aos_node::WorldCheckpointRef>, WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(universe_id)?;
        core.infra
            .checkpoint_backend_for_domain_mut(universe_id)?
            .latest_world_checkpoint(world_id)
            .map_err(WorkerError::LogFirst)
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
        Ok(self.lock_core()?.infra.journal.is_broker())
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

    pub fn debug_fail_next_batch_commit(&self) -> Result<(), WorkerError> {
        let mut core = self.lock_core()?;
        core.infra.journal.debug_fail_next_batch_commit();
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

    pub fn active_baseline(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<aos_node::SnapshotRecord, WorkerError> {
        self.with_active_world(universe_id, world_id, |world| {
            Ok(world.active_baseline.clone())
        })
    }

    pub fn checkpoint_world_now(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<usize, WorkerError> {
        let mut core = self.lock_core()?;
        core.require_default_universe(universe_id)?;
        core.ensure_registered_world(universe_id, world_id)?;
        if !core.state.active_worlds.contains_key(&world_id) {
            core.activate_world(world_id)?;
        }
        let mut profile = super::types::SupervisorRunProfile::default();
        core.drive_scheduler_until_quiescent(true, &mut profile)?;
        let published = core.publish_due_checkpoints(Duration::ZERO, None)?;
        core.drive_scheduler_until_quiescent(true, &mut profile)?;
        Ok(published)
    }

    pub fn manifest(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<ManifestResponse, WorkerError> {
        self.with_active_world(universe_id, world_id, |world| {
            let manifest = world.kernel.get_manifest(Consistency::Head)?.value;
            Ok(ManifestResponse {
                journal_head: world.kernel.heights().head,
                manifest_hash: world.kernel.manifest_hash().to_hex(),
                summary: manifest_summary(&manifest, |name| world.kernel.get_def(name)),
                manifest,
            })
        })
    }

    pub fn defs_list(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        kinds: Option<Vec<String>>,
        prefix: Option<String>,
    ) -> Result<DefsListResponse, WorkerError> {
        self.with_active_world(universe_id, world_id, |world| {
            Ok(DefsListResponse {
                journal_head: world.kernel.heights().head,
                manifest_hash: world.kernel.manifest_hash().to_hex(),
                defs: world.kernel.list_defs(kinds.as_deref(), prefix.as_deref()),
            })
        })
    }

    pub fn def_get(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        name: &str,
    ) -> Result<DefGetResponse, WorkerError> {
        self.with_active_world(universe_id, world_id, |world| {
            let def = world.kernel.get_def(name).ok_or_else(|| {
                WorkerError::Persist(aos_node::PersistError::not_found(format!(
                    "definition '{name}' not found"
                )))
            })?;
            Ok(DefGetResponse {
                journal_head: world.kernel.heights().head,
                manifest_hash: world.kernel.manifest_hash().to_hex(),
                def,
            })
        })
    }

    pub fn state_get(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        key: Option<Vec<u8>>,
    ) -> Result<StateGetResponse, WorkerError> {
        self.with_active_world(universe_id, world_id, |world| {
            let key_bytes = key.unwrap_or_default();
            let state_read = world.kernel.get_workflow_state(
                workflow,
                Some(key_bytes.as_slice()),
                Consistency::Head,
            )?;
            let state = state_read.value;
            let state_hash = state.as_ref().map(|bytes| Hash::of_bytes(bytes).to_hex());
            let size = state.as_ref().map(|bytes| bytes.len() as u64).unwrap_or(0);
            let cell = state_hash.map(|state_hash| StateCellSummary {
                journal_head: state_read.meta.journal_height,
                workflow: workflow.to_owned(),
                key_hash: Hash::of_bytes(&key_bytes).as_bytes().to_vec(),
                key_bytes: key_bytes.clone(),
                state_hash,
                size,
                last_active_ns: 0,
            });
            Ok(StateGetResponse {
                journal_head: state_read.meta.journal_height,
                workflow: workflow.to_owned(),
                key_b64: Some(BASE64_STANDARD.encode(&key_bytes)),
                cell,
                state_b64: state.map(|bytes| BASE64_STANDARD.encode(bytes)),
            })
        })
    }

    pub fn state_list(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        limit: u32,
    ) -> Result<StateListResponse, WorkerError> {
        self.with_active_world(universe_id, world_id, |world| {
            let mut cells = world
                .list_cells(workflow)?
                .into_iter()
                .map(|cell| StateCellSummary {
                    journal_head: world.kernel.heights().head,
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
                journal_head: world.kernel.heights().head,
                workflow: workflow.to_owned(),
                cells,
            })
        })
    }

    pub fn workspace_resolve(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workspace: &str,
        version: Option<u64>,
    ) -> Result<WorkspaceResolveResponse, WorkerError> {
        #[derive(Debug, Default, Deserialize)]
        struct WorkspaceHistoryState {
            latest: u64,
            versions: std::collections::BTreeMap<u64, WorkspaceCommitMetaState>,
        }

        #[derive(Debug, Deserialize)]
        struct WorkspaceCommitMetaState {
            root_hash: String,
        }

        self.with_active_world(universe_id, world_id, |world| {
            let key = serde_cbor::to_vec(&workspace.to_owned())?;
            let history = world
                .kernel
                .get_workflow_state("sys/Workspace@1", Some(key.as_slice()), Consistency::Head)?
                .value;
            let receipt = if let Some(bytes) = history {
                let history: WorkspaceHistoryState = serde_cbor::from_slice(&bytes)?;
                let head = Some(history.latest);
                let target = version.unwrap_or(history.latest);
                if let Some(entry) = history.versions.get(&target) {
                    aos_effect_types::WorkspaceResolveReceipt {
                        exists: true,
                        resolved_version: Some(target),
                        head,
                        root_hash: Some(HashRef::new(entry.root_hash.clone()).map_err(|err| {
                            WorkerError::Persist(aos_node::PersistError::validation(
                                err.to_string(),
                            ))
                        })?),
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
        })
    }

    pub fn journal_head(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<HeadInfoResponse, WorkerError> {
        self.with_active_world(universe_id, world_id, |world| {
            let bounds = world.kernel.journal_bounds();
            Ok(HeadInfoResponse {
                journal_head: world.kernel.heights().head,
                retained_from: bounds.retained_from,
                manifest_hash: Some(world.kernel.manifest_hash().to_hex()),
            })
        })
    }

    pub fn journal_entries(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<JournalEntriesResponse, WorkerError> {
        self.with_active_world(universe_id, world_id, |world| {
            let bounds = world.kernel.journal_bounds();
            let from = from.max(bounds.retained_from);
            let entries = world.kernel.dump_journal_from(from)?;
            let mut rows = Vec::new();
            let mut next_from = from;
            for entry in entries.into_iter().take(limit as usize) {
                let record_value = serde_cbor::from_slice::<serde_cbor::Value>(&entry.payload)
                    .ok()
                    .and_then(|value| serde_json::to_value(value).ok())
                    .unwrap_or_else(|| {
                        serde_json::json!({ "payload_b64": BASE64_STANDARD.encode(&entry.payload) })
                    });
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
        })
    }

    pub fn journal_entries_raw(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<RawJournalEntriesResponse, WorkerError> {
        self.with_active_world(universe_id, world_id, |world| {
            let bounds = world.kernel.journal_bounds();
            let from = from.max(bounds.retained_from);
            let entries = world.kernel.dump_journal_from(from)?;
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
        })
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
        let world_ids = core
            .state
            .owned_worlds
            .iter()
            .copied()
            .chain(core.state.registered_worlds.keys().copied())
            .collect::<std::collections::BTreeSet<_>>();
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

fn manifest_summary(
    manifest: &Manifest,
    mut get_def: impl FnMut(&str) -> Option<AirNode>,
) -> ManifestSummary {
    let mut workflow_op_count = 0;
    let mut effect_op_count = 0;

    for op_ref in &manifest.ops {
        if let Some(AirNode::Defop(op)) = get_def(op_ref.name.as_str()) {
            match op.op_kind {
                OpKind::Workflow => workflow_op_count += 1,
                OpKind::Effect => effect_op_count += 1,
            }
        }
    }

    let routes = manifest
        .routing
        .as_ref()
        .map(|routing| {
            routing
                .subscriptions
                .iter()
                .map(|route| RouteSummary {
                    event: route.event.as_str().to_string(),
                    op: route.op.as_str().to_string(),
                    key_field: route.key_field.clone(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    ManifestSummary {
        schema_count: manifest.schemas.len(),
        module_count: manifest.modules.len(),
        op_count: manifest.ops.len(),
        workflow_op_count,
        effect_op_count,
        secret_count: manifest.secrets.len(),
        routing_subscription_count: routes.len(),
        routes,
    }
}

#[cfg(test)]
mod tests {
    use super::manifest_summary;
    use aos_air_types::{
        AirNode, HashRef, Manifest, NamedRef, Routing, RoutingSubscription, SchemaRef,
    };
    use serde_json::json;

    fn hash_ref(byte: char) -> HashRef {
        HashRef::new(format!("sha256:{}", byte.to_string().repeat(64))).expect("hash ref")
    }

    fn named_ref(name: &str, byte: char) -> NamedRef {
        NamedRef {
            name: name.into(),
            hash: hash_ref(byte),
        }
    }

    fn workflow_op(name: &str) -> AirNode {
        serde_json::from_value(json!({
            "$kind": "defop",
            "name": name,
            "op_kind": "workflow",
            "workflow": {
                "state": "demo/State@1",
                "event": "demo/Event@1",
                "effects_emitted": []
            },
            "impl": {
                "module": "demo/workflow_wasm@1",
                "entrypoint": "workflow:handle"
            }
        }))
        .expect("workflow op")
    }

    fn effect_op(name: &str) -> AirNode {
        serde_json::from_value(json!({
            "$kind": "defop",
            "name": name,
            "op_kind": "effect",
            "effect": {
                "params": "demo/EffectParams@1",
                "receipt": "demo/EffectReceipt@1"
            },
            "impl": {
                "module": "demo/effect_adapter@1",
                "entrypoint": "effect:run"
            }
        }))
        .expect("effect op")
    }

    #[test]
    fn manifest_summary_counts_workflow_and_effect_ops_and_routes_by_op() {
        let manifest = Manifest {
            air_version: "2".into(),
            schemas: vec![named_ref("demo/Event@1", '1')],
            modules: vec![named_ref("demo/workflow_wasm@1", '2')],
            ops: vec![
                named_ref("demo/workflow@1", '3'),
                named_ref("demo/http.request@1", '4'),
            ],
            secrets: vec![named_ref("demo/secret@1", '5')],
            routing: Some(Routing {
                subscriptions: vec![RoutingSubscription {
                    event: SchemaRef::new("demo/Event@1").expect("schema ref"),
                    op: "demo/workflow@1".into(),
                    key_field: Some("tenant_id".into()),
                }],
            }),
        };

        let summary = manifest_summary(&manifest, |name| match name {
            "demo/workflow@1" => Some(workflow_op(name)),
            "demo/http.request@1" => Some(effect_op(name)),
            _ => None,
        });

        assert_eq!(summary.schema_count, 1);
        assert_eq!(summary.module_count, 1);
        assert_eq!(summary.op_count, 2);
        assert_eq!(summary.workflow_op_count, 1);
        assert_eq!(summary.effect_op_count, 1);
        assert_eq!(summary.secret_count, 1);
        assert_eq!(summary.routing_subscription_count, 1);
        assert_eq!(summary.routes[0].event, "demo/Event@1");
        assert_eq!(summary.routes[0].op, "demo/workflow@1");
        assert_eq!(summary.routes[0].key_field.as_deref(), Some("tenant_id"));
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
        JournalBackend::refresh_all(&mut self.infra.journal).map_err(WorkerError::LogFirst)?;
        let checkpoint_worlds = self
            .infra
            .checkpoint_backend_for_domain_mut(self.infra.default_universe_id)?
            .list_worlds()
            .map_err(WorkerError::LogFirst)?;
        self.state.owned_worlds.extend(checkpoint_worlds);
        self.state
            .owned_worlds
            .extend(JournalBackend::world_ids(&self.infra.journal));
        Ok(())
    }

    pub(super) fn require_default_universe(
        &self,
        _universe_id: UniverseId,
    ) -> Result<(), WorkerError> {
        Ok(())
    }
}
