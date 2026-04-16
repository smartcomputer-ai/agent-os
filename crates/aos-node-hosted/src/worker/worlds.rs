use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use aos_effect_adapters::config::EffectAdapterConfig;
use aos_kernel::journal::{Journal, JournalRecord};
use aos_kernel::{Kernel, KernelConfig, ManifestLoader, Store};
use aos_node::{
    CheckpointBackend, CreateWorldRequest, CreateWorldSource, EffectExecutionClass, EffectRuntime,
    SharedEffectRuntime, SnapshotRecord, TimerScheduler, UniverseId, WorldId, WorldLogFrame,
    classify_effect_kind, partition_for_world,
};
use uuid::Uuid;

use crate::blobstore::HostedCas;

use super::core::WorkItem;
use super::types::{
    ActiveWorld, AsyncWorldState, HostedWorkerCore, HostedWorldMetadata, HostedWorldSummary,
    PendingCreatedWorld, RegisteredWorld, WorkerError,
};
use super::util::{
    effect_intent_from_pending, kernel_snapshot_record, latest_snapshot_record, parse_hash_ref,
    reopen_kernel_from_frame_log, snapshot_record_from_checkpoint, snapshot_record_from_frames,
    unix_time_ns,
};

impl HostedWorkerCore {
    pub(super) fn log_world_created(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        world_epoch: u64,
        source_kind: &'static str,
        total_create_ms: u128,
        created_at_ns: u64,
        active_baseline_height: u64,
        next_world_seq: u64,
        initial_record_count: usize,
        manifest_hash: &str,
    ) {
        let partition = partition_for_world(world_id, self.infra.kafka.partition_count());
        tracing::info!(
            universe_id = %universe_id,
            world_id = %world_id,
            partition,
            world_epoch,
            source_kind,
            total_create_ms,
            created_at_ns,
            active_baseline_height,
            next_world_seq,
            initial_record_count,
            manifest_hash,
            "aos-node-hosted world created"
        );
    }

    pub(super) fn log_world_opened(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        world_epoch: u64,
        trigger: &'static str,
        total_open_ms: u128,
        active_baseline_height: u64,
        next_world_seq: u64,
        frame_count: usize,
        replay_frame_count: usize,
        pending_create_checkpoint: bool,
    ) {
        let partition = partition_for_world(world_id, self.infra.kafka.partition_count());
        tracing::info!(
            universe_id = %universe_id,
            world_id = %world_id,
            partition,
            world_epoch,
            trigger,
            total_open_ms,
            active_baseline_height,
            next_world_seq,
            frame_count,
            replay_frame_count,
            pending_create_checkpoint,
            "aos-node-hosted world opened"
        );
    }

    fn kernel_config_for_world(
        &self,
        universe_id: UniverseId,
    ) -> Result<KernelConfig, WorkerError> {
        let world_config = self.infra.world_config_for_domain(universe_id)?;
        Ok(world_config.apply_kernel_defaults(KernelConfig {
            universe_id: universe_id.as_uuid(),
            secret_resolver: Some(Arc::new(
                self.infra.vault.resolver_for_universe(universe_id),
            )),
            ..KernelConfig::default()
        }))
    }

    pub(super) fn reopen_active_world(
        &mut self,
        world_id: WorldId,
        drop_submission_ids: &[String],
    ) -> Result<(), WorkerError> {
        if let Some(mut async_state) = self.state.async_worlds.remove(&world_id) {
            async_state.abort_all_timers();
        }
        let (preserved_mailbox, mut accepted_submission_ids) = self
            .state
            .active_worlds
            .remove(&world_id)
            .map(|world| (world.mailbox, world.accepted_submission_ids))
            .unwrap_or_default();
        for submission_id in drop_submission_ids {
            accepted_submission_ids.remove(submission_id);
        }
        let open_started = Instant::now();
        let (reopened, async_state, metrics) =
            self.build_active_world(world_id, preserved_mailbox, accepted_submission_ids)?;
        let needs_projection_bootstrap = !reopened.projection_bootstrapped;
        self.log_world_opened(
            reopened.universe_id,
            world_id,
            reopened.world_epoch,
            "reopen",
            open_started.elapsed().as_millis(),
            reopened.active_baseline.height,
            reopened.next_world_seq,
            metrics.frame_count,
            metrics.replay_frame_count,
            reopened.pending_create_checkpoint,
        );
        self.state.active_worlds.insert(world_id, reopened);
        self.state.async_worlds.insert(world_id, async_state);
        self.mark_world_ready(world_id);
        if needs_projection_bootstrap {
            self.schedule_projection_updates_for_worlds(&[world_id])?;
        }
        Ok(())
    }

    pub(super) fn seed_world_direct(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
        request: CreateWorldRequest,
    ) -> Result<(), WorkerError> {
        let create_started = Instant::now();
        let source_kind = create_world_source_kind(&request);
        self.require_default_universe(universe_id)?;
        if self.state.registered_worlds.contains_key(&world_id)
            || self.state.active_worlds.contains_key(&world_id)
        {
            return Err(WorkerError::Persist(aos_node::PersistError::validation(
                format!("world {world_id} already exists"),
            )));
        }

        let (registered, mut active, async_state) = self.build_seeded_world(world_id, request)?;
        let manifest_hash = registered.manifest_hash.clone();
        let initial_frame = initial_frame_for_kernel(
            active.universe_id,
            active.world_id,
            active.world_epoch,
            &active.kernel,
            active.next_world_seq,
        )?;
        let initial_record_count = initial_frame
            .as_ref()
            .map(|frame| frame.records.len())
            .unwrap_or_default();
        if let Some(frame) = initial_frame {
            self.infra.kafka.append_frame(frame.clone())?;
            active.next_world_seq = frame.world_seq_end.saturating_add(1);
            if let Some(snapshot) =
                snapshot_record_from_frames(std::slice::from_ref(&frame), |_| true)
            {
                active.active_baseline = snapshot;
            }
        }

        self.state.registered_worlds.insert(world_id, registered);
        self.state.active_worlds.insert(world_id, active);
        self.state.async_worlds.insert(world_id, async_state);
        self.rehydrate_runtime_work(world_id)?;
        let partition = partition_for_world(world_id, self.infra.kafka.partition_count());
        let _ = self.create_partition_checkpoint(partition, unix_time_ns(), 0, None, "seed")?;
        let world = self
            .state
            .active_worlds
            .get(&world_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        let total_create_ms = create_started.elapsed().as_millis();
        self.log_world_created(
            world.universe_id,
            world_id,
            world.world_epoch,
            source_kind,
            total_create_ms,
            world.created_at_ns,
            world.active_baseline.height,
            world.next_world_seq,
            initial_record_count,
            &manifest_hash,
        );
        self.log_world_opened(
            world.universe_id,
            world_id,
            world.world_epoch,
            "create",
            total_create_ms,
            world.active_baseline.height,
            world.next_world_seq,
            usize::from(initial_record_count > 0),
            0,
            world.pending_create_checkpoint,
        );
        self.schedule_projection_updates_for_worlds(&[world_id])?;
        Ok(())
    }

    pub(super) fn prepare_pending_created_world(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
        submission_id: &str,
        request: CreateWorldRequest,
    ) -> Result<Option<WorldLogFrame>, WorkerError> {
        self.require_default_universe(universe_id)?;
        let (registered, mut active, async_state) = self.build_seeded_world(world_id, request)?;
        active
            .accepted_submission_ids
            .insert(submission_id.to_owned());

        let initial_frame = initial_frame_for_kernel(
            active.universe_id,
            active.world_id,
            active.world_epoch,
            &active.kernel,
            active.next_world_seq,
        )?;
        if let Some(frame) = initial_frame.as_ref() {
            active.next_world_seq = frame.world_seq_end.saturating_add(1);
            if let Some(snapshot) =
                snapshot_record_from_frames(std::slice::from_ref(frame), |_| true)
            {
                active.active_baseline = snapshot;
            }
        }

        self.state.registered_worlds.insert(world_id, registered);
        self.state.active_worlds.insert(world_id, active);
        self.state.async_worlds.insert(world_id, async_state);
        self.state
            .pending_created_worlds
            .insert(world_id, PendingCreatedWorld);
        Ok(initial_frame)
    }

    fn build_seeded_world(
        &mut self,
        world_id: WorldId,
        request: CreateWorldRequest,
    ) -> Result<(RegisteredWorld, ActiveWorld, AsyncWorldState), WorkerError> {
        let universe_id = request.universe_id;
        let store = self.infra.store_for_domain(universe_id)?;
        let kernel_config = self.kernel_config_for_world(universe_id)?;

        let (loaded, manifest_hash, kernel, baseline) = match &request.source {
            CreateWorldSource::Manifest { manifest_hash } => {
                let manifest_hash = parse_hash_ref(manifest_hash)?;
                let loaded = ManifestLoader::load_from_hash(store.as_ref(), manifest_hash)?;
                let kernel = Kernel::from_loaded_manifest_with_config(
                    Arc::clone(&store),
                    loaded.clone(),
                    Journal::new(),
                    kernel_config,
                )?;
                let baseline = latest_snapshot_from_kernel(&kernel)?.ok_or_else(|| {
                    WorkerError::Persist(aos_node::PersistError::backend(
                        "manifest world produced no bootstrap snapshot",
                    ))
                })?;
                (loaded, manifest_hash.to_hex(), kernel, baseline)
            }
            CreateWorldSource::Seed { seed } => {
                let manifest_hash = seed.baseline.manifest_hash.as_ref().ok_or_else(|| {
                    WorkerError::Persist(aos_node::PersistError::validation(
                        "seed baseline requires manifest_hash",
                    ))
                })?;
                let loaded =
                    ManifestLoader::load_from_hash(store.as_ref(), parse_hash_ref(manifest_hash)?)?;
                let mut kernel = Kernel::from_loaded_manifest_without_replay_with_config(
                    Arc::clone(&store),
                    loaded.clone(),
                    Journal::new(),
                    kernel_config,
                )?;
                kernel.restore_snapshot_record(&kernel_snapshot_record(&seed.baseline))?;
                kernel.create_snapshot()?;
                let baseline =
                    latest_snapshot_from_kernel(&kernel)?.unwrap_or_else(|| seed.baseline.clone());
                (loaded, manifest_hash.clone(), kernel, baseline)
            }
        };

        let effect_runtime =
            self.build_registered_effect_runtime(universe_id, Arc::clone(&store), &loaded)?;
        let async_state = self.build_async_world_state();
        let workflow_modules = loaded
            .modules
            .values()
            .filter(|module| matches!(module.module_kind, aos_air_types::ModuleKind::Workflow))
            .map(|module| module.name.to_string())
            .collect::<Vec<_>>();

        let registered = RegisteredWorld {
            universe_id,
            store: Arc::clone(&store),
            loaded,
            manifest_hash: manifest_hash.clone(),
            effect_runtime,
            world_epoch: 1,
            projection_token: Uuid::new_v4().to_string(),
            projection_continuity: None,
            disabled_reason: None,
            metadata: HostedWorldMetadata {
                workflow_modules,
                warnings: Vec::new(),
            },
        };

        let active = ActiveWorld {
            world_id,
            universe_id,
            created_at_ns: request.created_at_ns,
            world_epoch: 1,
            active_baseline: baseline,
            next_world_seq: 0,
            kernel,
            accepted_submission_ids: Default::default(),
            mailbox: VecDeque::new(),
            ready: false,
            running: false,
            commit_blocked: false,
            pending_slice: None,
            pending_slices: VecDeque::new(),
            disabled_reason: None,
            last_checkpointed_head: 0,
            last_checkpointed_at_ns: 0,
            pending_create_checkpoint: true,
            projection_bootstrapped: false,
        };

        Ok((registered, active, async_state))
    }

    fn shared_effect_runtime_for_universe(
        &mut self,
        universe_id: UniverseId,
        store: Arc<HostedCas>,
    ) -> SharedEffectRuntime<WorldId> {
        if let Some(runtime) = self.shared_effect_runtimes.get(&universe_id) {
            return runtime.clone();
        }
        let runtime = SharedEffectRuntime::new(
            store,
            &EffectAdapterConfig::default(),
            self.effect_event_tx.clone(),
        );
        self.shared_effect_runtimes
            .insert(universe_id, runtime.clone());
        runtime
    }

    fn build_registered_effect_runtime(
        &mut self,
        universe_id: UniverseId,
        store: Arc<HostedCas>,
        loaded: &aos_kernel::LoadedManifest,
    ) -> Result<EffectRuntime<WorldId>, WorkerError> {
        let world_config = self.infra.world_config_for_domain(universe_id)?;
        let shared = self.shared_effect_runtime_for_universe(universe_id, store);
        EffectRuntime::from_loaded_manifest_with_shared(
            shared,
            loaded,
            world_config.strict_effect_bindings,
        )
        .map_err(WorkerError::Runtime)
    }

    fn build_async_world_state(&self) -> AsyncWorldState {
        AsyncWorldState {
            timer_scheduler: TimerScheduler::new(),
            scheduled_timers: Default::default(),
            timer_tasks: Default::default(),
        }
    }

    pub(super) fn rehydrate_runtime_work(&mut self, world_id: WorldId) -> Result<(), WorkerError> {
        let pending = match self.state.active_worlds.get(&world_id) {
            Some(world) => world.kernel.pending_workflow_receipts_snapshot(),
            None => return Ok(()),
        };

        let scheduler_tx = self.scheduler_tx.clone();
        let mut external_intents = Vec::new();
        let mut inline_intents = Vec::new();

        {
            let Some(async_state) = self.state.async_worlds.get_mut(&world_id) else {
                return Ok(());
            };

            async_state.abort_all_timers();
            async_state.timer_scheduler = TimerScheduler::new();
            async_state.scheduled_timers.clear();

            for pending in &pending {
                let intent = effect_intent_from_pending(pending)?;
                match classify_effect_kind(intent.kind.as_str()) {
                    EffectExecutionClass::ExternalAsync => {
                        external_intents.push(intent);
                    }
                    EffectExecutionClass::OwnerLocalTimer => {
                        Self::ensure_timer_started(
                            scheduler_tx.clone(),
                            world_id,
                            async_state,
                            intent,
                        )?;
                    }
                    EffectExecutionClass::InlineInternal => {
                        inline_intents.push(intent);
                    }
                }
            }
        }

        let mut inline_receipts = Vec::new();
        if !inline_intents.is_empty() {
            let Some(world) = self.state.active_worlds.get_mut(&world_id) else {
                return Ok(());
            };
            for intent in inline_intents {
                if let Some(receipt) = world.kernel.handle_internal_intent(&intent)? {
                    inline_receipts.push(receipt);
                }
            }
        }
        for receipt in inline_receipts.into_iter().rev() {
            self.enqueue_local_input_front(world_id, aos_kernel::WorldInput::Receipt(receipt))?;
        }

        let Some(registered) = self.state.registered_worlds.get(&world_id) else {
            return Ok(());
        };
        for intent in external_intents {
            let _ = registered.effect_runtime.ensure_started(world_id, intent)?;
        }
        Ok(())
    }

    pub(super) fn ensure_registered_world(
        &mut self,
        universe_id_hint: UniverseId,
        world_id: WorldId,
    ) -> Result<(), WorkerError> {
        if self.state.registered_worlds.contains_key(&world_id) {
            return Ok(());
        }

        let partition = partition_for_world(world_id, self.infra.kafka.partition_count());
        self.infra.kafka.recover_partition_from_broker(partition)?;
        let universe_id = self
            .universe_id_from_journal(world_id)
            .or_else(|| {
                self.checkpoint_world_entry(universe_id_hint, world_id)
                    .map(|(u, _, _)| u)
            })
            .unwrap_or(universe_id_hint);
        let manifest_hash = self
            .manifest_hash_from_journal(world_id)
            .or_else(|| {
                self.checkpoint_world_entry(universe_id, world_id)
                    .map(|(_, _, baseline)| baseline.manifest_hash)
            })
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        let world_epoch = self
            .world_epoch_from_journal(world_id)
            .or_else(|| {
                self.checkpoint_world_entry(universe_id, world_id)
                    .map(|(_, epoch, _)| epoch)
            })
            .unwrap_or(1);
        self.register_world_from_manifest_hash(universe_id, world_id, &manifest_hash, world_epoch)
    }

    fn universe_id_from_journal(&self, world_id: WorldId) -> Option<UniverseId> {
        self.infra
            .kafka
            .world_frames(world_id)
            .iter()
            .flat_map(|frame| frame.records.iter())
            .find_map(|record| match record {
                JournalRecord::Snapshot(snapshot) => Some(snapshot.universe_id.into()),
                _ => None,
            })
    }

    fn manifest_hash_from_journal(&self, world_id: WorldId) -> Option<String> {
        self.infra
            .kafka
            .world_frames(world_id)
            .iter()
            .flat_map(|frame| frame.records.iter())
            .filter_map(|record| match record {
                JournalRecord::Snapshot(snapshot) => snapshot.manifest_hash.clone(),
                _ => None,
            })
            .next_back()
    }

    fn world_epoch_from_journal(&self, world_id: WorldId) -> Option<u64> {
        self.infra
            .kafka
            .world_frames(world_id)
            .last()
            .map(|frame| frame.world_epoch)
    }

    fn register_world_from_manifest_hash(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
        manifest_hash: &str,
        world_epoch: u64,
    ) -> Result<(), WorkerError> {
        let store = self.infra.store_for_domain(universe_id)?;
        let loaded =
            ManifestLoader::load_from_hash(store.as_ref(), parse_hash_ref(manifest_hash)?)?;
        let effect_runtime =
            self.build_registered_effect_runtime(universe_id, Arc::clone(&store), &loaded)?;
        let workflow_modules = loaded
            .modules
            .values()
            .filter(|module| matches!(module.module_kind, aos_air_types::ModuleKind::Workflow))
            .map(|module| module.name.to_string())
            .collect::<Vec<_>>();
        self.state.registered_worlds.insert(
            world_id,
            RegisteredWorld {
                universe_id,
                store,
                loaded,
                manifest_hash: manifest_hash.to_owned(),
                effect_runtime,
                world_epoch,
                projection_token: Uuid::new_v4().to_string(),
                projection_continuity: None,
                disabled_reason: None,
                metadata: HostedWorldMetadata {
                    workflow_modules,
                    warnings: Vec::new(),
                },
            },
        );
        Ok(())
    }

    pub(super) fn activate_world(&mut self, world_id: WorldId) -> Result<(), WorkerError> {
        if self.state.active_worlds.contains_key(&world_id)
            && self.state.async_worlds.contains_key(&world_id)
        {
            return Ok(());
        }
        if self.state.active_worlds.contains_key(&world_id) {
            self.reopen_active_world(world_id, &[])?;
            return Ok(());
        }
        let open_started = Instant::now();
        let (active, async_state, metrics) =
            self.build_active_world(world_id, VecDeque::new(), BTreeSet::new())?;
        self.log_world_opened(
            active.universe_id,
            world_id,
            active.world_epoch,
            "activate",
            open_started.elapsed().as_millis(),
            active.active_baseline.height,
            active.next_world_seq,
            metrics.frame_count,
            metrics.replay_frame_count,
            active.pending_create_checkpoint,
        );
        let needs_projection_bootstrap = !active.projection_bootstrapped;
        self.state.active_worlds.insert(world_id, active);
        self.state.async_worlds.insert(world_id, async_state);
        self.rehydrate_runtime_work(world_id)?;
        if needs_projection_bootstrap {
            self.schedule_projection_updates_for_worlds(&[world_id])?;
        }
        Ok(())
    }

    fn build_active_world(
        &mut self,
        world_id: WorldId,
        mailbox: VecDeque<WorkItem>,
        accepted_submission_ids: BTreeSet<String>,
    ) -> Result<(ActiveWorld, AsyncWorldState, ActiveWorldOpenMetrics), WorkerError> {
        let registered =
            self.state
                .registered_worlds
                .get(&world_id)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id: self.infra.default_universe_id,
                    world_id,
                })?;
        let universe_id = registered.universe_id;
        let store = Arc::clone(&registered.store);
        let loaded = registered.loaded.clone();
        let world_epoch = registered.world_epoch;
        let disabled_reason = registered.disabled_reason.clone();
        let frames = self.infra.kafka.world_frames(world_id).to_vec();
        let frame_count = frames.len();
        let (active_baseline, tail_frames, has_checkpoint) =
            self.baseline_and_tail_frames(universe_id, world_id, &frames)?;
        let replay_frame_count = tail_frames.len();
        let next_world_seq = frames
            .last()
            .map(|frame| frame.world_seq_end.saturating_add(1))
            .unwrap_or_default()
            .max(active_baseline.height.saturating_add(1));
        let async_state = self.build_async_world_state();
        let kernel = reopen_kernel_from_frame_log(
            store,
            loaded,
            &active_baseline,
            &tail_frames,
            self.kernel_config_for_world(universe_id)?,
        )?;
        let projection_bootstrapped = self.prepare_projection_continuity_for_reopen(
            world_id,
            kernel.journal_head(),
            &active_baseline,
        )?;

        Ok((
            ActiveWorld {
                world_id,
                universe_id,
                created_at_ns: 0,
                world_epoch,
                active_baseline: active_baseline.clone(),
                next_world_seq,
                kernel,
                accepted_submission_ids,
                mailbox,
                ready: false,
                running: false,
                commit_blocked: false,
                pending_slice: None,
                pending_slices: VecDeque::new(),
                disabled_reason,
                last_checkpointed_head: active_baseline.height,
                last_checkpointed_at_ns: has_checkpoint.then(|| unix_time_ns()).unwrap_or_default(),
                pending_create_checkpoint: !has_checkpoint,
                projection_bootstrapped,
            },
            async_state,
            ActiveWorldOpenMetrics {
                frame_count,
                replay_frame_count,
            },
        ))
    }

    fn baseline_and_tail_frames(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
        frames: &[WorldLogFrame],
    ) -> Result<(SnapshotRecord, Vec<WorldLogFrame>, bool), WorkerError> {
        let checkpoint_snapshot = self
            .checkpoint_world_entry(universe_id, world_id)
            .map(|(_, _, baseline)| snapshot_record_from_checkpoint(&baseline));
        let journal_snapshot = snapshot_record_from_frames(frames, |_| true);
        let snapshot = match (checkpoint_snapshot.as_ref(), journal_snapshot.as_ref()) {
            (Some(checkpoint), Some(journal)) if journal.height >= checkpoint.height => {
                journal.clone()
            }
            (Some(checkpoint), _) => checkpoint.clone(),
            (None, Some(journal)) => journal.clone(),
            (None, None) => Err(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?,
        };
        let tail = frames
            .iter()
            .filter(|frame| frame.world_seq_end > snapshot.height)
            .cloned()
            .collect::<Vec<_>>();
        let has_checkpoint = checkpoint_snapshot
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.height >= snapshot.height);
        Ok((snapshot, tail, has_checkpoint))
    }

    fn checkpoint_world_entry(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Option<(UniverseId, u64, aos_node::PromotableBaselineRef)> {
        let journal_topic = self.infra.kafka.config().journal_topic.clone();
        let partition = partition_for_world(world_id, self.infra.kafka.partition_count());
        let checkpoint = self
            .infra
            .blob_meta_for_domain_mut(universe_id)
            .ok()?
            .latest_checkpoint(&journal_topic, partition)?
            .clone();
        checkpoint.worlds.into_iter().find_map(|entry| {
            (entry.universe_id == universe_id && entry.world_id == world_id).then_some((
                entry.universe_id,
                entry.world_epoch,
                entry.baseline,
            ))
        })
    }

    pub(super) fn disable_world(&mut self, world_id: WorldId, reason: impl Into<String>) {
        let reason = reason.into();
        self.state.active_worlds.remove(&world_id);
        if let Some(mut async_state) = self.state.async_worlds.remove(&world_id) {
            async_state.abort_all_timers();
        }
        if let Some(world) = self.state.registered_worlds.get_mut(&world_id) {
            world.disabled_reason = Some(reason.clone());
            let warning = format!("disabled: {reason}");
            if !world.metadata.warnings.iter().any(|item| item == &warning) {
                world.metadata.warnings.push(warning);
            }
        }
    }

    pub(super) fn world_summary(
        &mut self,
        world_id: WorldId,
    ) -> Result<HostedWorldSummary, WorkerError> {
        let registered =
            self.state
                .registered_worlds
                .get(&world_id)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id: self.infra.default_universe_id,
                    world_id,
                })?;
        let next_world_seq = self
            .state
            .active_worlds
            .get(&world_id)
            .map(|world| world.next_world_seq)
            .or_else(|| {
                self.infra
                    .kafka
                    .world_frames(world_id)
                    .last()
                    .map(|frame| frame.world_seq_end.saturating_add(1))
            })
            .unwrap_or_default();
        Ok(HostedWorldSummary {
            universe_id: registered.universe_id,
            world_id,
            world_root: self
                .infra
                .domain_paths(registered.universe_id)
                .root()
                .display()
                .to_string(),
            manifest_hash: registered.manifest_hash.clone(),
            world_epoch: registered.world_epoch,
            effective_partition: partition_for_world(world_id, self.infra.kafka.partition_count()),
            next_world_seq,
            workflow_modules: registered.metadata.workflow_modules.clone(),
            warnings: registered.metadata.warnings.clone(),
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct ActiveWorldOpenMetrics {
    frame_count: usize,
    replay_frame_count: usize,
}

fn create_world_source_kind(request: &CreateWorldRequest) -> &'static str {
    match request.source {
        CreateWorldSource::Manifest { .. } => "manifest",
        CreateWorldSource::Seed { .. } => "seed",
    }
}

fn latest_snapshot_from_kernel<S: Store + 'static>(
    kernel: &Kernel<S>,
) -> Result<Option<SnapshotRecord>, WorkerError> {
    Ok(
        latest_snapshot_record(&kernel.dump_journal()?).map(|snapshot| SnapshotRecord {
            snapshot_ref: snapshot.snapshot_ref,
            height: snapshot.height,
            universe_id: snapshot.universe_id.into(),
            logical_time_ns: snapshot.logical_time_ns,
            receipt_horizon_height: snapshot.receipt_horizon_height,
            manifest_hash: snapshot.manifest_hash,
        }),
    )
}

fn initial_frame_for_kernel<S: Store + 'static>(
    universe_id: UniverseId,
    world_id: WorldId,
    world_epoch: u64,
    kernel: &Kernel<S>,
    world_seq_start: u64,
) -> Result<Option<WorldLogFrame>, WorkerError> {
    let tail = kernel.dump_journal()?;
    if tail.is_empty() {
        return Ok(None);
    }
    let records = tail
        .into_iter()
        .map(|entry| serde_cbor::from_slice::<JournalRecord>(&entry.payload))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(WorldLogFrame {
        format_version: 1,
        universe_id,
        world_id,
        world_epoch,
        world_seq_start,
        world_seq_end: world_seq_start + records.len() as u64 - 1,
        records,
    }))
}
