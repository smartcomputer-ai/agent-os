use std::collections::{BTreeMap, BTreeSet};

use aos_kernel::WorldInput;
use aos_node::{
    CheckpointBackend, CreateWorldSource, EffectExecutionClass, HostControl, JournalBackend,
    JournalCommit, JournalDisposition, JournalFlush, JournalSourceAck, SubmissionPayload,
    WorldCheckpointRef, WorldId, classify_effect_kind, partition_for_world,
};

use super::commands::{
    command_failed_record, command_succeeded_record, synthesize_queued_command_record,
};
use super::core::{
    AckRef, CompletedSlice, DurableDisposition, FlushOutcome, KafkaOffsetAck, LocalInputMsg,
    PendingState, SchedulerMsg, WorkItem,
};
use super::types::{AsyncWorldState, HostedWorkerCore, WorkerError};
use super::util::{
    adapter_start_context_from_opened, build_timer_receipt, snapshot_record_from_checkpoint,
    snapshot_record_from_frames, timer_entry_from_intent, unix_time_ns,
};

impl HostedWorkerCore {
    pub(super) fn flush_ready_batch(&mut self) -> Result<Option<FlushOutcome>, WorkerError> {
        let active_worlds = &self.state.active_worlds;
        let Some(batch) = self.state.scheduler.collect_flushable_slices(
            self.flush_limits,
            self.max_local_continuation_slices_per_flush,
            |slice, batch| Self::slice_is_flush_eligible(active_worlds, slice, batch),
        ) else {
            return Ok(None);
        };

        let slice_ids = batch.slice_ids.clone();
        let ack_refs = batch.ack_refs.clone();
        if self.debug_skip_flush_commit {
            let committed_slices = slice_ids
                .iter()
                .filter_map(|slice_id| self.state.scheduler.staged_slices.get(slice_id).cloned())
                .collect::<Vec<_>>();
            return Ok(Some(FlushOutcome {
                committed_slices,
                ack_refs,
                journal_commit: JournalCommit::default(),
            }));
        }
        let flush = JournalFlush {
            frames: batch.frames,
            dispositions: batch
                .dispositions
                .into_iter()
                .map(journal_disposition)
                .collect(),
            source_acks: journal_source_acks(&batch.ack_refs),
        };

        let journal_commit = match JournalBackend::commit_flush(&mut self.infra.journal, flush) {
            Ok(commit) => commit,
            Err(err) => {
                if matches!(err, aos_node::BackendError::NonContiguousWorldSeq { .. }) {
                    JournalBackend::refresh_all(&mut self.infra.journal)?;
                    self.handle_flush_failure(&slice_ids)?;
                    return Ok(None);
                }
                self.handle_flush_failure(&slice_ids)?;
                return Err(WorkerError::LogFirst(err));
            }
        };

        let committed_slices = slice_ids
            .iter()
            .filter_map(|slice_id| self.state.scheduler.staged_slices.get(slice_id).cloned())
            .collect::<Vec<_>>();
        Ok(Some(FlushOutcome {
            committed_slices,
            ack_refs: batch.ack_refs,
            journal_commit,
        }))
    }

    fn slice_is_flush_eligible(
        active_worlds: &std::collections::BTreeMap<WorldId, super::types::ActiveWorld>,
        slice: &CompletedSlice,
        batch: &super::core::FlushBatch,
    ) -> bool {
        for world_id in &slice.affected_worlds {
            let Some(world) = active_worlds.get(world_id) else {
                return false;
            };
            let mut found = false;
            for pending_slice_id in &world.pending_slices {
                if *pending_slice_id == slice.id {
                    found = true;
                    break;
                }
                if !batch.slice_ids.contains(pending_slice_id) {
                    return false;
                }
            }
            if !found {
                return false;
            }
        }
        true
    }

    pub(super) fn finalize_flush_success(
        &mut self,
        outcome: FlushOutcome,
    ) -> Result<(), WorkerError> {
        if self.debug_fail_after_next_flush_commit {
            self.debug_fail_after_next_flush_commit = false;
            return Err(WorkerError::Persist(aos_node::PersistError::backend(
                "debug fail after durable flush commit before post-commit",
            )));
        }
        let committed_offsets = ack_offsets_by_partition(&outcome.ack_refs);
        if !committed_offsets.is_empty() {
            self.state.scheduler.advance_flush_rr_cursor();
        }
        for (partition, last_offset) in committed_offsets {
            if let Some(queue) = self
                .state
                .scheduler
                .pending_by_partition
                .get_mut(&partition)
            {
                while queue
                    .front()
                    .is_some_and(|entry| entry.ack.offset <= last_offset)
                {
                    queue.pop_front();
                }
            }
        }

        let committed_ids = outcome
            .committed_slices
            .iter()
            .map(|slice| slice.id)
            .collect::<Vec<_>>();
        let completed_accept_tokens = outcome
            .ack_refs
            .iter()
            .filter_map(|ack_ref| direct_accept_token(*ack_ref))
            .collect::<BTreeSet<_>>();
        self.remove_slice_tracking(&committed_ids);

        for slice in outcome.committed_slices {
            self.state.scheduler.staged_slices.remove(&slice.id);
            self.apply_post_commit(slice, &outcome.journal_commit)?;
        }
        for accept_token in completed_accept_tokens {
            if let Some(waiter) = self.state.accept_waiters.remove(&accept_token) {
                waiter.complete();
            }
        }
        Ok(())
    }

    fn apply_post_commit(
        &mut self,
        slice: CompletedSlice,
        journal_commit: &JournalCommit,
    ) -> Result<(), WorkerError> {
        for world_id in &slice.affected_worlds {
            if let Some(world) = self.state.active_worlds.get_mut(world_id) {
                Self::remove_pending_slice_id(world, slice.id);
                world.running = false;
            }
        }
        if let Some(checkpoint) = slice.checkpoint {
            self.apply_checkpoint_post_commit(checkpoint, journal_commit)?;
            for world_id in slice.affected_worlds {
                self.mark_world_ready(world_id);
            }
            return Ok(());
        }

        let world_id = slice.world_id;
        let pending_created = self
            .state
            .pending_created_worlds
            .remove(&world_id)
            .is_some();
        let mut inline_followups = Vec::new();
        let mut timer_intents = Vec::new();
        let mut external_intents = Vec::new();
        let mut manifest_hash = None;
        let mut journal_height = None;
        let mut universe_id = None;

        if let Some(world) = self.state.active_worlds.get_mut(&world_id) {
            universe_id = Some(world.universe_id);
            if let Some(frame) = slice.frames.last() {
                world.next_world_seq = world
                    .next_world_seq
                    .max(frame.world_seq_end.saturating_add(1));
                journal_height = Some(frame.world_seq_end);
                manifest_hash = Some(world.kernel.manifest_hash().to_hex());
                if let Some(snapshot) = snapshot_record_from_frames(&slice.frames, |_| true) {
                    world.active_baseline = snapshot;
                }
            }

            for opened in &slice.opened_effects {
                match classify_effect_kind(opened.intent.kind.as_str()) {
                    EffectExecutionClass::InlineInternal => {
                        if let Some(receipt) =
                            world.kernel.handle_internal_intent(&opened.intent)?
                        {
                            inline_followups.push(receipt);
                        }
                    }
                    EffectExecutionClass::OwnerLocalTimer => {
                        timer_intents.push(opened.intent.clone());
                    }
                    EffectExecutionClass::ExternalAsync => {
                        external_intents.push((
                            opened.intent.clone(),
                            adapter_start_context_from_opened(opened),
                        ));
                    }
                }
            }
        }

        let scheduler_tx = self.scheduler_tx.clone();
        if let Some(async_state) = self.state.async_worlds.get_mut(&world_id) {
            for intent in timer_intents {
                Self::ensure_timer_started(scheduler_tx.clone(), world_id, async_state, intent)?;
            }
        }
        if let Some(registered) = self.state.registered_worlds.get(&world_id) {
            for (intent, context) in external_intents {
                let _ = registered
                    .effect_runtime
                    .ensure_started_with_context(world_id, intent, context)?;
            }
        }

        if pending_created {
            self.rehydrate_runtime_work(world_id)?;
            if let Some(world) = self.state.active_worlds.get(&world_id) {
                let source_kind = pending_created_source_kind(slice.original_item.as_ref());
                let total_create_ms = if world.created_at_ns > 0 {
                    unix_time_ns().saturating_sub(world.created_at_ns) as u128 / 1_000_000
                } else {
                    0
                };
                let initial_record_count = slice
                    .frames
                    .iter()
                    .map(|frame| frame.records.len())
                    .sum::<usize>();
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
                    &world.kernel.manifest_hash().to_hex(),
                );
            }
        }
        for receipt in inline_followups.into_iter().rev() {
            self.enqueue_local_input_front(world_id, WorldInput::Receipt(receipt))?;
        }

        if let Some(WorkItem::Accepted { envelope, .. }) = slice.original_item.as_ref()
            && let Some(command) = envelope.command.as_ref()
            && let Some(universe_id) = universe_id
        {
            let meta = self.infra.checkpoint_backend_for_domain_mut(universe_id)?;
            let base = meta
                .get_command_record(world_id, &command.command_id)?
                .unwrap_or_else(|| synthesize_queued_command_record(command));
            let record = if slice.disposition.as_ref().is_some_and(|disposition| {
                matches!(disposition, DurableDisposition::CommandFailure { .. })
            }) {
                command_failed_record(
                    base,
                    &WorkerError::Persist(aos_node::PersistError::validation("command failed")),
                    journal_height.unwrap_or_default(),
                    manifest_hash.unwrap_or_default(),
                )
            } else {
                command_succeeded_record(
                    base,
                    journal_height.unwrap_or_default(),
                    manifest_hash.unwrap_or_default(),
                )
            };
            meta.put_command_record(world_id, record)?;
        }

        self.mark_world_ready(world_id);
        Ok(())
    }

    fn apply_checkpoint_post_commit(
        &mut self,
        checkpoint: super::core::CheckpointCommit,
        journal_commit: &JournalCommit,
    ) -> Result<(), WorkerError> {
        let world_count = checkpoint.worlds.len();
        let mut compaction_targets = Vec::new();
        let mut checkpointed_worlds = Vec::with_capacity(world_count);
        let worlds_with_cursors = journal_commit.world_cursors.len();

        for world_commit in checkpoint.worlds {
            if let Some(world) = self.state.active_worlds.get_mut(&world_commit.world_id) {
                world.last_checkpointed_head = world_commit.baseline.height;
                world.last_checkpointed_at_ns = checkpoint.created_at_ns;
                world.pending_create_checkpoint = false;
                world.next_world_seq = world
                    .next_world_seq
                    .max(world_commit.world_seq.saturating_add(1));
                let snapshot = snapshot_record_from_checkpoint(&world_commit.baseline);
                if snapshot.height >= world.active_baseline.height {
                    world.active_baseline = snapshot;
                }
            }
            if let Some(height) = world_commit.compact_through {
                compaction_targets.push((world_commit.world_id, height));
            }
            let blob_meta = self
                .infra
                .checkpoint_backend_for_domain_mut(world_commit.universe_id)?;
            let previous_cursor = blob_meta
                .latest_world_checkpoint(world_commit.world_id)?
                .and_then(|previous| previous.journal_cursor);
            let journal_cursor = journal_commit
                .world_cursors
                .get(&world_commit.world_id)
                .cloned()
                .or(previous_cursor);
            checkpointed_worlds.push(format_checkpointed_world(
                world_commit.universe_id,
                world_commit.world_id,
                world_commit.world_epoch,
                world_commit.baseline.height,
                world_commit.world_seq,
                world_commit.compact_through,
                journal_cursor.as_ref(),
            ));
            blob_meta.commit_world_checkpoint(WorldCheckpointRef {
                universe_id: world_commit.universe_id,
                world_id: world_commit.world_id,
                world_epoch: world_commit.world_epoch,
                checkpointed_at_ns: checkpoint.created_at_ns,
                world_seq: world_commit.world_seq,
                baseline: world_commit.baseline,
                journal_cursor,
            })?;
        }

        for (world_id, height) in compaction_targets {
            if let Some(world) = self.state.active_worlds.get_mut(&world_id) {
                world.kernel.compact_journal_through(height)?;
            }
        }

        tracing::info!(
            worlds_with_cursors,
            worlds = world_count,
            checkpointed_worlds = %checkpointed_worlds.join(","),
            trigger = checkpoint.trigger,
            "aos-node checkpoint published"
        );
        Ok(())
    }

    pub(super) fn ensure_timer_started(
        scheduler_tx: Option<tokio::sync::mpsc::UnboundedSender<SchedulerMsg>>,
        world_id: WorldId,
        async_state: &mut AsyncWorldState,
        intent: aos_effects::EffectIntent,
    ) -> Result<(), WorkerError> {
        if !async_state.scheduled_timers.insert(intent.intent_hash) {
            return Ok(());
        }
        async_state.timer_scheduler.schedule(&intent)?;
        let Some(scheduler_tx) = scheduler_tx else {
            return Ok(());
        };

        let entry = timer_entry_from_intent(&intent)?;
        let deadline = entry.deadline_instant(unix_time_ns());
        let handle = tokio::spawn(async move {
            tokio::time::sleep_until(deadline.into()).await;
            let _ = scheduler_tx.send(SchedulerMsg::LocalInput(LocalInputMsg {
                world_id,
                input: WorldInput::Receipt(build_timer_receipt(&entry).unwrap_or_else(|_| {
                    aos_effects::EffectReceipt {
                        intent_hash: intent.intent_hash,
                        adapter_id: "timer.local".into(),
                        status: aos_effects::ReceiptStatus::Error,
                        payload_cbor: Vec::new(),
                        cost_cents: None,
                        signature: Vec::new(),
                    }
                })),
            }));
        });
        async_state.timer_tasks.insert(intent.intent_hash, handle);
        Ok(())
    }

    fn handle_flush_failure(&mut self, slice_ids: &[u64]) -> Result<(), WorkerError> {
        let attempted_slices = slice_ids
            .iter()
            .filter_map(|slice_id| self.state.scheduler.staged_slices.get(slice_id).cloned())
            .collect::<Vec<_>>();
        let touched_worlds = attempted_slices
            .iter()
            .flat_map(|slice| slice.affected_worlds.iter().copied())
            .collect::<std::collections::BTreeSet<_>>();
        let rollback_slice_ids = self
            .state
            .scheduler
            .staged_slices
            .iter()
            .filter_map(|(slice_id, slice)| {
                slice
                    .affected_worlds
                    .iter()
                    .any(|world_id| touched_worlds.contains(world_id))
                    .then_some(*slice_id)
            })
            .collect::<std::collections::BTreeSet<_>>();
        let rollback_slices = rollback_slice_ids
            .iter()
            .filter_map(|slice_id| self.state.scheduler.staged_slices.get(slice_id).cloned())
            .collect::<Vec<_>>();
        let mut dropped_submission_ids_by_world =
            std::collections::BTreeMap::<WorldId, Vec<String>>::new();
        let mut rollback_order_by_world =
            std::collections::BTreeMap::<WorldId, Vec<CompletedSlice>>::new();
        let pending_created_worlds = touched_worlds
            .iter()
            .copied()
            .filter(|world_id| self.state.pending_created_worlds.contains_key(world_id))
            .collect::<std::collections::BTreeSet<_>>();

        for world_id in &touched_worlds {
            let Some(world) = self.state.active_worlds.get(world_id) else {
                continue;
            };
            for pending_slice_id in &world.pending_slices {
                if !rollback_slice_ids.contains(pending_slice_id) {
                    continue;
                }
                let Some(slice) = self.state.scheduler.staged_slices.get(pending_slice_id) else {
                    continue;
                };
                rollback_order_by_world
                    .entry(*world_id)
                    .or_default()
                    .push(slice.clone());
            }
        }

        for slice in &rollback_slices {
            if let Some(WorkItem::Accepted { envelope, .. }) = slice.original_item.as_ref() {
                dropped_submission_ids_by_world
                    .entry(slice.world_id)
                    .or_default()
                    .push(envelope.submission_id.clone());
            }
        }

        for slice in &rollback_slices {
            if let Some(token) = slice.ack_ref.and_then(kafka_offset_ack)
                && let Some(queue) = self
                    .state
                    .scheduler
                    .pending_by_partition
                    .get_mut(&token.partition)
                && let Some(entry) = queue.iter_mut().find(|entry| entry.ack == token)
            {
                entry.state = PendingState::Received;
            }
        }
        let rollback_slice_ids = rollback_slice_ids.into_iter().collect::<Vec<_>>();
        self.remove_slice_tracking(&rollback_slice_ids);
        for slice_id in &rollback_slice_ids {
            self.state.scheduler.staged_slices.remove(slice_id);
        }

        for world_id in touched_worlds {
            if pending_created_worlds.contains(&world_id) {
                self.remove_pending_created_world_state(world_id);
                continue;
            }
            if self.state.active_worlds.contains_key(&world_id) {
                let drop_submission_ids = dropped_submission_ids_by_world
                    .get(&world_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                self.reopen_active_world(world_id, drop_submission_ids)?;
            }
        }

        for world_id in &pending_created_worlds {
            self.restore_pending_created_world_ingress(*world_id)?;
        }

        for (world_id, slices) in rollback_order_by_world {
            if pending_created_worlds.contains(&world_id) {
                continue;
            }
            let Some(world) = self.state.active_worlds.get_mut(&world_id) else {
                continue;
            };
            for slice in slices.into_iter().rev() {
                if let Some(item) = slice.original_item {
                    world.mailbox.push_front(item);
                }
            }
            self.mark_world_ready(world_id);
        }

        Ok(())
    }

    pub(super) fn remove_pending_created_world_state(&mut self, world_id: WorldId) {
        self.state.pending_created_worlds.remove(&world_id);
        self.state
            .ready_worlds
            .retain(|candidate| *candidate != world_id);
        if let Some(mut async_state) = self.state.async_worlds.remove(&world_id) {
            async_state.abort_all_timers();
        }
        self.state.active_worlds.remove(&world_id);
        self.state.registered_worlds.remove(&world_id);
    }

    fn restore_pending_created_world_ingress(
        &mut self,
        world_id: WorldId,
    ) -> Result<(), WorkerError> {
        let partition = partition_for_world(world_id, self.infra.journal.partition_count());
        let entries = self
            .state
            .scheduler
            .pending_by_partition
            .get(&partition)
            .map(|queue| {
                queue
                    .iter()
                    .filter(|entry| entry.envelope.world_id == world_id)
                    .map(|entry| (entry.ack, entry.envelope.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for (token, envelope) in entries {
            self.route_accepted_submission(AckRef::KafkaOffset(token), envelope)?;
        }
        Ok(())
    }

    pub(super) fn remove_slice_tracking(&mut self, slice_ids: &[u64]) {
        self.state
            .scheduler
            .ready_non_kafka_slices
            .retain(|slice_id| !slice_ids.contains(slice_id));
    }
}

fn journal_disposition(disposition: DurableDisposition) -> JournalDisposition {
    match disposition {
        DurableDisposition::RejectedSubmission {
            ack_ref,
            world_id,
            reason,
        } => JournalDisposition::RejectedSubmission {
            source_ack: ack_ref.and_then(journal_source_ack),
            world_id,
            reason,
        },
        DurableDisposition::CommandFailure {
            ack_ref,
            world_id,
            command_id,
            error_code,
        } => JournalDisposition::CommandFailure {
            source_ack: ack_ref.and_then(journal_source_ack),
            world_id,
            command_id,
            error_code,
        },
    }
}

fn kafka_offset_ack(ack_ref: AckRef) -> Option<KafkaOffsetAck> {
    match ack_ref {
        AckRef::KafkaOffset(ack) => Some(ack),
        AckRef::DirectAccept { .. } => None,
    }
}

fn journal_source_ack(ack_ref: AckRef) -> Option<JournalSourceAck> {
    kafka_offset_ack(ack_ref).map(|ack| JournalSourceAck::PartitionOffset {
        partition: ack.partition,
        offset: ack.offset,
    })
}

fn journal_source_acks(ack_refs: &[AckRef]) -> Vec<JournalSourceAck> {
    ack_refs
        .iter()
        .filter_map(|ack_ref| journal_source_ack(*ack_ref))
        .collect()
}

fn direct_accept_token(ack_ref: AckRef) -> Option<u64> {
    match ack_ref {
        AckRef::DirectAccept { accept_token } => Some(accept_token),
        AckRef::KafkaOffset(_) => None,
    }
}

fn ack_offsets_by_partition(ack_refs: &[AckRef]) -> BTreeMap<u32, i64> {
    let mut commits = BTreeMap::new();
    for ack_ref in ack_refs {
        if let Some(ack) = kafka_offset_ack(*ack_ref) {
            commits
                .entry(ack.partition)
                .and_modify(|offset: &mut i64| *offset = (*offset).max(ack.offset))
                .or_insert(ack.offset);
        }
    }
    commits
}

fn format_checkpointed_world(
    universe_id: aos_node::UniverseId,
    world_id: aos_node::WorldId,
    world_epoch: u64,
    checkpoint_height: u64,
    world_seq: u64,
    compact_through: Option<u64>,
    journal_cursor: Option<&aos_node::WorldJournalCursor>,
) -> String {
    let compact = compact_through
        .map(|height| height.to_string())
        .unwrap_or_else(|| "none".to_owned());
    let cursor = journal_cursor
        .map(format_journal_cursor)
        .unwrap_or_else(|| "none".to_owned());
    format!(
        "universe={universe_id} world={world_id} epoch={world_epoch} checkpoint_height={checkpoint_height} world_seq={world_seq} compact_through={compact} cursor={cursor}"
    )
}

fn format_journal_cursor(cursor: &aos_node::WorldJournalCursor) -> String {
    match cursor {
        aos_node::WorldJournalCursor::Kafka {
            journal_topic,
            partition,
            journal_offset,
        } => format!("kafka:{journal_topic}:{partition}:{journal_offset}"),
        aos_node::WorldJournalCursor::Sqlite { frame_offset } => {
            format!("sqlite:{frame_offset}")
        }
    }
}

fn pending_created_source_kind(item: Option<&WorkItem>) -> &'static str {
    let Some(WorkItem::Accepted { envelope, .. }) = item else {
        return "unknown";
    };
    let SubmissionPayload::HostControl { control } = &envelope.payload else {
        return "unknown";
    };
    let HostControl::CreateWorld { request } = control;
    match &request.source {
        CreateWorldSource::Manifest { .. } => "manifest",
        CreateWorldSource::Seed { .. } => "seed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use aos_air_types::{CURRENT_AIR_VERSION, Manifest};
    use aos_cbor::to_canonical_cbor;
    use aos_node::{CreateWorldRequest, UniverseId, WorldLogFrame};

    use crate::worker::HostedWorkerRuntime;
    use crate::worker::core::{AckRef, KafkaOffsetAck, PendingIngressEntry};

    fn empty_manifest_hash(runtime: &HostedWorkerRuntime, universe_id: UniverseId) -> String {
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: Vec::new(),
            modules: Vec::new(),
            effects: Vec::new(),
            effect_bindings: Vec::new(),
            caps: Vec::new(),
            policies: Vec::new(),
            secrets: Vec::new(),
            defaults: None,
            module_bindings: Default::default(),
            routing: None,
        };
        let manifest_bytes = to_canonical_cbor(&manifest).expect("encode manifest");
        runtime
            .put_blob(universe_id, &manifest_bytes)
            .expect("store manifest")
            .to_hex()
    }

    fn create_empty_world(
        runtime: &HostedWorkerRuntime,
        universe_id: UniverseId,
        manifest_hash: &str,
    ) -> (WorldId, u64) {
        let accepted = runtime
            .create_world(
                universe_id,
                CreateWorldRequest {
                    world_id: None,
                    universe_id,
                    created_at_ns: 1,
                    source: CreateWorldSource::Manifest {
                        manifest_hash: manifest_hash.to_owned(),
                    },
                },
            )
            .expect("create world");
        let summary = runtime
            .get_world(universe_id, accepted.world_id)
            .expect("world summary");
        (accepted.world_id, summary.world_epoch)
    }

    fn runtime_with_empty_world() -> (HostedWorkerRuntime, UniverseId, WorldId, u64) {
        let runtime = HostedWorkerRuntime::new_embedded_kafka(1).expect("embedded runtime");
        let universe_id = runtime.default_universe_id().expect("default universe");
        let manifest_hash = empty_manifest_hash(&runtime, universe_id);
        let (world_id, world_epoch) = create_empty_world(&runtime, universe_id, &manifest_hash);
        (runtime, universe_id, world_id, world_epoch)
    }

    #[test]
    fn apply_post_commit_does_not_regress_next_world_seq_below_staged_successor() {
        let (runtime, universe_id, world_id, world_epoch) = runtime_with_empty_world();
        let mut core = runtime.lock_core().expect("lock core");
        let reserved_second_seq = 2;
        {
            let world = core
                .state
                .active_worlds
                .get_mut(&world_id)
                .expect("active world");
            world.pending_slices.push_back(10);
            world.pending_slices.push_back(11);
            world.next_world_seq = reserved_second_seq;
            HostedWorkerCore::sync_pending_slice_flags(world);
        }

        let first_slice = CompletedSlice {
            id: 10,
            world_id,
            affected_worlds: vec![world_id],
            staged_at: std::time::Instant::now(),
            ack_ref: None,
            original_item: Some(WorkItem::LocalInput(WorldInput::DomainEvent(
                aos_wasm_abi::DomainEvent {
                    schema: "demo/Noop@1".into(),
                    value: vec![1],
                    key: None,
                },
            ))),
            frames: vec![WorldLogFrame {
                format_version: 1,
                universe_id,
                world_id,
                world_epoch,
                world_seq_start: 0,
                world_seq_end: 0,
                records: Vec::new(),
            }],
            disposition: None,
            opened_effects: Vec::new(),
            checkpoint: None,
            approx_bytes: 1,
        };

        core.apply_post_commit(first_slice, &JournalCommit::default())
            .expect("apply post commit");

        let world = core
            .state
            .active_worlds
            .get(&world_id)
            .expect("active world after commit");
        assert_eq!(world.pending_slices.len(), 1);
        assert_eq!(world.pending_slices.front().copied(), Some(11));
        assert!(
            world.next_world_seq >= reserved_second_seq,
            "next_world_seq regressed below already staged successor"
        );
    }

    #[test]
    fn flush_ready_batch_does_not_commit_later_same_world_local_slice_before_older_predecessor() {
        let runtime = HostedWorkerRuntime::new_embedded_kafka(1).expect("embedded runtime");
        let universe_id = runtime.default_universe_id().expect("default universe");
        let manifest_hash = empty_manifest_hash(&runtime, universe_id);
        let (world_a, world_a_epoch) = create_empty_world(&runtime, universe_id, &manifest_hash);
        let (world_b, world_b_epoch) = create_empty_world(&runtime, universe_id, &manifest_hash);
        let mut core = runtime.lock_core().expect("lock core");
        core.flush_limits.max_bytes = 2;
        core.flush_limits.max_slices = 8;

        {
            let world = core
                .state
                .active_worlds
                .get_mut(&world_a)
                .expect("active world a");
            world.pending_slices.push_back(10);
            world.pending_slices.push_back(11);
            HostedWorkerCore::sync_pending_slice_flags(world);
        }
        {
            let world = core
                .state
                .active_worlds
                .get_mut(&world_b)
                .expect("active world b");
            world.pending_slices.push_back(20);
            HostedWorkerCore::sync_pending_slice_flags(world);
        }
        core.state.scheduler.pending_by_partition.insert(
            0,
            std::collections::VecDeque::from(vec![PendingIngressEntry {
                ack: KafkaOffsetAck {
                    partition: 0,
                    offset: 1,
                },
                envelope: aos_node::SubmissionEnvelope {
                    submission_id: "sub-b".into(),
                    universe_id,
                    world_id: world_b,
                    world_epoch: world_b_epoch,
                    command: None,
                    payload: SubmissionPayload::WorldInput {
                        input: WorldInput::DomainEvent(aos_wasm_abi::DomainEvent {
                            schema: "demo/Noop@1".into(),
                            value: vec![1],
                            key: None,
                        }),
                    },
                },
                state: PendingState::Serviced(20),
            }]),
        );
        core.state.scheduler.stage_slice(CompletedSlice {
            id: 10,
            world_id: world_a,
            affected_worlds: vec![world_a],
            staged_at: std::time::Instant::now(),
            ack_ref: None,
            original_item: Some(WorkItem::LocalInput(WorldInput::DomainEvent(
                aos_wasm_abi::DomainEvent {
                    schema: "demo/Noop@1".into(),
                    value: vec![1],
                    key: None,
                },
            ))),
            frames: vec![WorldLogFrame {
                format_version: 1,
                universe_id,
                world_id: world_a,
                world_epoch: world_a_epoch,
                world_seq_start: 0,
                world_seq_end: 0,
                records: Vec::new(),
            }],
            disposition: None,
            opened_effects: Vec::new(),
            checkpoint: None,
            approx_bytes: 2,
        });
        core.state.scheduler.stage_slice(CompletedSlice {
            id: 11,
            world_id: world_a,
            affected_worlds: vec![world_a],
            staged_at: std::time::Instant::now(),
            ack_ref: None,
            original_item: Some(WorkItem::LocalInput(WorldInput::DomainEvent(
                aos_wasm_abi::DomainEvent {
                    schema: "demo/Noop@1".into(),
                    value: vec![1],
                    key: None,
                },
            ))),
            frames: vec![WorldLogFrame {
                format_version: 1,
                universe_id,
                world_id: world_a,
                world_epoch: world_a_epoch,
                world_seq_start: 1,
                world_seq_end: 1,
                records: Vec::new(),
            }],
            disposition: None,
            opened_effects: Vec::new(),
            checkpoint: None,
            approx_bytes: 1,
        });
        core.state.scheduler.stage_slice(CompletedSlice {
            id: 20,
            world_id: world_b,
            affected_worlds: vec![world_b],
            staged_at: std::time::Instant::now(),
            ack_ref: Some(AckRef::KafkaOffset(KafkaOffsetAck {
                partition: 0,
                offset: 1,
            })),
            original_item: Some(WorkItem::Accepted {
                ack_ref: AckRef::KafkaOffset(KafkaOffsetAck {
                    partition: 0,
                    offset: 1,
                }),
                envelope: aos_node::SubmissionEnvelope {
                    submission_id: "sub-b".into(),
                    universe_id,
                    world_id: world_b,
                    world_epoch: world_b_epoch,
                    command: None,
                    payload: SubmissionPayload::WorldInput {
                        input: WorldInput::DomainEvent(aos_wasm_abi::DomainEvent {
                            schema: "demo/Noop@1".into(),
                            value: vec![1],
                            key: None,
                        }),
                    },
                },
            }),
            frames: vec![WorldLogFrame {
                format_version: 1,
                universe_id,
                world_id: world_b,
                world_epoch: world_b_epoch,
                world_seq_start: 0,
                world_seq_end: 0,
                records: Vec::new(),
            }],
            disposition: None,
            opened_effects: Vec::new(),
            checkpoint: None,
            approx_bytes: 1,
        });

        let batch = core
            .state
            .scheduler
            .collect_flushable_slices(
                core.flush_limits,
                core.max_local_continuation_slices_per_flush,
                |slice, batch| {
                    HostedWorkerCore::slice_is_flush_eligible(
                        &core.state.active_worlds,
                        slice,
                        batch,
                    )
                },
            )
            .expect("flush batch");
        assert_eq!(batch.slice_ids, vec![20]);
    }

    #[test]
    fn handle_flush_failure_requeues_all_speculative_slices_for_reopened_world() {
        let runtime = HostedWorkerRuntime::new_embedded_kafka(1).expect("embedded runtime");
        let universe_id = runtime.default_universe_id().expect("default universe");
        let manifest_hash = empty_manifest_hash(&runtime, universe_id);
        let (world_id, world_epoch) = create_empty_world(&runtime, universe_id, &manifest_hash);
        let mut core = runtime.lock_core().expect("lock core");

        let token_1 = KafkaOffsetAck {
            partition: 0,
            offset: 1,
        };
        let token_2 = KafkaOffsetAck {
            partition: 0,
            offset: 2,
        };
        let envelope_1 = aos_node::SubmissionEnvelope {
            submission_id: "sub-1".into(),
            universe_id,
            world_id,
            world_epoch,
            command: None,
            payload: SubmissionPayload::WorldInput {
                input: WorldInput::DomainEvent(aos_wasm_abi::DomainEvent {
                    schema: "demo/Noop@1".into(),
                    value: vec![1],
                    key: None,
                }),
            },
        };
        let envelope_2 = aos_node::SubmissionEnvelope {
            submission_id: "sub-2".into(),
            universe_id,
            world_id,
            world_epoch,
            command: None,
            payload: SubmissionPayload::WorldInput {
                input: WorldInput::DomainEvent(aos_wasm_abi::DomainEvent {
                    schema: "demo/Noop@1".into(),
                    value: vec![2],
                    key: None,
                }),
            },
        };

        core.state.scheduler.pending_by_partition.insert(
            0,
            std::collections::VecDeque::from(vec![
                PendingIngressEntry {
                    ack: token_1,
                    envelope: envelope_1.clone(),
                    state: PendingState::Received,
                },
                PendingIngressEntry {
                    ack: token_2,
                    envelope: envelope_2.clone(),
                    state: PendingState::Received,
                },
            ]),
        );
        core.stage_completed_slice(CompletedSlice {
            id: 10,
            world_id,
            affected_worlds: vec![world_id],
            staged_at: std::time::Instant::now(),
            ack_ref: Some(AckRef::KafkaOffset(token_1)),
            original_item: Some(WorkItem::Accepted {
                ack_ref: AckRef::KafkaOffset(token_1),
                envelope: envelope_1.clone(),
            }),
            frames: vec![WorldLogFrame {
                format_version: 1,
                universe_id,
                world_id,
                world_epoch,
                world_seq_start: 0,
                world_seq_end: 0,
                records: Vec::new(),
            }],
            disposition: None,
            opened_effects: Vec::new(),
            checkpoint: None,
            approx_bytes: 1,
        })
        .expect("stage first slice");
        core.stage_completed_slice(CompletedSlice {
            id: 11,
            world_id,
            affected_worlds: vec![world_id],
            staged_at: std::time::Instant::now(),
            ack_ref: Some(AckRef::KafkaOffset(token_2)),
            original_item: Some(WorkItem::Accepted {
                ack_ref: AckRef::KafkaOffset(token_2),
                envelope: envelope_2.clone(),
            }),
            frames: vec![WorldLogFrame {
                format_version: 1,
                universe_id,
                world_id,
                world_epoch,
                world_seq_start: 1,
                world_seq_end: 1,
                records: Vec::new(),
            }],
            disposition: None,
            opened_effects: Vec::new(),
            checkpoint: None,
            approx_bytes: 1,
        })
        .expect("stage second slice");
        {
            let world = core
                .state
                .active_worlds
                .get_mut(&world_id)
                .expect("active world");
            world.accepted_submission_ids.insert("sub-1".into());
            world.accepted_submission_ids.insert("sub-2".into());
        }

        core.handle_flush_failure(&[10])
            .expect("rollback failed flush");

        assert!(core.state.scheduler.staged_slices.is_empty());
        let queue = core
            .state
            .scheduler
            .pending_by_partition
            .get(&0)
            .expect("partition queue");
        assert!(
            queue
                .iter()
                .all(|entry| matches!(entry.state, PendingState::Received))
        );

        let world = core
            .state
            .active_worlds
            .get(&world_id)
            .expect("reopened world");
        assert!(world.pending_slices.is_empty());
        assert!(!world.commit_blocked);
        assert_eq!(world.pending_slice, None);
        assert!(!world.accepted_submission_ids.contains("sub-1"));
        assert!(!world.accepted_submission_ids.contains("sub-2"));
        assert_eq!(world.mailbox.len(), 2);
        assert!(matches!(
            world.mailbox.front(),
            Some(WorkItem::Accepted { envelope, .. }) if envelope.submission_id == "sub-1"
        ));
        assert!(matches!(
            world.mailbox.get(1),
            Some(WorkItem::Accepted { envelope, .. }) if envelope.submission_id == "sub-2"
        ));
    }
}
