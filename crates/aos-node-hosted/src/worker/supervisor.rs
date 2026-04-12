use std::collections::{BTreeMap, BTreeSet};
use std::thread;
use std::time::{Duration, Instant};

use aos_kernel::journal::JournalRecord;
use aos_node::partition_for_world;
use aos_runtime::{RunMode, now_wallclock_ns};
use tokio::runtime::Builder as RuntimeBuilder;
use tracing::{debug, warn};

use crate::config::HostedWorkerConfig;

use super::runtime::HostedWorkerRuntime;
use super::timers::PartitionTimerState;
use super::types::{
    HostedWorkerRuntimeInner, PartitionRunOutcome, SupervisorOutcome, SupervisorRunProfile,
    WorkerError,
};
use super::util::unix_time_ns;

#[derive(Clone)]
pub struct HostedWorker {
    pub config: HostedWorkerConfig,
}

impl HostedWorker {
    pub fn new(config: HostedWorkerConfig) -> Self {
        Self { config }
    }

    pub fn with_worker_runtime(&self, runtime: HostedWorkerRuntime) -> WorkerSupervisor {
        WorkerSupervisor {
            runtime,
            workers: BTreeMap::new(),
            poll_interval: self.config.supervisor_poll_interval,
            checkpoint_interval: self.config.checkpoint_interval,
            checkpoint_every_events: self
                .config
                .checkpoint_every_events
                .map(|count| count as usize),
            checkpoint_on_create: self.config.checkpoint_on_create,
            last_checkpoint: Instant::now(),
        }
    }
}

struct PartitionWorker {
    partition: u32,
    events_since_checkpoint: usize,
    timers: PartitionTimerState,
    disabled_worlds: BTreeMap<aos_node::WorldId, String>,
}

impl PartitionWorker {
    fn new(partition: u32) -> Self {
        Self {
            partition,
            events_since_checkpoint: 0,
            timers: PartitionTimerState::default(),
            disabled_worlds: BTreeMap::new(),
        }
    }

    fn run_once(
        &mut self,
        runtime: &mut HostedWorkerRuntimeInner,
        profile: &mut SupervisorRunProfile,
        checkpoint_on_create: bool,
    ) -> Result<PartitionRunOutcome, WorkerError> {
        let (mut outcome, run_profile) = runtime
            .run_partition_once_profiled(self.partition, checkpoint_on_create)
            .inspect_err(|err| {
                tracing::error!(
                    partition = self.partition,
                    error = %err,
                    "hosted worker failed during partition submission replay"
                );
            })?;
        profile.partition_drain_submissions += run_profile.drain_submissions;
        profile.partition_process_create += run_profile.process_create;
        profile.partition_process_existing += run_profile.process_existing;
        profile.partition_activate_world += run_profile.activate_world;
        profile.partition_apply_submission += run_profile.apply_submission;
        profile.partition_build_external_event += run_profile.build_external_event;
        profile.partition_host_drain += run_profile.host_drain;
        profile.partition_post_apply += run_profile.post_apply;
        profile.partition_commit_batch += run_profile.commit_batch;
        profile.partition_commit_command_records += run_profile.commit_command_records;
        profile.partition_promote_worlds += run_profile.promote_worlds;
        profile.partition_inline_checkpoint += run_profile.inline_checkpoint;
        if outcome.inline_checkpoint_published {
            self.events_since_checkpoint = 0;
        } else {
            self.events_since_checkpoint = self
                .events_since_checkpoint
                .saturating_add(outcome.checkpoint_event_frames);
        }
        let active_worlds = self.activate_partition_worlds(runtime).inspect_err(|err| {
            tracing::error!(
                partition = self.partition,
                error = %err,
                "hosted worker failed while activating partition worlds"
            );
        })?;
        self.timers.retain_worlds(
            &active_worlds
                .iter()
                .map(|(_, world_id)| *world_id)
                .collect::<Vec<_>>(),
        );
        outcome.frames_appended = outcome
            .frames_appended
            .checked_add(self.process_worlds_once(runtime, &active_worlds)?)
            .expect("hosted partition frame count overflow");
        Ok(outcome)
    }

    fn publish_checkpoint(
        &mut self,
        runtime: &mut HostedWorkerRuntimeInner,
        created_at_ns: u64,
        trigger: &'static str,
    ) -> Result<aos_node::PartitionCheckpoint, WorkerError> {
        let checkpoint =
            runtime.create_partition_checkpoint(self.partition, created_at_ns, trigger)?;
        self.events_since_checkpoint = 0;
        Ok(checkpoint)
    }

    fn checkpoint_due_by_events(&self, threshold: Option<usize>) -> bool {
        threshold.is_some_and(|threshold| self.events_since_checkpoint >= threshold)
    }

    fn activate_partition_worlds(
        &mut self,
        runtime: &mut HostedWorkerRuntimeInner,
    ) -> Result<Vec<(aos_node::UniverseId, aos_node::WorldId)>, WorkerError> {
        let mut worlds = Vec::new();
        let default_universe_id = runtime.infra.default_universe_id;
        let world_ids = runtime.infra.kafka.world_ids();
        for world_id in world_ids {
            let effective_partition =
                partition_for_world(world_id, runtime.infra.kafka.partition_count());
            if effective_partition != self.partition {
                continue;
            }
            let universe_id = runtime
                .state
                .registered_worlds
                .get(&world_id)
                .map(|world| world.universe_id)
                .unwrap_or(default_universe_id);
            if self.disabled_worlds.contains_key(&world_id) {
                continue;
            }
            if let Some(reason) = runtime.world_disabled_reason(world_id) {
                tracing::warn!(
                    universe_id = %universe_id,
                    world_id = %world_id,
                    reason,
                    "skipping disabled hosted world"
                );
                self.disabled_worlds.insert(world_id, reason.to_owned());
                continue;
            }
            match (|| -> Result<(), WorkerError> {
                runtime.ensure_registered_world(default_universe_id, world_id)?;
                let universe_id = runtime
                    .state
                    .registered_worlds
                    .get(&world_id)
                    .map(|world| world.universe_id)
                    .unwrap_or(default_universe_id);
                runtime.activate_world(universe_id, world_id)
            })() {
                Ok(()) => worlds.push((universe_id, world_id)),
                Err(WorkerError::Host(err)) => {
                    let reason = err.to_string();
                    tracing::error!(
                        universe_id = %universe_id,
                        world_id = %world_id,
                        error = %reason,
                        "disabling hosted world after activation host error"
                    );
                    self.disabled_worlds.insert(world_id, reason.clone());
                    runtime.disable_world(world_id, reason);
                    self.timers.reset_world(world_id);
                }
                Err(WorkerError::Kernel(err)) => {
                    let reason = err.to_string();
                    tracing::error!(
                        universe_id = %universe_id,
                        world_id = %world_id,
                        error = %reason,
                        "disabling hosted world after activation kernel error"
                    );
                    self.disabled_worlds.insert(world_id, reason.clone());
                    runtime.disable_world(world_id, reason);
                    self.timers.reset_world(world_id);
                }
                Err(err) => {
                    let reason = err.to_string();
                    tracing::error!(
                        universe_id = %universe_id,
                        world_id = %world_id,
                        error = %reason,
                        "disabling hosted world after activation error"
                    );
                    self.disabled_worlds.insert(world_id, reason.clone());
                    runtime.disable_world(world_id, reason);
                    self.timers.reset_world(world_id);
                }
            }
        }
        Ok(worlds)
    }

    fn process_worlds_once(
        &mut self,
        runtime: &mut HostedWorkerRuntimeInner,
        worlds: &[(aos_node::UniverseId, aos_node::WorldId)],
    ) -> Result<usize, WorkerError> {
        let mut frames_appended = 0usize;
        for (universe_id, world_id) in worlds {
            match self.process_world_until_stable(runtime, *universe_id, *world_id) {
                Ok(count) => {
                    frames_appended = frames_appended.saturating_add(count);
                }
                Err(WorkerError::Host(err)) => {
                    let reason = err.to_string();
                    tracing::error!(
                        universe_id = %universe_id,
                        world_id = %world_id,
                        error = %reason,
                        "disabling hosted world after daemon cycle host error"
                    );
                    runtime.disable_world(*world_id, reason);
                    self.timers.reset_world(*world_id);
                }
                Err(WorkerError::Kernel(err)) => {
                    let reason = err.to_string();
                    tracing::error!(
                        universe_id = %universe_id,
                        world_id = %world_id,
                        error = %reason,
                        "disabling hosted world after daemon cycle kernel error"
                    );
                    runtime.disable_world(*world_id, reason);
                    self.timers.reset_world(*world_id);
                }
                Err(err) => return Err(err),
            }
        }
        Ok(frames_appended)
    }

    fn process_world_until_stable(
        &mut self,
        runtime: &mut HostedWorkerRuntimeInner,
        universe_id: aos_node::UniverseId,
        world_id: aos_node::WorldId,
    ) -> Result<usize, WorkerError> {
        let timer_state = self.timers.world_mut(world_id);
        let mut frames_appended = 0usize;
        for _ in 0..32 {
            let journal_tail_start = {
                let world = runtime.state.active_worlds.get_mut(&world_id).ok_or(
                    WorkerError::UnknownWorld {
                        universe_id,
                        world_id,
                    },
                )?;
                if !timer_state.rehydrated {
                    timer_state
                        .scheduler
                        .rehydrate_daemon_state(world.host.kernel_mut());
                    timer_state.rehydrated = true;
                }
                let now_ns = now_wallclock_ns();
                if world.host.logical_time_now_ns() < now_ns {
                    let _ = world.host.set_logical_time_ns(now_ns);
                }
                world.host.journal_bounds().next_seq
            };

            let cycle = thread::scope(|scope| -> Result<_, WorkerError> {
                let world = runtime.state.active_worlds.get_mut(&world_id).ok_or(
                    WorkerError::UnknownWorld {
                        universe_id,
                        world_id,
                    },
                )?;
                let handle = scope.spawn(|| -> Result<_, WorkerError> {
                    let exec = RuntimeBuilder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("build hosted world cycle runtime");
                    exec.block_on(world.host.run_cycle(RunMode::Daemon {
                        scheduler: &mut timer_state.scheduler,
                    }))
                    .map_err(WorkerError::from)
                });
                handle.join().map_err(|panic| {
                    let message = if let Some(message) = panic.downcast_ref::<&str>() {
                        (*message).to_owned()
                    } else if let Some(message) = panic.downcast_ref::<String>() {
                        message.clone()
                    } else {
                        "hosted world cycle thread panicked".to_owned()
                    };
                    WorkerError::Build(anyhow::anyhow!(message))
                })?
            })?;

            let fired = {
                let world = runtime.state.active_worlds.get_mut(&world_id).ok_or(
                    WorkerError::UnknownWorld {
                        universe_id,
                        world_id,
                    },
                )?;
                let fired = world.host.fire_due_timers(&mut timer_state.scheduler)?;
                if fired > 0 {
                    let _ = world.host.drain()?;
                }
                fired
            };

            let tail = {
                let world = runtime.state.active_worlds.get(&world_id).ok_or(
                    WorkerError::UnknownWorld {
                        universe_id,
                        world_id,
                    },
                )?;
                world.host.kernel().dump_journal_from(journal_tail_start)?
            };

            let progressed = cycle.effects_dispatched > 0
                || cycle.receipts_applied > 0
                || cycle.initial_drain.ticks > 0
                || cycle.final_drain.ticks > 0
                || fired > 0;
            if tail.is_empty() {
                if progressed {
                    continue;
                }
                return Ok(frames_appended);
            }

            let mut records = Vec::with_capacity(tail.len());
            for entry in tail {
                let record: JournalRecord = serde_cbor::from_slice(&entry.payload)?;
                records.push(record);
            }
            let world_epoch = runtime
                .state
                .registered_worlds
                .get(&world_id)
                .map(|world| world.world_epoch)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id,
                    world_id,
                })?;
            let expected_world_seq = runtime.infra.kafka.next_world_seq(world_id);
            if expected_world_seq > journal_tail_start {
                warn!(
                    universe_id = %universe_id,
                    world_id = %world_id,
                    expected_world_seq,
                    journal_tail_start,
                    "hosted worker daemon world sequence diverged from host journal tail; using host tail"
                );
            } else if expected_world_seq < journal_tail_start {
                debug!(
                    universe_id = %universe_id,
                    world_id = %world_id,
                    expected_world_seq,
                    journal_tail_start,
                    "hosted worker daemon world sequence advanced ahead of persisted tail; using host tail"
                );
            }
            let world_seq_start = journal_tail_start;
            let world_seq_end = world_seq_start + records.len() as u64 - 1;
            let frame = aos_node::WorldLogFrame {
                format_version: 1,
                universe_id,
                world_id,
                world_epoch,
                world_seq_start,
                world_seq_end,
                records,
            };
            if let Err(err) = runtime.infra.kafka.append_frame(frame) {
                let accepted_submission_ids = runtime
                    .state
                    .active_worlds
                    .get(&world_id)
                    .map(|world| world.accepted_submission_ids.clone())
                    .ok_or(WorkerError::UnknownWorld {
                        universe_id,
                        world_id,
                    })?;
                runtime.rollback_active_worlds(BTreeMap::from([(
                    world_id,
                    accepted_submission_ids,
                )]))?;
                self.timers.reset_world(world_id);
                return Err(WorkerError::LogFirst(err));
            }
            runtime.emit_projection_updates_for_worlds(&[world_id])?;
            frames_appended = frames_appended.saturating_add(1);
        }

        warn!("hosted worker hit max cycle budget while processing world {world_id}");
        Ok(frames_appended)
    }
}

pub struct WorkerSupervisor {
    runtime: HostedWorkerRuntime,
    workers: BTreeMap<u32, PartitionWorker>,
    poll_interval: Duration,
    checkpoint_interval: Duration,
    checkpoint_every_events: Option<usize>,
    checkpoint_on_create: bool,
    last_checkpoint: Instant,
}

impl WorkerSupervisor {
    pub async fn run_once(&mut self) -> Result<SupervisorOutcome, WorkerError> {
        Ok(self.run_once_profiled().await?.0)
    }

    pub async fn run_once_profiled(
        &mut self,
    ) -> Result<(SupervisorOutcome, SupervisorRunProfile), WorkerError> {
        let total_started = Instant::now();
        let mut frames_appended = 0usize;
        let mut checkpoints_published = 0usize;
        let mut profile = SupervisorRunProfile::default();
        {
            let mut inner = self.runtime.lock_inner()?;
            let sync_assignments_started = Instant::now();
            let (newly_assigned, _): (Vec<u32>, Vec<u32>) =
                inner.infra.kafka.sync_assignments_and_poll()?;
            profile.sync_assignments = sync_assignments_started.elapsed();
            let assigned = inner.infra.kafka.assigned_partitions();
            profile.assigned_partitions = assigned.len();
            profile.newly_assigned_partitions = newly_assigned.len();
            let sync_worlds_started = Instant::now();
            inner.sync_active_worlds(&assigned, &newly_assigned)?;
            profile.sync_active_worlds = sync_worlds_started.elapsed();
            let assigned_set = assigned.iter().copied().collect::<BTreeSet<_>>();
            self.workers
                .retain(|partition, _| assigned_set.contains(partition));
            for partition in assigned {
                self.workers
                    .entry(partition)
                    .or_insert_with(|| PartitionWorker::new(partition));
            }
            let run_partitions_started = Instant::now();
            for worker in self.workers.values_mut() {
                let outcome =
                    worker.run_once(&mut inner, &mut profile, self.checkpoint_on_create)?;
                frames_appended += outcome.frames_appended;
            }
            profile.run_partitions = run_partitions_started.elapsed();
            let checkpoint_due_by_time = self.last_checkpoint.elapsed() >= self.checkpoint_interval;
            let checkpoint_due_by_events = self
                .workers
                .iter()
                .filter_map(|(partition, worker)| {
                    worker
                        .checkpoint_due_by_events(self.checkpoint_every_events)
                        .then_some(*partition)
                })
                .collect::<BTreeSet<_>>();
            let publish_checkpoints_started = Instant::now();
            if checkpoint_due_by_time || !checkpoint_due_by_events.is_empty() {
                for worker in self.workers.values_mut() {
                    if checkpoint_due_by_time
                        || checkpoint_due_by_events.contains(&worker.partition)
                    {
                        let trigger = if checkpoint_due_by_time {
                            "interval"
                        } else {
                            "events"
                        };
                        worker.publish_checkpoint(&mut inner, unix_time_ns(), trigger)?;
                        checkpoints_published += 1;
                    }
                }
                self.last_checkpoint = Instant::now();
            }
            profile.publish_checkpoints = publish_checkpoints_started.elapsed();
            profile.total = total_started.elapsed();
            Ok((
                SupervisorOutcome {
                    frames_appended,
                    checkpoints_published,
                    registered_worlds: inner.state.registered_worlds.len(),
                    pending_submissions: inner.infra.kafka.pending_submission_count(),
                },
                profile,
            ))
        }
    }

    pub async fn serve_forever(&mut self) -> Result<(), WorkerError> {
        loop {
            self.run_once().await?;
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}
