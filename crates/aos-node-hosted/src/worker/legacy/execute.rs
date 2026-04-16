//! Legacy pre-cutover partition-oriented worker execution helpers.
//!
//! This file is intentionally not on the compiled hosted worker path.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use crate::blobstore::HostedCas;
use aos_air_types::ModuleKind;
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_kernel::journal::JournalRecord;
use aos_node::{
    CommandIngress, CommandRecord, CreateWorldRequest, RejectedSubmission, SubmissionEnvelope,
    SubmissionPayload, SubmissionRejection, UniverseId, WorldId, WorldLogFrame,
};
use uuid::Uuid;

use super::commands::{
    command_failed_record, command_succeeded_record, run_plane_command,
    synthesize_queued_command_record,
};
use super::types::{
    ActiveWorld, CommandRollbackRecords, CommitCommandRecords, HostedWorkerCore,
    HostedWorldMetadata, PartitionRunOutcome, PartitionRunProfile, PendingCreatedWorld,
    PendingCreatedWorlds, RegisteredWorld, SubmissionRollbackIds, WorkerError,
};
use super::util::{resolve_cbor_payload, submission_to_external_event, unix_time_ns};

impl HostedWorkerCore {
    pub(super) fn seed_world_direct(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
        request: CreateWorldRequest,
        publish_checkpoint: bool,
    ) -> Result<(), WorkerError> {
        let _ = universe_id;
        if self.state.registered_worlds.contains_key(&world_id)
            || self.state.active_worlds.contains_key(&world_id)
        {
            return Err(WorkerError::Persist(aos_node::PersistError::validation(
                format!("world {world_id} already exists"),
            )));
        }

        self.prepare_create_request_materialization(request.universe_id, &request)?;
        let store: Arc<HostedCas> = self.infra.store_for_domain(request.universe_id)?;
        let open_started = Instant::now();
        let created = aos_node::create_plane_world_from_request(
            store,
            &request,
            request.universe_id,
            world_id,
            1,
            self.infra.world_config_for_domain(request.universe_id)?,
            EffectAdapterConfig::default(),
            self.kernel_config_for_world(request.universe_id)?,
        )
        .map_err(WorkerError::LogFirst)?;
        let total_open_ms = open_started.elapsed().as_millis();
        if let Some(frame) = created.initial_frame {
            self.infra.kafka.append_frame(frame)?;
        }
        self.register_world_from_manifest_hash(
            request.universe_id,
            world_id,
            &created.initial_manifest_hash,
            1,
        )?;
        self.state.active_worlds.insert(
            world_id,
            ActiveWorld {
                last_checkpointed_head: 0,
                last_checkpointed_at_ns: 0,
                host: created.host,
                accepted_submission_ids: BTreeSet::new(),
                projection_bootstrapped: false,
            },
        );
        self.log_world_opened(request.universe_id, world_id, "seed", total_open_ms);
        if publish_checkpoint {
            let partition =
                aos_node::partition_for_world(world_id, self.infra.kafka.partition_count());
            self.create_partition_checkpoint(partition, unix_time_ns(), "seed")?;
        }
        Ok(())
    }

    pub(super) fn run_partition_once_profiled(
        &mut self,
        partition: u32,
        checkpoint_on_create: bool,
    ) -> Result<(PartitionRunOutcome, PartitionRunProfile), WorkerError> {
        let mut profile = PartitionRunProfile::default();
        let drain_started = Instant::now();
        let batch = self.infra.kafka.drain_partition_submissions(partition)?;
        profile.drain_submissions = drain_started.elapsed();
        let mut frames = Vec::new();
        let mut checkpoint_event_frames = 0usize;
        let create_seen = batch
            .submissions
            .iter()
            .any(|submission| matches!(submission.payload, SubmissionPayload::CreateWorld { .. }));

        let mut pending_created_worlds = BTreeMap::new();
        let mut speculative_next_world_seq = BTreeMap::new();
        let mut rollback_submission_ids = BTreeMap::new();
        let mut rollback_command_records = BTreeMap::new();
        let mut commit_command_records = Vec::new();
        for submission in batch.submissions.iter().cloned() {
            let frame = if let SubmissionPayload::CreateWorld { request } = &submission.payload {
                let started = Instant::now();
                self.process_create_world_submission(
                    &submission,
                    request.clone(),
                    &mut pending_created_worlds,
                )?
                .inspect(|_| {
                    profile.process_create += started.elapsed();
                })
            } else {
                let started = Instant::now();
                self.process_existing_submission(
                    &submission,
                    &mut speculative_next_world_seq,
                    &mut rollback_submission_ids,
                    &mut rollback_command_records,
                    &mut commit_command_records,
                    &mut profile,
                )
                .inspect_err(|err| {
                    tracing::error!(
                        submission_id = %submission.submission_id,
                        world_id = %submission.world_id,
                        error = %err,
                        "hosted worker failed while replaying existing submission"
                    );
                })?
                .inspect(|_| {
                    profile.process_existing += started.elapsed();
                })
            };
            if let Some(frame) = frame {
                speculative_next_world_seq
                    .insert(frame.world_id, frame.world_seq_end.saturating_add(1));
                if !matches!(submission.payload, SubmissionPayload::CreateWorld { .. }) {
                    checkpoint_event_frames += 1;
                }
                frames.push(frame);
            }
        }

        let commit_batch_started = Instant::now();
        let projected_world_ids = frames
            .iter()
            .map(|frame| frame.world_id)
            .collect::<Vec<_>>();
        if let Err(err) = self
            .infra
            .kafka
            .commit_submission_batch(batch, frames.clone())
        {
            self.rollback_active_worlds(rollback_submission_ids)?;
            self.rollback_command_records(rollback_command_records)?;
            return Err(WorkerError::LogFirst(err));
        }
        profile.commit_batch = commit_batch_started.elapsed();
        let command_records_started = Instant::now();
        for (world_id, record) in commit_command_records {
            let universe_id = self
                .state
                .registered_worlds
                .get(&world_id)
                .map(|world| world.universe_id)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id: self.infra.default_universe_id,
                    world_id,
                })?;
            self.infra
                .blob_meta_for_domain_mut(universe_id)?
                .put_command_record(world_id, record)?;
        }
        profile.commit_command_records = command_records_started.elapsed();
        let promote_started = Instant::now();
        self.finalize_created_worlds(pending_created_worlds)?;
        profile.promote_worlds = promote_started.elapsed();
        self.emit_projection_updates_for_worlds(&projected_world_ids)?;
        if create_seen && checkpoint_on_create {
            let checkpoint_started = Instant::now();
            self.create_partition_checkpoint(partition, unix_time_ns(), "create")?;
            profile.inline_checkpoint = checkpoint_started.elapsed();
        }
        Ok((
            PartitionRunOutcome {
                frames_appended: frames.len(),
                checkpoint_event_frames,
                inline_checkpoint_published: create_seen && checkpoint_on_create,
            },
            profile,
        ))
    }

    pub(super) fn process_existing_submission(
        &mut self,
        submission: &SubmissionEnvelope,
        speculative_next_world_seq: &mut BTreeMap<WorldId, u64>,
        rollback_submission_ids: &mut SubmissionRollbackIds,
        rollback_command_records: &mut CommandRollbackRecords,
        commit_command_records: &mut CommitCommandRecords,
        profile: &mut PartitionRunProfile,
    ) -> Result<Option<WorldLogFrame>, WorkerError> {
        let world_key = submission.world_id;
        if !self.state.registered_worlds.contains_key(&world_key) {
            let _ = self.ensure_registered_world(submission.universe_id, world_key);
        }
        let expected_world_epoch = match self.state.registered_worlds.get(&world_key) {
            Some(world) => world.world_epoch,
            None => {
                self.infra.kafka.record_rejected(RejectedSubmission {
                    submission: submission.clone(),
                    reason: SubmissionRejection::UnknownWorld,
                });
                return Ok(None);
            }
        };

        if submission.world_epoch != expected_world_epoch {
            let got = submission.world_epoch;
            self.infra.kafka.record_rejected(RejectedSubmission {
                submission: submission.clone(),
                reason: SubmissionRejection::WorldEpochMismatch {
                    expected: expected_world_epoch,
                    got,
                },
            });
            return Ok(None);
        }

        let rollback_record = match &submission.payload {
            SubmissionPayload::Command { command } => {
                Some(self.command_rollback_record(world_key, command)?)
            }
            _ => None,
        };
        if let Some(reason) = self.world_disabled_reason(world_key) {
            self.infra.kafka.record_rejected(RejectedSubmission {
                submission: submission.clone(),
                reason: SubmissionRejection::InvalidSubmission {
                    message: format!("world {world_key} is disabled: {reason}"),
                },
            });
            return Ok(None);
        }
        let activate_started = Instant::now();
        if !self.state.active_worlds.contains_key(&world_key) {
            self.ensure_registered_world(submission.universe_id, world_key)?;
            match self.activate_world(submission.universe_id, world_key) {
                Ok(()) => {}
                Err(WorkerError::Host(err)) => {
                    let reason = err.to_string();
                    tracing::error!(
                        universe_id = %submission.universe_id,
                        world_id = %world_key,
                        error = %reason,
                        "disabling hosted world after submission activation host error"
                    );
                    self.disable_world(world_key, reason.clone());
                    self.infra.kafka.record_rejected(RejectedSubmission {
                        submission: submission.clone(),
                        reason: SubmissionRejection::InvalidSubmission {
                            message: format!("world {world_key} is disabled: {reason}"),
                        },
                    });
                    return Ok(None);
                }
                Err(WorkerError::Kernel(err)) => {
                    let reason = err.to_string();
                    tracing::error!(
                        universe_id = %submission.universe_id,
                        world_id = %world_key,
                        error = %reason,
                        "disabling hosted world after submission activation kernel error"
                    );
                    self.disable_world(world_key, reason.clone());
                    self.infra.kafka.record_rejected(RejectedSubmission {
                        submission: submission.clone(),
                        reason: SubmissionRejection::InvalidSubmission {
                            message: format!("world {world_key} is disabled: {reason}"),
                        },
                    });
                    return Ok(None);
                }
                Err(err) => return Err(err),
            }
        }
        profile.activate_world += activate_started.elapsed();
        let expected_world_seq = *speculative_next_world_seq
            .entry(world_key)
            .or_insert_with(|| self.infra.kafka.next_world_seq(world_key));
        let (store, universe_id) =
            {
                let world = self.state.registered_worlds.get(&world_key).ok_or(
                    WorkerError::UnknownWorld {
                        universe_id: submission.universe_id,
                        world_id: world_key,
                    },
                )?;
                (world.store.clone(), world.universe_id)
            };
        let journal_tail_start =
            {
                let world = self.state.active_worlds.get_mut(&world_key).ok_or(
                    WorkerError::UnknownWorld {
                        universe_id: submission.universe_id,
                        world_id: world_key,
                    },
                )?;
                rollback_submission_ids
                    .entry(world_key)
                    .or_insert_with(|| world.accepted_submission_ids.clone());
                if !world
                    .accepted_submission_ids
                    .insert(submission.submission_id.clone())
                {
                    self.infra.kafka.record_rejected(RejectedSubmission {
                        submission: submission.clone(),
                        reason: SubmissionRejection::DuplicateSubmissionId,
                    });
                    return Ok(None);
                }
                world.host.journal_bounds().next_seq
            };

        if let SubmissionPayload::Command { command } = &submission.payload {
            let base_record = rollback_record
                .clone()
                .unwrap_or_else(|| synthesize_queued_command_record(command));
            rollback_command_records
                .entry((world_key, command.command_id.clone()))
                .or_insert(base_record);
        }

        let world =
            self.state
                .active_worlds
                .get_mut(&world_key)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id: submission.universe_id,
                    world_id: world_key,
                })?;
        let apply_started = Instant::now();
        let process_result = match &submission.payload {
            SubmissionPayload::Command { command } => {
                let payload = resolve_cbor_payload(store.as_ref(), &command.payload)?;
                run_plane_command(&mut world.host, command, &payload)
            }
            _ => {
                let build_event_started = Instant::now();
                let external_event =
                    submission_to_external_event(store.as_ref(), &submission.payload)?;
                profile.build_external_event += build_event_started.elapsed();
                let host_drain_started = Instant::now();
                (|| -> Result<(), WorkerError> {
                    world.host.enqueue_external(external_event)?;
                    world.host.drain().map(|_| ()).map_err(WorkerError::from)
                })()
                .inspect(|_| {
                    profile.host_drain += host_drain_started.elapsed();
                })
            }
        };
        profile.apply_submission += apply_started.elapsed();
        if let Err(err) = process_result {
            world
                .accepted_submission_ids
                .remove(&submission.submission_id);
            if let SubmissionPayload::Command { command } = &submission.payload {
                let failed = command_failed_record(
                    rollback_command_records
                        .remove(&(submission.world_id, command.command_id.clone()))
                        .unwrap_or_else(|| synthesize_queued_command_record(command)),
                    &err,
                    world.host.heights().head,
                    world.host.kernel().manifest_hash().to_hex(),
                );
                self.infra
                    .blob_meta_for_domain_mut(universe_id)?
                    .put_command_record(submission.world_id, failed)?;
            }
            self.infra.kafka.record_rejected(RejectedSubmission {
                submission: submission.clone(),
                reason: SubmissionRejection::InvalidSubmission {
                    message: err.to_string(),
                },
            });
            return Ok(None);
        }
        let post_apply_started = Instant::now();
        let tail = world.host.kernel().dump_journal_from(journal_tail_start)?;
        if let SubmissionPayload::Command { command } = &submission.payload {
            let succeeded = command_succeeded_record(
                rollback_command_records
                    .get(&(submission.world_id, command.command_id.clone()))
                    .cloned()
                    .unwrap_or_else(|| synthesize_queued_command_record(command)),
                world.host.heights().head,
                world.host.kernel().manifest_hash().to_hex(),
            );
            commit_command_records.push((submission.world_id, succeeded));
        }
        if tail.is_empty() {
            return Ok(None);
        }

        let mut records = Vec::with_capacity(tail.len());
        for entry in tail {
            let record: JournalRecord = serde_cbor::from_slice(&entry.payload)?;
            records.push(record);
        }
        profile.post_apply += post_apply_started.elapsed();

        if expected_world_seq > journal_tail_start {
            tracing::warn!(
                universe_id = %universe_id,
                world_id = %submission.world_id,
                expected_world_seq,
                journal_tail_start,
                "hosted worker world sequence diverged from host journal tail; using host tail"
            );
        } else if expected_world_seq < journal_tail_start {
            tracing::debug!(
                universe_id = %universe_id,
                world_id = %submission.world_id,
                expected_world_seq,
                journal_tail_start,
                "hosted worker world sequence advanced ahead of persisted tail; using host tail"
            );
        }
        let world_seq_start = journal_tail_start;
        let world_seq_end = world_seq_start + records.len() as u64 - 1;
        Ok(Some(WorldLogFrame {
            format_version: 1,
            universe_id,
            world_id: submission.world_id,
            world_epoch: expected_world_epoch,
            world_seq_start,
            world_seq_end,
            records,
        }))
    }

    pub(super) fn process_create_world_submission(
        &mut self,
        submission: &SubmissionEnvelope,
        request: CreateWorldRequest,
        pending_created_worlds: &mut PendingCreatedWorlds,
    ) -> Result<Option<WorldLogFrame>, WorkerError> {
        let world_key = submission.world_id;
        if self.state.registered_worlds.contains_key(&world_key)
            || self.state.active_worlds.contains_key(&world_key)
            || pending_created_worlds.contains_key(&world_key)
        {
            self.infra.kafka.record_rejected(RejectedSubmission {
                submission: submission.clone(),
                reason: SubmissionRejection::WorldAlreadyExists,
            });
            return Ok(None);
        }

        self.prepare_create_request_materialization(request.universe_id, &request)?;
        let store: Arc<HostedCas> = self.infra.store_for_domain(request.universe_id)?;
        let open_started = Instant::now();
        let created = aos_node::create_plane_world_from_request(
            store,
            &request,
            request.universe_id,
            submission.world_id,
            1,
            self.infra.world_config_for_domain(request.universe_id)?,
            EffectAdapterConfig::default(),
            self.kernel_config_for_world(request.universe_id)?,
        )
        .map_err(WorkerError::LogFirst)?;
        let total_open_ms = open_started.elapsed().as_millis();
        let loaded =
            self.load_manifest_into_local_cas(request.universe_id, &created.initial_manifest_hash)?;
        let workflow_modules = loaded
            .modules
            .values()
            .filter(|module| matches!(module.module_kind, ModuleKind::Workflow))
            .map(|module| module.name.to_string())
            .collect::<Vec<_>>();
        let world_store: Arc<HostedCas> = self.infra.store_for_domain(request.universe_id)?;
        let registered = RegisteredWorld {
            universe_id: request.universe_id,
            store: world_store,
            loaded,
            manifest_hash: created.initial_manifest_hash.clone(),
            world_epoch: 1,
            projection_token: Uuid::new_v4().to_string(),
            projection_continuity: None,
            disabled_reason: None,
            metadata: HostedWorldMetadata {
                workflow_modules,
                warnings: Vec::new(),
            },
        };
        let mut accepted_submission_ids = BTreeSet::new();
        accepted_submission_ids.insert(submission.submission_id.clone());
        pending_created_worlds.insert(
            world_key,
            PendingCreatedWorld {
                registered,
                host: created.host,
                accepted_submission_ids,
                total_open_ms,
            },
        );
        Ok(created.initial_frame)
    }

    pub(super) fn prepare_create_request_materialization(
        &mut self,
        _universe_id: UniverseId,
        request: &CreateWorldRequest,
    ) -> Result<(), WorkerError> {
        match &request.source {
            aos_node::CreateWorldSource::Manifest { manifest_hash } => {
                let _ = self.load_manifest_into_local_cas(request.universe_id, manifest_hash)?;
            }
            aos_node::CreateWorldSource::Seed { seed } => {
                let manifest_hash = seed.baseline.manifest_hash.as_deref().ok_or_else(|| {
                    WorkerError::Persist(aos_node::PersistError::validation(
                        "seed baseline requires manifest_hash",
                    ))
                })?;
                let _ = self.load_manifest_into_local_cas(request.universe_id, manifest_hash)?;
                self.hydrate_snapshot_into_local_cas(
                    request.universe_id,
                    &seed.baseline.snapshot_ref,
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn rollback_active_worlds(
        &mut self,
        rollback_submission_ids: SubmissionRollbackIds,
    ) -> Result<(), WorkerError> {
        for (world_id, accepted_submission_ids) in rollback_submission_ids {
            let universe_id = self
                .state
                .registered_worlds
                .get(&world_id)
                .map(|world| world.universe_id)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id: self.infra.default_universe_id,
                    world_id,
                })?;
            let host = self.reopen_registered_world_host_from_log(universe_id, world_id)?;
            let active_baseline = self.select_source_snapshot(
                universe_id,
                world_id,
                &aos_node::SnapshotSelector::ActiveBaseline,
            )?;
            let projection_bootstrapped = self.prepare_projection_continuity_for_reopen(
                world_id,
                host.heights().head,
                &active_baseline,
            )?;
            let (last_checkpointed_head, last_checkpointed_at_ns) = self
                .state
                .registered_worlds
                .get(&world_id)
                .map(|world| world.universe_id)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id: self.infra.default_universe_id,
                    world_id,
                })
                .and_then(|universe_id| {
                    self.checkpoint_watermark_for_world(universe_id, world_id)
                        .map(|watermark| watermark.unwrap_or((0, 0)))
                })?;
            self.state.active_worlds.insert(
                world_id,
                super::types::ActiveWorld {
                    host,
                    accepted_submission_ids,
                    last_checkpointed_head,
                    last_checkpointed_at_ns,
                    projection_bootstrapped,
                },
            );
        }
        Ok(())
    }

    pub(super) fn rollback_command_records(
        &mut self,
        rollback_command_records: CommandRollbackRecords,
    ) -> Result<(), WorkerError> {
        for ((world_id, _command_id), record) in rollback_command_records {
            let universe_id = self
                .state
                .registered_worlds
                .get(&world_id)
                .map(|world| world.universe_id)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id: self.infra.default_universe_id,
                    world_id,
                })?;
            self.infra
                .blob_meta_for_domain_mut(universe_id)?
                .put_command_record(world_id, record)?;
        }
        Ok(())
    }

    pub(super) fn command_rollback_record(
        &mut self,
        world_id: WorldId,
        command: &CommandIngress,
    ) -> Result<CommandRecord, WorkerError> {
        let universe_id = self
            .state
            .registered_worlds
            .get(&world_id)
            .map(|world| world.universe_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id: self.infra.default_universe_id,
                world_id,
            })?;
        Ok(self
            .infra
            .blob_meta_for_domain_mut(universe_id)?
            .get_command_record(world_id, &command.command_id)?
            .unwrap_or_else(|| synthesize_queued_command_record(command)))
    }

    pub(super) fn finalize_created_worlds(
        &mut self,
        pending_created_worlds: PendingCreatedWorlds,
    ) -> Result<(), WorkerError> {
        for (world_id, pending_created) in pending_created_worlds {
            if self.state.registered_worlds.contains_key(&world_id)
                || self.state.active_worlds.contains_key(&world_id)
            {
                return Err(WorkerError::Persist(aos_node::PersistError::validation(
                    format!("world {world_id} already exists during create finalization"),
                )));
            }
            let universe_id = pending_created.registered.universe_id;
            self.state
                .registered_worlds
                .insert(world_id, pending_created.registered);
            self.state.active_worlds.insert(
                world_id,
                super::types::ActiveWorld {
                    host: pending_created.host,
                    accepted_submission_ids: pending_created.accepted_submission_ids,
                    last_checkpointed_head: 0,
                    last_checkpointed_at_ns: 0,
                    projection_bootstrapped: false,
                },
            );
            self.log_world_opened(
                universe_id,
                world_id,
                "create",
                pending_created.total_open_ms,
            );
        }
        Ok(())
    }
}
