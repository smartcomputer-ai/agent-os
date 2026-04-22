use std::time::Instant;

use aos_kernel::WorldControl;
use aos_kernel::WorldInput;
use aos_node::{
    EffectRuntimeEvent, HostControl, JournalBackend, SubmissionEnvelope, SubmissionPayload,
    SubmissionRejection, WorldId, WorldLogFrame, validate_create_world_request,
};

use super::core::{
    AcceptedSubmission, AckRef, CompletedSlice, DurableDisposition, KafkaOffsetAck, LocalInputMsg,
    PendingIngressEntry, PendingState, SchedulerMsg, WorkItem,
};
use super::types::{HostedStrictQuiescence, HostedWorkerCore, WorkerError};
use super::util::{build_timer_receipt, unix_time_ns};

impl HostedWorkerCore {
    pub(super) fn handle_scheduler_msg(&mut self, msg: SchedulerMsg) -> Result<bool, WorkerError> {
        match msg {
            SchedulerMsg::Accepted(submission) => self.handle_accepted_submission(submission),
            SchedulerMsg::LocalInput(local) => self.handle_local_input_msg(local),
            SchedulerMsg::FlushTick | SchedulerMsg::CheckpointTick | SchedulerMsg::Shutdown => {
                Ok(false)
            }
        }
    }

    fn handle_local_input_msg(&mut self, local: LocalInputMsg) -> Result<bool, WorkerError> {
        if let WorldInput::Receipt(receipt) = &local.input {
            self.clear_async_receipt_tracking(local.world_id, receipt.intent_hash);
        }
        if !self.owns_world(local.world_id) {
            return Ok(false);
        }
        if !self.state.active_worlds.contains_key(&local.world_id)
            && self.state.registered_worlds.contains_key(&local.world_id)
        {
            self.activate_world(local.world_id)?;
        }
        if self.state.active_worlds.contains_key(&local.world_id) {
            self.enqueue_local_input_back(local.world_id, local.input)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn owns_world(&self, world_id: WorldId) -> bool {
        self.state.owned_worlds.contains(&world_id)
    }

    pub(super) fn strict_quiescence_snapshot(
        &self,
        world_id: WorldId,
    ) -> Result<HostedStrictQuiescence, WorkerError> {
        let world = self
            .state
            .active_worlds
            .get(&world_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id: self.infra.default_universe_id,
                world_id,
            })?;
        let kernel = world.kernel.quiescence_status();
        let scheduled_timers = self
            .state
            .async_worlds
            .get(&world_id)
            .map(|state| state.scheduled_timers.len())
            .unwrap_or_default();

        Ok(HostedStrictQuiescence {
            non_terminal_workflow_instances: kernel.non_terminal_workflow_instances,
            inflight_workflow_intents: kernel.inflight_workflow_intents,
            pending_workflow_receipts: kernel.pending_workflow_receipts,
            queued_effects: kernel.queued_effects,
            workflow_queue_pending: kernel.workflow_queue_pending,
            mailbox_len: world.mailbox.len(),
            running: world.running,
            commit_blocked: world.commit_blocked,
            pending_slice: world.pending_slice.is_some(),
            scheduled_timers,
        })
    }

    pub(super) fn ensure_world_strict_quiescent_for_apply(
        &self,
        world_id: WorldId,
    ) -> Result<(), WorkerError> {
        let snapshot = self.strict_quiescence_snapshot(world_id)?;
        let blocked = snapshot.non_terminal_workflow_instances > 0
            || snapshot.inflight_workflow_intents > 0
            || snapshot.pending_workflow_receipts > 0
            || snapshot.queued_effects > 0
            || snapshot.workflow_queue_pending
            || snapshot.mailbox_len > 0
            || snapshot.commit_blocked
            || snapshot.pending_slice
            || snapshot.scheduled_timers > 0;
        if !blocked {
            return Ok(());
        }

        Err(WorkerError::StrictQuiescenceBlocked {
            world_id,
            operation: "manifest apply",
            non_terminal_workflow_instances: snapshot.non_terminal_workflow_instances,
            inflight_workflow_intents: snapshot.inflight_workflow_intents,
            pending_workflow_receipts: snapshot.pending_workflow_receipts,
            queued_effects: snapshot.queued_effects,
            workflow_queue_pending: snapshot.workflow_queue_pending,
            mailbox_len: snapshot.mailbox_len,
            running: snapshot.running,
            commit_blocked: snapshot.commit_blocked,
            pending_slice: snapshot.pending_slice,
            scheduled_timers: snapshot.scheduled_timers,
        })
    }

    pub(super) fn drive_scheduler_until_quiescent(
        &mut self,
        mut force_flush: bool,
        profile: &mut super::types::SupervisorRunProfile,
    ) -> Result<(usize, usize), WorkerError> {
        let mut committed_frames = 0usize;
        loop {
            let mut progressed = false;
            progressed |= self.drain_effect_events()?;
            progressed |= self.drain_due_timers()?;

            let service_started = Instant::now();
            progressed |= self.service_ready_worlds()?;
            profile.run_partitions += service_started.elapsed();

            let flush_started = Instant::now();
            if force_flush || self.flush_pressure_reached() {
                match self.flush_ready_batch() {
                    Ok(Some(outcome)) => {
                        committed_frames = committed_frames.saturating_add(
                            outcome
                                .committed_slices
                                .iter()
                                .map(|slice| slice.frames.len())
                                .sum::<usize>(),
                        );
                        self.finalize_flush_success(outcome)?;
                        progressed = true;
                    }
                    Ok(None) => {}
                    Err(err) => {
                        profile.partition_commit_batch += flush_started.elapsed();
                        return Err(err);
                    }
                }
            }
            profile.partition_commit_batch += flush_started.elapsed();
            force_flush = false;

            if !progressed {
                break;
            }
        }

        Ok((committed_frames, 0))
    }

    fn flush_pressure_reached(&self) -> bool {
        if self.state.scheduler.staged_slices.is_empty() {
            return false;
        }
        if self.any_world_waiting_on_commit_capacity() {
            return true;
        }
        if self.flush_limits.max_slices > 0
            && self.state.scheduler.staged_slices.len() >= self.flush_limits.max_slices
        {
            return true;
        }
        if self.flush_limits.max_bytes > 0 {
            let staged_bytes = self
                .state
                .scheduler
                .staged_slices
                .values()
                .map(|slice| slice.approx_bytes)
                .sum::<usize>();
            if staged_bytes >= self.flush_limits.max_bytes {
                return true;
            }
        }
        if self.flush_limits.max_delay.is_zero() {
            return true;
        }
        self.state
            .scheduler
            .staged_slices
            .values()
            .map(|slice| slice.staged_at)
            .min()
            .is_some_and(|staged_at| staged_at.elapsed() >= self.flush_limits.max_delay)
    }

    fn any_world_waiting_on_commit_capacity(&self) -> bool {
        self.state.active_worlds.iter().any(|(world_id, world)| {
            !world.pending_slices.is_empty()
                && !world.mailbox.is_empty()
                && !self.world_has_staging_capacity(*world_id)
        })
    }

    fn drain_effect_events(&mut self) -> Result<bool, WorkerError> {
        let mut progressed = false;
        if let Some(rx) = self.effect_event_rx.as_mut() {
            let mut events = Vec::new();
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
            for event in events {
                match event {
                    EffectRuntimeEvent::WorldInput { world_id, input } => {
                        if !self.owns_world(world_id) {
                            continue;
                        }
                        if !self.state.active_worlds.contains_key(&world_id)
                            && self.state.registered_worlds.contains_key(&world_id)
                        {
                            self.activate_world(world_id)?;
                        }
                        if self.state.active_worlds.contains_key(&world_id) {
                            self.enqueue_local_input_back(world_id, input)?;
                            progressed = true;
                        }
                    }
                }
            }
        }
        Ok(progressed)
    }

    fn drain_due_timers(&mut self) -> Result<bool, WorkerError> {
        if self.scheduler_tx.is_some() {
            return Ok(false);
        }
        let now_ns = unix_time_ns();
        let world_ids = self.state.async_worlds.keys().copied().collect::<Vec<_>>();
        let mut due = Vec::new();
        for world_id in world_ids {
            let Some(async_state) = self.state.async_worlds.get_mut(&world_id) else {
                continue;
            };
            for entry in async_state.timer_scheduler.pop_due(now_ns) {
                async_state.scheduled_timers.remove(&entry.intent_hash);
                due.push((world_id, build_timer_receipt(&entry)?));
            }
        }
        let progressed = !due.is_empty();
        for (world_id, receipt) in due {
            self.enqueue_local_input_back(world_id, WorldInput::Receipt(receipt))?;
        }
        Ok(progressed)
    }

    fn clear_async_receipt_tracking(&mut self, world_id: WorldId, intent_hash: [u8; 32]) {
        if let Some(async_state) = self.state.async_worlds.get_mut(&world_id) {
            async_state.scheduled_timers.remove(&intent_hash);
            if let Some(handle) = async_state.timer_tasks.remove(&intent_hash) {
                handle.abort();
            }
        }
    }

    pub(super) fn handle_accepted_submission(
        &mut self,
        submission: AcceptedSubmission,
    ) -> Result<bool, WorkerError> {
        if let AckRef::KafkaOffset(ack) = submission.ack_ref {
            self.state
                .scheduler
                .pending_by_partition
                .entry(ack.partition)
                .or_default()
                .push_back(PendingIngressEntry {
                    ack,
                    envelope: submission.envelope.clone(),
                    state: PendingState::Received,
                });
        }
        self.route_accepted_submission(submission.ack_ref, submission.envelope)?;
        Ok(true)
    }

    pub(super) fn route_accepted_submission(
        &mut self,
        ack_ref: AckRef,
        envelope: SubmissionEnvelope,
    ) -> Result<(), WorkerError> {
        let world_id = envelope.world_id;
        let universe_id = envelope.universe_id;
        if let SubmissionPayload::HostControl { control } = &envelope.payload {
            let control = control.clone();
            return self.route_host_control_submission(ack_ref, envelope, control);
        }
        if !self.owns_world(world_id) {
            let slice = self.rejected_ingress_slice(
                WorkItem::Accepted { ack_ref, envelope },
                world_id,
                SubmissionRejection::UnknownWorld,
            )?;
            self.stage_completed_slice(slice)?;
            return Ok(());
        }

        if self.ensure_registered_world(universe_id, world_id).is_err() {
            let slice = self.rejected_ingress_slice(
                WorkItem::Accepted { ack_ref, envelope },
                world_id,
                SubmissionRejection::UnknownWorld,
            )?;
            self.stage_completed_slice(slice)?;
            return Ok(());
        }

        let expected_epoch = self
            .state
            .registered_worlds
            .get(&world_id)
            .map(|world| world.world_epoch)
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        if envelope.world_epoch != expected_epoch {
            let got = envelope.world_epoch;
            let slice = self.rejected_ingress_slice(
                WorkItem::Accepted { ack_ref, envelope },
                world_id,
                SubmissionRejection::WorldEpochMismatch {
                    expected: expected_epoch,
                    got,
                },
            )?;
            self.stage_completed_slice(slice)?;
            return Ok(());
        }

        self.activate_world(world_id)?;
        if let Some(reason) = self
            .state
            .active_worlds
            .get(&world_id)
            .and_then(|world| world.disabled_reason.clone())
        {
            let slice = self.rejected_ingress_slice(
                WorkItem::Accepted { ack_ref, envelope },
                world_id,
                SubmissionRejection::InvalidSubmission {
                    message: format!("world {world_id} is disabled: {reason}"),
                },
            )?;
            self.stage_completed_slice(slice)?;
            return Ok(());
        }

        self.enqueue_accepted_item(world_id, WorkItem::Accepted { ack_ref, envelope })
    }

    fn route_host_control_submission(
        &mut self,
        ack_ref: AckRef,
        envelope: SubmissionEnvelope,
        control: HostControl,
    ) -> Result<(), WorkerError> {
        let world_id = envelope.world_id;
        let universe_id = envelope.universe_id;
        let submission_id = envelope.submission_id.clone();
        match control {
            HostControl::CreateWorld { mut request } => {
                if request
                    .world_id
                    .is_some_and(|request_world_id| request_world_id != world_id)
                {
                    let slice = self.rejected_ingress_slice(
                        WorkItem::Accepted { ack_ref, envelope },
                        world_id,
                        SubmissionRejection::InvalidSubmission {
                            message:
                                "host control create_world world_id must match envelope world_id"
                                    .into(),
                        },
                    )?;
                    self.stage_completed_slice(slice)?;
                    return Ok(());
                }
                request.world_id = Some(world_id);
                if let Err(err) = validate_create_world_request(&request) {
                    let slice = self.rejected_ingress_slice(
                        WorkItem::Accepted { ack_ref, envelope },
                        world_id,
                        SubmissionRejection::InvalidSubmission {
                            message: err.to_string(),
                        },
                    )?;
                    self.stage_completed_slice(slice)?;
                    return Ok(());
                }

                if self.state.pending_created_worlds.contains_key(&world_id)
                    || self.state.registered_worlds.contains_key(&world_id)
                    || self.state.active_worlds.contains_key(&world_id)
                {
                    let slice = self.rejected_ingress_slice(
                        WorkItem::Accepted { ack_ref, envelope },
                        world_id,
                        SubmissionRejection::WorldAlreadyExists,
                    )?;
                    self.stage_completed_slice(slice)?;
                    return Ok(());
                }

                match self.ensure_registered_world(universe_id, world_id) {
                    Ok(()) => {
                        let slice = self.rejected_ingress_slice(
                            WorkItem::Accepted { ack_ref, envelope },
                            world_id,
                            SubmissionRejection::WorldAlreadyExists,
                        )?;
                        self.stage_completed_slice(slice)?;
                        return Ok(());
                    }
                    Err(WorkerError::UnknownWorld { .. }) => {}
                    Err(err) => return Err(err),
                }
                self.state.owned_worlds.insert(world_id);

                let frame = match self.prepare_pending_created_world(
                    universe_id,
                    world_id,
                    &submission_id,
                    request,
                ) {
                    Ok(frame) => frame,
                    Err(err) => {
                        let slice = self.rejected_ingress_slice(
                            WorkItem::Accepted { ack_ref, envelope },
                            world_id,
                            SubmissionRejection::InvalidSubmission {
                                message: err.to_string(),
                            },
                        )?;
                        self.stage_completed_slice(slice)?;
                        return Ok(());
                    }
                };

                let Some(frame) = frame else {
                    self.state.pending_created_worlds.remove(&world_id);
                    if let Some(mut async_state) = self.state.async_worlds.remove(&world_id) {
                        async_state.abort_all_timers();
                    }
                    self.state.active_worlds.remove(&world_id);
                    self.state.registered_worlds.remove(&world_id);
                    let slice = self.rejected_ingress_slice(
                        WorkItem::Accepted { ack_ref, envelope },
                        world_id,
                        SubmissionRejection::InvalidSubmission {
                            message:
                                "host control create_world produced no durable bootstrap frame"
                                    .into(),
                        },
                    )?;
                    self.stage_completed_slice(slice)?;
                    return Ok(());
                };

                let approx_bytes = serde_cbor::to_vec(&frame)?.len();
                let slice_id = self.next_slice_id();
                self.stage_completed_slice(CompletedSlice {
                    id: slice_id,
                    world_id,
                    affected_worlds: vec![world_id],
                    staged_at: Instant::now(),
                    ack_ref: Some(ack_ref),
                    original_item: Some(WorkItem::Accepted { ack_ref, envelope }),
                    frames: vec![frame],
                    disposition: None,
                    opened_effects: Vec::new(),
                    checkpoint: None,
                    approx_bytes,
                })?;
                Ok(())
            }
        }
    }

    fn enqueue_accepted_item(
        &mut self,
        world_id: WorldId,
        item: WorkItem,
    ) -> Result<(), WorkerError> {
        let slot =
            self.state
                .active_worlds
                .get_mut(&world_id)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id: self.infra.default_universe_id,
                    world_id,
                })?;
        slot.mailbox.push_back(item);
        self.mark_world_ready(world_id);
        Ok(())
    }

    fn enqueue_local_input_back(
        &mut self,
        world_id: WorldId,
        input: WorldInput,
    ) -> Result<(), WorkerError> {
        let slot =
            self.state
                .active_worlds
                .get_mut(&world_id)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id: self.infra.default_universe_id,
                    world_id,
                })?;
        slot.mailbox.push_back(WorkItem::LocalInput(input));
        self.mark_world_ready(world_id);
        Ok(())
    }

    pub(super) fn enqueue_local_input_front(
        &mut self,
        world_id: WorldId,
        input: WorldInput,
    ) -> Result<(), WorkerError> {
        let slot =
            self.state
                .active_worlds
                .get_mut(&world_id)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id: self.infra.default_universe_id,
                    world_id,
                })?;
        slot.mailbox.push_front(WorkItem::LocalInput(input));
        self.mark_world_ready(world_id);
        Ok(())
    }

    pub(super) fn mark_world_ready(&mut self, world_id: WorldId) {
        let should_ready = self.state.active_worlds.get(&world_id).is_some_and(|slot| {
            !slot.ready
                && !slot.running
                && !slot.mailbox.is_empty()
                && self.world_has_staging_capacity(world_id)
        });
        if !should_ready {
            return;
        }
        let Some(slot) = self.state.active_worlds.get_mut(&world_id) else {
            return;
        };
        slot.ready = true;
        self.state.ready_worlds.push_back(world_id);
    }

    fn world_is_serviceable(&self, world_id: WorldId) -> bool {
        self.state.active_worlds.get(&world_id).is_some_and(|slot| {
            slot.disabled_reason.is_none()
                && !slot.running
                && !slot.mailbox.is_empty()
                && self.world_has_staging_capacity(world_id)
        })
    }

    fn world_has_staging_capacity(&self, world_id: WorldId) -> bool {
        let Some(slot) = self.state.active_worlds.get(&world_id) else {
            return false;
        };
        if slot.pending_slices.len() >= self.max_uncommitted_slices_per_world {
            return false;
        }
        if slot
            .pending_slices
            .iter()
            .filter_map(|slice_id| self.state.scheduler.staged_slices.get(slice_id))
            .any(Self::slice_is_barrier)
        {
            return false;
        }
        if !slot.pending_slices.is_empty()
            && Self::front_mailbox_item_requires_strict_quiescence(slot)
        {
            return false;
        }
        true
    }

    fn front_mailbox_item_requires_strict_quiescence(world: &super::types::ActiveWorld) -> bool {
        matches!(
            world.mailbox.front(),
            Some(WorkItem::Accepted { envelope, .. })
                if matches!(
                    envelope.payload,
                    SubmissionPayload::WorldControl {
                        control: WorldControl::ApplyProposal { .. }
                    } | SubmissionPayload::WorldControl {
                        control: WorldControl::ApplyPatchDirect { .. }
                    }
                )
        )
    }

    fn service_ready_worlds(&mut self) -> Result<bool, WorkerError> {
        let mut progressed = false;
        while let Some(world_id) = self.state.ready_worlds.pop_front() {
            if let Some(slot) = self.state.active_worlds.get_mut(&world_id) {
                slot.ready = false;
            }
            if !self.world_is_serviceable(world_id) {
                continue;
            }
            self.service_one_world(world_id)?;
            progressed = true;
        }
        Ok(progressed)
    }

    fn service_one_world(&mut self, world_id: WorldId) -> Result<(), WorkerError> {
        let item =
            {
                let slot = self.state.active_worlds.get_mut(&world_id).ok_or(
                    WorkerError::UnknownWorld {
                        universe_id: self.infra.default_universe_id,
                        world_id,
                    },
                )?;
                slot.running = true;
                slot.mailbox.pop_front().ok_or_else(|| {
                    WorkerError::Persist(aos_node::PersistError::backend(format!(
                        "world {world_id} mailbox unexpectedly empty"
                    )))
                })?
            };

        let accepted_submission_id = if let WorkItem::Accepted { envelope, .. } = &item {
            let submission_id = envelope.submission_id.clone();
            let already_seen = {
                let world = self.state.active_worlds.get_mut(&world_id).ok_or(
                    WorkerError::UnknownWorld {
                        universe_id: self.infra.default_universe_id,
                        world_id,
                    },
                )?;
                !world.accepted_submission_ids.insert(submission_id.clone())
            };
            if already_seen {
                let slice = self.rejected_ingress_slice(
                    item,
                    world_id,
                    SubmissionRejection::DuplicateSubmissionId,
                )?;
                self.stage_completed_slice(slice)?;
                return Ok(());
            }
            Some(submission_id)
        } else {
            None
        };

        let tail_start = self
            .state
            .active_worlds
            .get(&world_id)
            .map(|world| world.kernel.journal_head())
            .ok_or(WorkerError::UnknownWorld {
                universe_id: self.infra.default_universe_id,
                world_id,
            })?;

        let service_result: Result<(), WorkerError> = match &item {
            WorkItem::Accepted { envelope, .. } => match &envelope.payload {
                SubmissionPayload::HostControl { .. } => {
                    Err(WorkerError::Persist(aos_node::PersistError::backend(
                        "host control must be handled outside world admission".to_owned(),
                    )))
                }
                SubmissionPayload::WorldInput { input } => {
                    let world = self.state.active_worlds.get_mut(&world_id).ok_or(
                        WorkerError::UnknownWorld {
                            universe_id: self.infra.default_universe_id,
                            world_id,
                        },
                    )?;
                    world
                        .kernel
                        .accept(input.clone())
                        .map_err(WorkerError::Kernel)
                }
                SubmissionPayload::WorldControl { control } => {
                    if matches!(
                        control,
                        aos_kernel::WorldControl::ApplyProposal { .. }
                            | aos_kernel::WorldControl::ApplyPatchDirect { .. }
                    ) {
                        self.ensure_world_strict_quiescent_for_apply(world_id)?;
                    }
                    let world = self.state.active_worlds.get_mut(&world_id).ok_or(
                        WorkerError::UnknownWorld {
                            universe_id: self.infra.default_universe_id,
                            world_id,
                        },
                    )?;
                    let _ = world
                        .kernel
                        .apply_control(control.clone())
                        .map_err(WorkerError::Kernel)?;
                    Ok(())
                }
            },
            WorkItem::LocalInput(input) => {
                let world = self.state.active_worlds.get_mut(&world_id).ok_or(
                    WorkerError::UnknownWorld {
                        universe_id: self.infra.default_universe_id,
                        world_id,
                    },
                )?;
                world
                    .kernel
                    .accept(input.clone())
                    .map_err(WorkerError::Kernel)
            }
        };

        match service_result {
            Ok(()) => {
                let drain = {
                    let world = self.state.active_worlds.get_mut(&world_id).ok_or(
                        WorkerError::UnknownWorld {
                            universe_id: self.infra.default_universe_id,
                            world_id,
                        },
                    )?;
                    world.kernel.drain_until_idle_from(tail_start)?
                };
                let slice = self.build_completed_slice(world_id, item, drain)?;
                if let Some(slice) = slice {
                    if slice.frames.is_empty()
                        && let Some(submission_id) = accepted_submission_id.as_deref()
                        && let Some(world) = self.state.active_worlds.get_mut(&world_id)
                    {
                        world.accepted_submission_ids.remove(submission_id);
                    }
                    self.stage_completed_slice(slice)?;
                } else {
                    let slot = self.state.active_worlds.get_mut(&world_id).ok_or(
                        WorkerError::UnknownWorld {
                            universe_id: self.infra.default_universe_id,
                            world_id,
                        },
                    )?;
                    slot.running = false;
                    if !slot.mailbox.is_empty() {
                        self.mark_world_ready(world_id);
                    }
                }
            }
            Err(err) => {
                if let Some(submission_id) = accepted_submission_id.as_deref()
                    && let Some(world) = self.state.active_worlds.get_mut(&world_id)
                {
                    world.accepted_submission_ids.remove(submission_id);
                }
                if let Some(slice) = self.failure_slice(world_id, item, &err)? {
                    self.stage_completed_slice(slice)?;
                } else {
                    self.disable_world(world_id, err.to_string());
                    return Err(err);
                }
            }
        }

        Ok(())
    }

    fn build_completed_slice(
        &mut self,
        world_id: WorldId,
        item: WorkItem,
        drain: aos_kernel::KernelDrain,
    ) -> Result<Option<CompletedSlice>, WorkerError> {
        let durable_next_world_seq = JournalBackend::durable_head(&self.infra.journal, world_id)
            .map_err(WorkerError::LogFirst)?
            .next_world_seq;
        let frames = {
            let world =
                self.state
                    .active_worlds
                    .get(&world_id)
                    .ok_or(WorkerError::UnknownWorld {
                        universe_id: self.infra.default_universe_id,
                        world_id,
                    })?;
            if drain.tail.entries.is_empty() {
                Vec::new()
            } else {
                let world_seq_start = world.next_world_seq.max(durable_next_world_seq);
                vec![WorldLogFrame {
                    format_version: 1,
                    universe_id: world.universe_id,
                    world_id,
                    world_epoch: world.world_epoch,
                    world_seq_start,
                    world_seq_end: world_seq_start + drain.tail.entries.len() as u64 - 1,
                    records: drain
                        .tail
                        .entries
                        .iter()
                        .map(|entry| entry.record.clone())
                        .collect(),
                }]
            }
        };

        if frames.is_empty() && !matches!(item, WorkItem::Accepted { .. }) {
            return Ok(None);
        }

        let disposition = match (&item, frames.is_empty()) {
            (WorkItem::Accepted { ack_ref, envelope }, true) => {
                if let Some(command) = envelope.command.as_ref() {
                    Some(DurableDisposition::CommandFailure {
                        ack_ref: Some(*ack_ref),
                        world_id,
                        command_id: command.command_id.clone(),
                        error_code: "no_journal_effect".into(),
                    })
                } else {
                    Some(DurableDisposition::RejectedSubmission {
                        ack_ref: Some(*ack_ref),
                        world_id,
                        reason: SubmissionRejection::InvalidSubmission {
                            message: "ingress item produced no journal frame".into(),
                        },
                    })
                }
            }
            _ => None,
        };

        let approx_bytes = frames
            .iter()
            .map(|frame| serde_cbor::to_vec(frame).map(|bytes| bytes.len()))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .sum::<usize>()
            + disposition
                .as_ref()
                .map(|disposition| serde_cbor::to_vec(disposition).map(|bytes| bytes.len()))
                .transpose()?
                .unwrap_or_default()
            + usize::from(frames.is_empty() && disposition.is_none());

        Ok(Some(CompletedSlice {
            id: self.next_slice_id(),
            world_id,
            affected_worlds: vec![world_id],
            staged_at: Instant::now(),
            ack_ref: match &item {
                WorkItem::Accepted { ack_ref, .. } => Some(*ack_ref),
                WorkItem::LocalInput(_) => None,
            },
            original_item: Some(item),
            frames,
            disposition,
            opened_effects: drain.opened_effects,
            checkpoint: None,
            approx_bytes,
        }))
    }

    fn failure_slice(
        &mut self,
        world_id: WorldId,
        item: WorkItem,
        err: &WorkerError,
    ) -> Result<Option<CompletedSlice>, WorkerError> {
        let WorkItem::Accepted { ack_ref, envelope } = &item else {
            return Ok(None);
        };
        let disposition = if let Some(command) = envelope.command.as_ref() {
            DurableDisposition::CommandFailure {
                ack_ref: Some(*ack_ref),
                world_id,
                command_id: command.command_id.clone(),
                error_code: "command_failed".into(),
            }
        } else {
            DurableDisposition::RejectedSubmission {
                ack_ref: Some(*ack_ref),
                world_id,
                reason: SubmissionRejection::InvalidSubmission {
                    message: err.to_string(),
                },
            }
        };
        let approx_bytes = serde_cbor::to_vec(&disposition)?.len();
        Ok(Some(CompletedSlice {
            id: self.next_slice_id(),
            world_id,
            affected_worlds: vec![world_id],
            staged_at: Instant::now(),
            ack_ref: Some(*ack_ref),
            original_item: Some(item),
            frames: Vec::new(),
            disposition: Some(disposition),
            opened_effects: Vec::new(),
            checkpoint: None,
            approx_bytes,
        }))
    }

    fn rejected_ingress_slice(
        &mut self,
        item: WorkItem,
        world_id: WorldId,
        reason: SubmissionRejection,
    ) -> Result<CompletedSlice, WorkerError> {
        let token = match &item {
            WorkItem::Accepted { ack_ref, .. } => Some(*ack_ref),
            WorkItem::LocalInput(_) => {
                return Err(WorkerError::Persist(aos_node::PersistError::backend(
                    "local input cannot be staged as rejected ingress",
                )));
            }
        };
        let disposition = DurableDisposition::RejectedSubmission {
            ack_ref: token,
            world_id,
            reason,
        };
        Ok(CompletedSlice {
            id: self.next_slice_id(),
            world_id,
            affected_worlds: vec![world_id],
            staged_at: Instant::now(),
            ack_ref: token,
            original_item: Some(item),
            frames: Vec::new(),
            disposition: Some(disposition.clone()),
            opened_effects: Vec::new(),
            checkpoint: None,
            approx_bytes: serde_cbor::to_vec(&disposition)?.len(),
        })
    }

    pub(super) fn stage_completed_slice(
        &mut self,
        slice: CompletedSlice,
    ) -> Result<(), WorkerError> {
        let affected_worlds = slice.affected_worlds.clone();
        if let Some(ack) = slice.ack_ref.and_then(Self::kafka_offset_ack) {
            self.mark_pending_serviced(ack, slice.id)?;
        }
        for world_id in &slice.affected_worlds {
            if let Some(world) = self.state.active_worlds.get_mut(world_id) {
                world.running = false;
                world.pending_slices.push_back(slice.id);
                if let Some(next_world_seq) =
                    Self::reserved_next_world_seq_for_slice(&slice, *world_id)
                {
                    world.next_world_seq = world.next_world_seq.max(next_world_seq);
                }
                Self::sync_pending_slice_flags(world);
            }
        }
        self.state.scheduler.stage_slice(slice);
        for world_id in affected_worlds {
            self.mark_world_ready(world_id);
        }
        Ok(())
    }

    fn reserved_next_world_seq_for_slice(slice: &CompletedSlice, world_id: WorldId) -> Option<u64> {
        if let Some(frame) = slice.frames.iter().find(|frame| frame.world_id == world_id) {
            return Some(frame.world_seq_end.saturating_add(1));
        }
        slice.checkpoint.as_ref().and_then(|checkpoint| {
            checkpoint
                .worlds
                .iter()
                .find(|world| world.world_id == world_id)
                .map(|world| world.world_seq.saturating_add(1))
        })
    }

    pub(super) fn remove_pending_slice_id(world: &mut super::types::ActiveWorld, slice_id: u64) {
        if let Some(front) = world.pending_slices.front().copied()
            && front == slice_id
        {
            world.pending_slices.pop_front();
            Self::sync_pending_slice_flags(world);
            return;
        }
        if let Some(position) = world
            .pending_slices
            .iter()
            .position(|pending| *pending == slice_id)
        {
            let _ = world.pending_slices.remove(position);
        }
        Self::sync_pending_slice_flags(world);
    }

    pub(super) fn sync_pending_slice_flags(world: &mut super::types::ActiveWorld) {
        world.commit_blocked = !world.pending_slices.is_empty();
        world.pending_slice = world.pending_slices.front().copied();
    }

    fn slice_is_barrier(slice: &CompletedSlice) -> bool {
        if slice.checkpoint.is_some() {
            return true;
        }
        if !slice.opened_effects.is_empty() {
            return true;
        }
        match slice.original_item.as_ref() {
            Some(WorkItem::Accepted { envelope, .. }) => match &envelope.payload {
                SubmissionPayload::HostControl { .. } => true,
                SubmissionPayload::WorldControl { .. } => true,
                _ => false,
            },
            _ => false,
        }
    }

    fn mark_pending_serviced(
        &mut self,
        ack: KafkaOffsetAck,
        slice_id: u64,
    ) -> Result<(), WorkerError> {
        let queue = self
            .state
            .scheduler
            .pending_by_partition
            .get_mut(&ack.partition)
            .ok_or_else(|| {
                WorkerError::Persist(aos_node::PersistError::backend(format!(
                    "missing pending partition {}",
                    ack.partition
                )))
            })?;
        let entry = queue
            .iter_mut()
            .find(|entry| entry.ack == ack)
            .ok_or_else(|| {
                WorkerError::Persist(aos_node::PersistError::backend(format!(
                    "missing pending ingress offset {}:{}",
                    ack.partition, ack.offset
                )))
            })?;
        entry.state = PendingState::Serviced(slice_id);
        Ok(())
    }

    fn kafka_offset_ack(ack_ref: AckRef) -> Option<KafkaOffsetAck> {
        match ack_ref {
            AckRef::KafkaOffset(ack) => Some(ack),
            AckRef::DirectAccept { .. } => None,
        }
    }

    pub(super) fn next_slice_id(&mut self) -> u64 {
        self.state.next_slice_id = self.state.next_slice_id.saturating_add(1);
        self.state.next_slice_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use aos_air_types::{CURRENT_AIR_VERSION, Manifest};
    use aos_cbor::to_canonical_cbor;
    use aos_node::{CreateWorldRequest, CreateWorldSource, UniverseId};

    use crate::worker::HostedWorkerRuntime;
    use crate::worker::core::{AcceptedSubmission, CheckpointCommit, KafkaOffsetAck};

    fn runtime_with_empty_world() -> (HostedWorkerRuntime, UniverseId, WorldId, u64) {
        let runtime = HostedWorkerRuntime::new_embedded_kafka(1).expect("embedded runtime");
        let universe_id = runtime.default_universe_id().expect("default universe");
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: Vec::new(),
            modules: Vec::new(),
            ops: Vec::new(),
            secrets: Vec::new(),
            routing: None,
        };
        let manifest_bytes = to_canonical_cbor(&manifest).expect("encode manifest");
        let manifest_hash = runtime
            .put_blob(universe_id, &manifest_bytes)
            .expect("store manifest")
            .to_hex();
        let accepted = runtime
            .create_world(
                universe_id,
                CreateWorldRequest {
                    world_id: None,
                    universe_id,
                    created_at_ns: 1,
                    source: CreateWorldSource::Manifest { manifest_hash },
                },
            )
            .expect("create world");
        let summary = runtime
            .get_world(universe_id, accepted.world_id)
            .expect("world summary");
        (runtime, universe_id, accepted.world_id, summary.world_epoch)
    }

    fn enqueue_ingress(
        core: &mut HostedWorkerCore,
        partition: u32,
        offset: i64,
        universe_id: UniverseId,
        world_id: WorldId,
        world_epoch: u64,
        submission_id: &str,
    ) {
        core.handle_accepted_submission(AcceptedSubmission {
            ack_ref: AckRef::KafkaOffset(KafkaOffsetAck { partition, offset }),
            envelope: SubmissionEnvelope {
                submission_id: submission_id.to_owned(),
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
            },
        })
        .expect("enqueue ingress");
    }

    #[test]
    fn service_ready_worlds_can_stage_multiple_uncommitted_slices_for_one_world() {
        let (runtime, universe_id, world_id, world_epoch) = runtime_with_empty_world();
        runtime
            .set_max_uncommitted_slices_per_world(2)
            .expect("set pipeline depth");
        let mut core = runtime.lock_core().expect("lock core");
        enqueue_ingress(&mut core, 0, 1, universe_id, world_id, world_epoch, "sub-1");
        enqueue_ingress(&mut core, 0, 2, universe_id, world_id, world_epoch, "sub-2");
        enqueue_ingress(&mut core, 0, 3, universe_id, world_id, world_epoch, "sub-3");

        assert!(core.service_ready_worlds().expect("service ready worlds"));
        let world = core
            .state
            .active_worlds
            .get(&world_id)
            .expect("active world");
        assert_eq!(world.pending_slices.len(), 2);
        assert_eq!(world.mailbox.len(), 1);
        assert_eq!(core.state.scheduler.staged_slices.len(), 2);
    }

    #[test]
    fn barrier_slice_prevents_more_same_world_staging() {
        let (runtime, universe_id, world_id, world_epoch) = runtime_with_empty_world();
        runtime
            .set_max_uncommitted_slices_per_world(4)
            .expect("set pipeline depth");
        let mut core = runtime.lock_core().expect("lock core");
        let slice_id = core.next_slice_id();
        core.stage_completed_slice(CompletedSlice {
            id: slice_id,
            world_id,
            affected_worlds: vec![world_id],
            staged_at: Instant::now(),
            ack_ref: None,
            original_item: None,
            frames: Vec::new(),
            disposition: None,
            opened_effects: Vec::new(),
            checkpoint: Some(CheckpointCommit {
                created_at_ns: 0,
                trigger: "test",
                worlds: Vec::new(),
            }),
            approx_bytes: 1,
        })
        .expect("stage barrier slice");
        enqueue_ingress(&mut core, 0, 1, universe_id, world_id, world_epoch, "sub-1");

        assert!(!core.service_ready_worlds().expect("service ready worlds"));
        let world = core
            .state
            .active_worlds
            .get(&world_id)
            .expect("active world");
        assert_eq!(world.pending_slices.len(), 1);
        assert_eq!(world.mailbox.len(), 1);
        assert_eq!(core.state.scheduler.staged_slices.len(), 1);
    }

    #[test]
    fn unowned_world_submission_is_rejected_before_activation() {
        let (runtime, universe_id, world_id, world_epoch) = runtime_with_empty_world();
        runtime
            .configure_owned_worlds(std::iter::empty())
            .expect("clear owned worlds");
        let mut core = runtime.lock_core().expect("lock core");

        enqueue_ingress(&mut core, 0, 1, universe_id, world_id, world_epoch, "sub-1");

        let staged = core
            .state
            .scheduler
            .staged_slices
            .values()
            .next()
            .expect("staged rejection");
        assert!(matches!(
            staged.disposition,
            Some(DurableDisposition::RejectedSubmission {
                reason: SubmissionRejection::UnknownWorld,
                ..
            })
        ));
        assert!(!core.state.active_worlds.contains_key(&world_id));
    }

    #[test]
    fn flush_pressure_reached_when_oldest_staged_slice_exceeds_max_delay() {
        let (runtime, _universe_id, world_id, _world_epoch) = runtime_with_empty_world();
        let mut core = runtime.lock_core().expect("lock core");
        core.flush_limits.max_delay = std::time::Duration::from_millis(50);
        core.state.scheduler.stage_slice(CompletedSlice {
            id: 1,
            world_id,
            affected_worlds: vec![world_id],
            staged_at: Instant::now(),
            ack_ref: Some(AckRef::KafkaOffset(KafkaOffsetAck {
                partition: 0,
                offset: 1,
            })),
            original_item: None,
            frames: Vec::new(),
            disposition: None,
            opened_effects: Vec::new(),
            checkpoint: None,
            approx_bytes: 1,
        });

        assert!(!core.flush_pressure_reached());

        core.state
            .scheduler
            .staged_slices
            .get_mut(&1)
            .expect("staged slice")
            .staged_at = Instant::now() - std::time::Duration::from_millis(75);
        assert!(core.flush_pressure_reached());
    }

    #[test]
    fn flush_pressure_reached_when_world_is_waiting_on_commit_capacity() {
        let (runtime, universe_id, world_id, world_epoch) = runtime_with_empty_world();
        runtime
            .set_max_uncommitted_slices_per_world(1)
            .expect("set pipeline depth");
        let mut core = runtime.lock_core().expect("lock core");
        core.flush_limits.max_delay = std::time::Duration::from_secs(60);
        enqueue_ingress(&mut core, 0, 1, universe_id, world_id, world_epoch, "sub-1");
        enqueue_ingress(&mut core, 0, 2, universe_id, world_id, world_epoch, "sub-2");
        assert!(core.service_ready_worlds().expect("service ready worlds"));

        assert!(core.flush_pressure_reached());
    }
}
