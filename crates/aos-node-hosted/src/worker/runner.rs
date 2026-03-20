use super::*;

impl<P> WorldRunner<P>
where
    P: HostedRuntimeStore + SecretStore + 'static,
{
    fn snapshot_hosted(&mut self) -> Result<(), WorkerError> {
        let persistence: Arc<dyn WorldStore> = self.runtime.clone();
        self.host.snapshot(persistence, self.universe, self.world)?;
        Ok(())
    }

    fn quiescent_for_reconciliation(&self) -> Result<bool, WorkerError> {
        let now_ns = now_wallclock_ns();
        let runtime = self
            .runtime
            .world_runtime_info(self.universe, self.world, now_ns)?;
        Ok(!runtime.has_pending_inbox
            && !runtime.has_pending_effects
            && !self.host.has_pending_effects()
            && !runtime
                .next_timer_due_at_ns
                .is_some_and(|deliver_at_ns| deliver_at_ns <= now_ns))
    }

    fn reconcile_workflow_runtime_waits_if_quiescent(
        &mut self,
    ) -> Result<(usize, usize), WorkerError> {
        if !self.quiescent_for_reconciliation()? {
            return Ok((0, 0));
        }
        self.reconcile_workflow_runtime_waits()
    }

    pub(super) fn reconcile_workflow_runtime_waits(
        &mut self,
    ) -> Result<(usize, usize), WorkerError> {
        let now_ns = now_wallclock_ns();
        let mut valid_intents: HashSet<[u8; 32]> = self
            .runtime
            .outstanding_intent_hashes_for_world(self.universe, self.world, now_ns)?
            .into_iter()
            .collect();
        valid_intents.extend(
            self.host
                .kernel()
                .queued_effects_snapshot()
                .into_iter()
                .map(|intent| intent.intent_hash),
        );
        let valid_receipt_intents: HashSet<[u8; 32]> = self
            .host
            .kernel()
            .pending_workflow_receipts_snapshot()
            .into_iter()
            .map(|pending| pending.intent_hash)
            .collect();
        let dropped_dispatches = self.runtime.retain_effect_dispatches_for_world(
            self.universe,
            self.world,
            &valid_receipt_intents,
            now_ns,
        )?;
        if dropped_dispatches > 0 {
            tracing::warn!(
                worker_id = %self.worker.config.worker_id,
                universe_id = %self.universe,
                world_id = %self.world,
                dropped_dispatches,
                "pruned orphan hosted effect dispatches with no live workflow receipt context"
            );
        }
        Ok(self
            .host
            .kernel_mut()
            .retain_workflow_runtime_waits(&valid_intents))
    }

    fn persist_healed_snapshot_after_reconciliation(
        &mut self,
        dropped_receipts: usize,
        dropped_intents: usize,
    ) -> Result<bool, WorkerError> {
        if dropped_receipts == 0 && dropped_intents == 0 {
            return Ok(false);
        }
        let now_ns = now_wallclock_ns();
        let runtime = self
            .runtime
            .world_runtime_info(self.universe, self.world, now_ns)?;
        if runtime.has_pending_inbox
            || runtime.has_pending_effects
            || self.host.has_pending_effects()
        {
            return Ok(false);
        }
        self.snapshot_hosted()?;
        tracing::info!(
            worker_id = %self.worker.config.worker_id,
            universe_id = %self.universe,
            world_id = %self.world,
            dropped_receipts,
            dropped_intents,
            snapshot_height_after = ?self.host.kernel().get_journal_head().active_baseline_height,
            "persisted healed hosted snapshot after pruning stale workflow runtime waits"
        );
        Ok(true)
    }

    fn open_host_with_keepalive(
        worker: &FdbWorker,
        runtime: Arc<P>,
        universe: UniverseId,
        world: WorldId,
        lease: &mut WorldLease,
    ) -> Result<(HotWorld, Arc<Mutex<WorldLease>>), WorkerError> {
        let open_started = Instant::now();
        tracing::info!(
            worker_id = %worker.config.worker_id,
            universe_id = %universe,
            world_id = %world,
            lease_epoch = lease.epoch,
            "opening hosted world"
        );
        let lease_cell = Arc::new(Mutex::new(lease.clone()));
        let leased = Arc::new(LeasedWorldPersistence::new(
            Arc::clone(&runtime),
            universe,
            world,
            Arc::clone(&lease_cell),
        ));
        let persistence: Arc<dyn WorldStore> = leased;

        let stop = Arc::new(AtomicBool::new(false));
        let keepalive = spawn_world_keepalive(
            worker,
            Arc::clone(&runtime),
            universe,
            world,
            Arc::clone(&lease_cell),
            Arc::clone(&stop),
        );

        let open_result = worker.open_world(
            persistence,
            Arc::clone(&runtime),
            universe,
            world,
            worker.world_config.clone(),
            worker.adapter_config.clone(),
            worker.kernel_config.clone(),
        );
        stop.store(true, Ordering::Relaxed);
        let keepalive_result = join_open_keepalive(keepalive);

        let latest_lease = lease_cell
            .lock()
            .map(|current| current.clone())
            .map_err(|_| {
                WorkerError::Runtime(std::io::Error::other("world lease mutex poisoned"))
            })?;

        match (open_result, keepalive_result) {
            (Ok(host), Ok(())) => {
                *lease = latest_lease;
                let heights = host.heights();
                tracing::info!(
                    worker_id = %worker.config.worker_id,
                    universe_id = %universe,
                    world_id = %world,
                    lease_epoch = lease.epoch,
                    journal_head = heights.head,
                    snapshot_height = ?heights.snapshot,
                    open_ms = open_started.elapsed().as_millis(),
                    "opened hosted world"
                );
                Ok((host, lease_cell))
            }
            (Err(err), Ok(())) => Err(err.into()),
            (Ok(_), Err(err)) => Err(err),
            (Err(err), Err(_keepalive_err)) => Err(err.into()),
        }
    }

    pub(super) fn open(
        worker: FdbWorker,
        runtime: Arc<P>,
        universe: UniverseId,
        world: WorldId,
        mut lease: WorldLease,
    ) -> Result<Self, WorkerError> {
        let (host, lease_cell) = Self::open_host_with_keepalive(
            &worker,
            Arc::clone(&runtime),
            universe,
            world,
            &mut lease,
        )?;
        let keepalive_stop = Arc::new(AtomicBool::new(false));
        let keepalive_handle = Some(spawn_world_keepalive(
            &worker,
            Arc::clone(&runtime),
            universe,
            world,
            Arc::clone(&lease_cell),
            Arc::clone(&keepalive_stop),
        ));
        let mut runner = Self {
            worker,
            runtime,
            universe,
            world,
            lease: Some(lease),
            lease_cell,
            keepalive_stop,
            keepalive_handle,
            host,
            idle_since_ns: None,
            suspended_since_ns: None,
            last_renew_ns: now_wallclock_ns(),
            last_materialized_head: None,
        };
        let (dropped_receipts, dropped_intents) =
            runner.reconcile_workflow_runtime_waits_if_quiescent()?;
        let _ = runner
            .persist_healed_snapshot_after_reconciliation(dropped_receipts, dropped_intents)?;
        runner.reset_projection_state_after_reopen()?;
        Ok(runner)
    }

    pub(super) fn resume(&mut self, lease: WorldLease) -> Result<(), WorkerError> {
        *self.lease_cell.lock().map_err(|_| {
            WorkerError::Runtime(std::io::Error::other("world lease mutex poisoned"))
        })? = lease.clone();
        self.lease = Some(lease);
        self.idle_since_ns = None;
        self.suspended_since_ns = None;
        self.last_renew_ns = now_wallclock_ns();
        self.keepalive_stop.store(false, Ordering::Relaxed);
        self.keepalive_handle = Some(spawn_world_keepalive(
            &self.worker,
            Arc::clone(&self.runtime),
            self.universe,
            self.world,
            Arc::clone(&self.lease_cell),
            Arc::clone(&self.keepalive_stop),
        ));
        let replay_from = self.host.heights().head;
        self.host.replay_entries_from(replay_from)?;
        let (dropped_receipts, dropped_intents) =
            self.reconcile_workflow_runtime_waits_if_quiescent()?;
        let _ =
            self.persist_healed_snapshot_after_reconciliation(dropped_receipts, dropped_intents)?;
        self.reset_projection_state_after_reopen()?;
        Ok(())
    }

    pub(super) fn suspend(&mut self) {
        self.stop_keepalive();
        self.lease = None;
        self.idle_since_ns = None;
        self.suspended_since_ns = Some(now_wallclock_ns());
    }

    fn stop_keepalive(&mut self) {
        self.keepalive_stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.keepalive_handle.take() {
            let _ = handle.join();
        }
    }

    pub(super) fn current_lease(&self) -> Result<&WorldLease, WorkerError> {
        self.lease.as_ref().ok_or_else(|| {
            WorkerError::Runtime(std::io::Error::other("world is warm but not leased"))
        })
    }

    pub(super) async fn step(&mut self) -> Result<RunnerStep, WorkerError> {
        let now_ns = now_wallclock_ns();
        let Some(current_lease) = self.lease.clone() else {
            return Ok(RunnerStep::Released(ReleaseDisposition::Warm));
        };
        if now_ns.saturating_sub(self.last_renew_ns)
            >= duration_ns(self.worker.config.lease_renew_interval)
        {
            match self.runtime.renew_world_lease(
                self.universe,
                self.world,
                &current_lease,
                now_ns,
                duration_ns(self.worker.config.lease_ttl),
            ) {
                Ok(lease) => {
                    self.lease = Some(lease.clone());
                    *self.lease_cell.lock().map_err(|_| {
                        WorkerError::Runtime(std::io::Error::other("world lease mutex poisoned"))
                    })? = lease;
                    self.last_renew_ns = now_ns;
                }
                Err(PersistError::Conflict(_)) => return Ok(RunnerStep::Fenced),
                Err(err) => return Err(err.into()),
            }
        }

        self.drain_inbox_to_journal()?;
        self.host.drain()?;
        self.materialize_query_projections()?;
        let (published_effects, published_timers) = self.publish_effects_and_timers()?;
        let claimed_effects = self.execute_claimed_effects().await?;
        let claimed_timers = self.fire_due_timers()?;
        if claimed_effects > 0 || claimed_timers > 0 {
            self.drain_inbox_to_journal()?;
            self.host.drain()?;
            self.materialize_query_projections()?;
        }
        let now_ns = now_wallclock_ns();
        let mut info = self
            .runtime
            .world_runtime_info(self.universe, self.world, now_ns)?;
        let has_live_work = info.has_pending_inbox
            || info.has_pending_effects
            || info
                .next_timer_due_at_ns
                .is_some_and(|deliver_at_ns| deliver_at_ns <= now_ns)
            || self.host.has_pending_effects()
            || published_effects > 0
            || published_timers > 0
            || claimed_effects > 0
            || claimed_timers > 0;
        let idle_for_ns = self.quiescent_idle_elapsed_ns(has_live_work, now_ns);
        let performed_maintenance = if info.has_pending_maintenance {
            match idle_for_ns {
                Some(idle_for_ns)
                    if idle_for_ns >= duration_ns(self.worker.config.maintenance_idle_after) =>
                {
                    self.run_snapshot_maintenance(idle_for_ns)?
                }
                _ => false,
            }
        } else {
            false
        };
        let created_replay_snapshot = if has_live_work && self.replay_snapshot_due()? {
            self.run_replay_snapshot_maintenance()?
        } else {
            false
        };
        if performed_maintenance {
            info =
                self.runtime
                    .world_runtime_info(self.universe, self.world, now_wallclock_ns())?;
        }
        let active = has_live_work
            || created_replay_snapshot
            || performed_maintenance
            || info.has_pending_maintenance;
        if active {
            return Ok(RunnerStep::KeepRunning);
        }

        if let Some(admin) = finalize_quiescent_admin(&info.meta.admin) {
            self.runtime
                .set_world_admin_lifecycle(self.universe, self.world, admin)?;
        }

        if info.meta.admin.status.should_release_when_quiescent() {
            let release_lease = self.lease.as_ref().expect("leased runner").clone();
            match self
                .runtime
                .release_world_lease(self.universe, self.world, &release_lease)
            {
                Ok(()) => return Ok(RunnerStep::Released(ReleaseDisposition::Drop)),
                Err(PersistError::Conflict(_)) => return Ok(RunnerStep::Fenced),
                Err(err) => return Err(err.into()),
            }
        }

        let now_ns = now_wallclock_ns();
        let idle_for_ns = self.quiescent_idle_elapsed_ns(false, now_ns).unwrap_or(0);
        if idle_for_ns < duration_ns(self.worker.config.idle_release_after) {
            return Ok(RunnerStep::KeepRunning);
        }

        let release_lease = self.lease.as_ref().expect("leased runner").clone();
        match self
            .runtime
            .release_world_lease(self.universe, self.world, &release_lease)
        {
            Ok(()) => Ok(RunnerStep::Released(ReleaseDisposition::Warm)),
            Err(PersistError::Conflict(_)) => Ok(RunnerStep::Fenced),
            Err(err) => Err(err.into()),
        }
    }

    pub(super) fn step_blocking(&mut self) -> Result<RunnerStep, WorkerError> {
        let now_ns = now_wallclock_ns();
        let Some(current_lease) = self.lease.clone() else {
            return Ok(RunnerStep::Released(ReleaseDisposition::Warm));
        };
        if now_ns.saturating_sub(self.last_renew_ns)
            >= duration_ns(self.worker.config.lease_renew_interval)
        {
            match self.runtime.renew_world_lease(
                self.universe,
                self.world,
                &current_lease,
                now_ns,
                duration_ns(self.worker.config.lease_ttl),
            ) {
                Ok(lease) => {
                    self.lease = Some(lease.clone());
                    *self.lease_cell.lock().map_err(|_| {
                        WorkerError::Runtime(std::io::Error::other("world lease mutex poisoned"))
                    })? = lease;
                    self.last_renew_ns = now_ns;
                }
                Err(PersistError::Conflict(_)) => return Ok(RunnerStep::Fenced),
                Err(err) => return Err(err.into()),
            }
        }

        self.drain_inbox_to_journal()?;
        self.host.drain()?;
        self.materialize_query_projections()?;
        let (published_effects, published_timers) = self.publish_effects_and_timers()?;
        let claimed_effects = self.execute_claimed_effects_blocking()?;
        let claimed_timers = self.fire_due_timers()?;
        if claimed_effects > 0 || claimed_timers > 0 {
            self.drain_inbox_to_journal()?;
            self.host.drain()?;
            self.materialize_query_projections()?;
        }
        let now_ns = now_wallclock_ns();
        let mut info = self
            .runtime
            .world_runtime_info(self.universe, self.world, now_ns)?;
        let has_live_work = info.has_pending_inbox
            || info.has_pending_effects
            || info
                .next_timer_due_at_ns
                .is_some_and(|deliver_at_ns| deliver_at_ns <= now_ns)
            || self.host.has_pending_effects()
            || published_effects > 0
            || published_timers > 0
            || claimed_effects > 0
            || claimed_timers > 0;
        let idle_for_ns = self.quiescent_idle_elapsed_ns(has_live_work, now_ns);
        let performed_maintenance = if info.has_pending_maintenance {
            match idle_for_ns {
                Some(idle_for_ns)
                    if idle_for_ns >= duration_ns(self.worker.config.maintenance_idle_after) =>
                {
                    self.run_snapshot_maintenance(idle_for_ns)?
                }
                _ => false,
            }
        } else {
            false
        };
        let created_replay_snapshot = if has_live_work && self.replay_snapshot_due()? {
            self.run_replay_snapshot_maintenance()?
        } else {
            false
        };
        if performed_maintenance {
            info =
                self.runtime
                    .world_runtime_info(self.universe, self.world, now_wallclock_ns())?;
        }
        let active = has_live_work
            || created_replay_snapshot
            || performed_maintenance
            || info.has_pending_maintenance;
        if active {
            return Ok(RunnerStep::KeepRunning);
        }

        if let Some(admin) = finalize_quiescent_admin(&info.meta.admin) {
            self.runtime
                .set_world_admin_lifecycle(self.universe, self.world, admin)?;
        }

        if info.meta.admin.status.should_release_when_quiescent() {
            let release_lease = self.lease.as_ref().expect("leased runner").clone();
            match self
                .runtime
                .release_world_lease(self.universe, self.world, &release_lease)
            {
                Ok(()) => return Ok(RunnerStep::Released(ReleaseDisposition::Drop)),
                Err(PersistError::Conflict(_)) => return Ok(RunnerStep::Fenced),
                Err(err) => return Err(err.into()),
            }
        }

        let now_ns = now_wallclock_ns();
        let idle_for_ns = self.quiescent_idle_elapsed_ns(false, now_ns).unwrap_or(0);
        if idle_for_ns < duration_ns(self.worker.config.idle_release_after) {
            return Ok(RunnerStep::KeepRunning);
        }

        let release_lease = self.lease.as_ref().expect("leased runner").clone();
        match self
            .runtime
            .release_world_lease(self.universe, self.world, &release_lease)
        {
            Ok(()) => Ok(RunnerStep::Released(ReleaseDisposition::Warm)),
            Err(PersistError::Conflict(_)) => Ok(RunnerStep::Fenced),
            Err(err) => Err(err.into()),
        }
    }

    fn reopen_host(&mut self) -> Result<(), WorkerError> {
        let mut lease = self.current_lease()?.clone();
        let (host, lease_cell) = Self::open_host_with_keepalive(
            &self.worker,
            Arc::clone(&self.runtime),
            self.universe,
            self.world,
            &mut lease,
        )?;
        self.host = host;
        self.lease = Some(lease);
        self.lease_cell = lease_cell;
        Ok(())
    }

    fn reopen_host_and_reconcile_projection_deltas(&mut self) -> Result<(), WorkerError> {
        self.reopen_host()?;
        self.reset_projection_state_after_reopen()?;
        Ok(())
    }

    fn reset_projection_state_after_reopen(&mut self) -> Result<(), WorkerError> {
        let persisted_head = self
            .runtime
            .head_projection(self.universe, self.world)?
            .map(|head| head.journal_head);
        let replayed_head = self.host.heights().head;
        self.last_materialized_head = persisted_head;
        if persisted_head.unwrap_or(0) >= replayed_head {
            self.host.drain_cell_projection_deltas();
            self.last_materialized_head = Some(replayed_head);
        }
        Ok(())
    }

    fn quiescent_idle_elapsed_ns(&mut self, has_live_work: bool, now_ns: u64) -> Option<u64> {
        if has_live_work {
            self.idle_since_ns = None;
            return None;
        }
        let idle_since = self.idle_since_ns.get_or_insert(now_ns);
        Some(now_ns.saturating_sub(*idle_since))
    }

    pub(super) fn drain_inbox_to_journal(&mut self) -> Result<u32, WorkerError> {
        let old_cursor = self.runtime.inbox_cursor(self.universe, self.world)?;
        let items = self.runtime.inbox_read_after(
            self.universe,
            self.world,
            old_cursor.clone(),
            self.worker.config.max_inbox_batch,
        )?;
        if items.is_empty() {
            return Ok(0);
        }

        let mut consumed = 0u32;
        let mut batch_old_cursor = old_cursor;
        let mut batch_expected_head = self.runtime.journal_head(self.universe, self.world)?;
        let mut batch_items: Vec<(aos_fdb::InboxSeq, InboxItem)> = Vec::new();

        for (seq, item) in items {
            if matches!(item, InboxItem::Control(_)) {
                let control_old_cursor = batch_items
                    .last()
                    .map(|(item_seq, _)| item_seq.clone())
                    .or_else(|| batch_old_cursor.clone());
                consumed = consumed.saturating_add(self.flush_journal_ingress_batch(
                    batch_old_cursor.take(),
                    &mut batch_expected_head,
                    &mut batch_items,
                )?);
                self.execute_control_ingress(control_old_cursor, seq.clone(), item)?;
                batch_old_cursor = Some(seq);
                batch_expected_head = self.runtime.journal_head(self.universe, self.world)?;
                consumed = consumed.saturating_add(1);
            } else {
                batch_items.push((seq, item));
            }
        }

        consumed = consumed.saturating_add(self.flush_journal_ingress_batch(
            batch_old_cursor,
            &mut batch_expected_head,
            &mut batch_items,
        )?);
        Ok(consumed)
    }

    fn flush_journal_ingress_batch(
        &mut self,
        old_cursor: Option<aos_fdb::InboxSeq>,
        expected_head: &mut u64,
        items: &mut Vec<(aos_fdb::InboxSeq, InboxItem)>,
    ) -> Result<u32, WorkerError> {
        if items.is_empty() {
            return Ok(0);
        }

        let mut journal_entries = Vec::with_capacity(items.len());
        let mut applied_items = Vec::with_capacity(items.len());
        let mut last_seq = None;
        for (offset, (seq, item)) in items.iter().cloned().enumerate() {
            let journal_seq = expected_head.saturating_add(offset as u64);
            journal_entries
                .push(self.encode_inbox_item_as_journal_entry(journal_seq, item.clone())?);
            applied_items.push(item);
            last_seq = Some(seq);
        }

        let first_height = self.runtime.drain_inbox_to_journal_guarded(
            self.universe,
            self.world,
            self.current_lease()?,
            now_wallclock_ns(),
            old_cursor,
            last_seq.expect("batch has terminal sequence"),
            *expected_head,
            &journal_entries,
        )?;
        debug_assert_eq!(first_height, *expected_head);
        *expected_head = expected_head.saturating_add(journal_entries.len() as u64);
        self.host.set_journal_next_seq(*expected_head);
        for item in applied_items {
            self.apply_ingress_item_to_host(item)?;
        }
        items.clear();
        Ok(journal_entries.len() as u32)
    }

    fn apply_ingress_item_to_host(&mut self, item: InboxItem) -> Result<(), WorkerError> {
        apply_ingress_item_to_hot_world(&*self.runtime, self.universe, &mut self.host, item)
            .map_err(|err| match err {
                HotWorldError::UnsupportedIngressItem(kind) => {
                    WorkerError::UnsupportedInboxItem(kind)
                }
                other => WorkerError::from(other),
            })
    }

    pub(super) fn encode_inbox_item_as_journal_entry(
        &mut self,
        journal_seq: u64,
        item: InboxItem,
    ) -> Result<Vec<u8>, WorkerError> {
        encode_ingress_as_journal_entry(
            &*self.runtime,
            self.universe,
            &mut self.host,
            journal_seq,
            item,
        )
        .map_err(|err| match err {
            HotWorldError::UnsupportedIngressItem(kind) => WorkerError::UnsupportedInboxItem(kind),
            other => WorkerError::from(other),
        })
    }

    fn execute_control_ingress(
        &mut self,
        old_cursor: Option<aos_fdb::InboxSeq>,
        seq: aos_fdb::InboxSeq,
        item: InboxItem,
    ) -> Result<(), WorkerError> {
        let InboxItem::Control(control) = item else {
            return Err(WorkerError::UnsupportedInboxItem("control"));
        };
        let Some(existing) =
            self.runtime
                .command_record(self.universe, self.world, &control.command_id)?
        else {
            return Err(PersistError::not_found(format!("command {}", control.command_id)).into());
        };

        if matches!(
            existing.status,
            CommandStatus::Succeeded | CommandStatus::Failed
        ) {
            self.runtime.inbox_commit_cursor_guarded(
                self.universe,
                self.world,
                self.current_lease()?,
                now_wallclock_ns(),
                old_cursor,
                seq,
            )?;
            return Ok(());
        }

        let started_at_ns = existing.started_at_ns.unwrap_or_else(now_wallclock_ns);
        let mut running = existing.clone();
        running.status = CommandStatus::Running;
        running.started_at_ns = Some(started_at_ns);
        running.finished_at_ns = None;
        running.error = None;
        self.runtime.update_command_record_guarded(
            self.universe,
            self.world,
            self.current_lease()?,
            now_wallclock_ns(),
            running.clone(),
        )?;

        let final_record = match self.run_control_command(&control) {
            Ok(outcome) => {
                let mut record = running;
                record.status = CommandStatus::Succeeded;
                record.finished_at_ns = Some(now_wallclock_ns());
                record.journal_height = outcome.journal_height;
                record.manifest_hash = outcome.manifest_hash;
                record.result_payload = outcome.result_payload;
                record.error = None;
                record
            }
            Err(err) => {
                let mut record = running;
                record.status = CommandStatus::Failed;
                record.finished_at_ns = Some(now_wallclock_ns());
                record.journal_height = Some(self.runtime.journal_head(self.universe, self.world)?);
                record.manifest_hash = Some(self.host.kernel().manifest_hash().to_hex());
                record.result_payload = None;
                record.error = Some(command_error_body(&err));
                record
            }
        };

        self.runtime.update_command_record_guarded(
            self.universe,
            self.world,
            self.current_lease()?,
            now_wallclock_ns(),
            final_record,
        )?;
        self.runtime.inbox_commit_cursor_guarded(
            self.universe,
            self.world,
            self.current_lease()?,
            now_wallclock_ns(),
            old_cursor,
            seq,
        )?;
        Ok(())
    }

    fn run_control_command(
        &mut self,
        control: &CommandIngress,
    ) -> Result<ControlCommandOutcome, WorkerError> {
        let payload = resolve_payload(&*self.runtime, self.universe, &control.payload)?;
        match control.command.as_str() {
            CMD_GOV_PROPOSE => self.run_gov_propose(control, &payload),
            CMD_GOV_SHADOW => self.run_gov_shadow(&payload),
            CMD_GOV_APPROVE => self.run_gov_approve(&payload),
            CMD_GOV_APPLY => self.run_gov_apply(&payload),
            CMD_WORLD_PAUSE => {
                self.run_lifecycle_command(control, WorldAdminStatus::Pausing, &payload)
            }
            CMD_WORLD_ARCHIVE => {
                self.run_lifecycle_command(control, WorldAdminStatus::Archiving, &payload)
            }
            CMD_WORLD_DELETE => {
                self.run_lifecycle_command(control, WorldAdminStatus::Deleting, &payload)
            }
            other => Err(HostError::External(format!("unknown command '{other}'")).into()),
        }
    }

    fn run_gov_propose(
        &mut self,
        control: &CommandIngress,
        payload: &[u8],
    ) -> Result<ControlCommandOutcome, WorkerError> {
        let params: GovProposeParams = serde_cbor::from_slice(payload)?;
        let patch =
            self.prepare_manifest_patch(params.patch.clone(), params.manifest_base.clone())?;
        let proposal_id = self
            .host
            .kernel_mut()
            .submit_proposal(patch, params.description.clone())?;
        let proposal = self
            .host
            .kernel()
            .governance()
            .proposals()
            .get(&proposal_id)
            .ok_or(KernelError::ProposalNotFound(proposal_id))?;
        let receipt = GovProposeReceipt {
            proposal_id,
            patch_hash: hash_ref_from_hex(&proposal.patch_hash)?,
            manifest_base: params.manifest_base,
        };
        let mut outcome = command_success_outcome(&receipt)?;
        outcome.journal_height = Some(self.host.heights().head);
        outcome.manifest_hash = Some(self.host.kernel().manifest_hash().to_hex());
        let _ = control;
        Ok(outcome)
    }

    fn run_gov_shadow(&mut self, payload: &[u8]) -> Result<ControlCommandOutcome, WorkerError> {
        let params: GovShadowParams = serde_cbor::from_slice(payload)?;
        let summary = self
            .host
            .kernel_mut()
            .run_shadow(params.proposal_id, None)?;
        let receipt = GovShadowReceipt {
            proposal_id: params.proposal_id,
            manifest_hash: hash_ref_from_hex(&summary.manifest_hash)?,
            predicted_effects: summary
                .predicted_effects
                .into_iter()
                .map(|effect| {
                    Ok(GovPredictedEffect {
                        kind: effect.kind,
                        cap: effect.cap,
                        intent_hash: hash_ref_from_hex(&effect.intent_hash)?,
                        params_json: effect
                            .params_json
                            .map(|value| serde_json::to_string(&value))
                            .transpose()?,
                    })
                })
                .collect::<Result<Vec<_>, WorkerError>>()?,
            pending_workflow_receipts: summary
                .pending_workflow_receipts
                .into_iter()
                .map(|pending| {
                    Ok(GovPendingWorkflowReceipt {
                        instance_id: pending.instance_id,
                        origin_module_id: pending.origin_module_id,
                        origin_instance_key_b64: pending.origin_instance_key_b64,
                        intent_hash: hash_ref_from_hex(&pending.intent_hash)?,
                        effect_kind: pending.effect_kind,
                        emitted_at_seq: pending.emitted_at_seq,
                    })
                })
                .collect::<Result<Vec<_>, WorkerError>>()?,
            workflow_instances: summary
                .workflow_instances
                .into_iter()
                .map(|instance| GovWorkflowInstancePreview {
                    instance_id: instance.instance_id,
                    status: instance.status,
                    last_processed_event_seq: instance.last_processed_event_seq,
                    module_version: instance.module_version,
                    inflight_intents: instance.inflight_intents as u64,
                })
                .collect(),
            module_effect_allowlists: summary
                .module_effect_allowlists
                .into_iter()
                .map(|allowlist| GovModuleEffectAllowlist {
                    module: allowlist.module,
                    effects_emitted: allowlist.effects_emitted,
                })
                .collect(),
            ledger_deltas: summary
                .ledger_deltas
                .into_iter()
                .map(|delta| GovLedgerDelta {
                    ledger: match delta.ledger {
                        aos_kernel::shadow::LedgerKind::Capability => GovLedgerKind::Capability,
                        aos_kernel::shadow::LedgerKind::Policy => GovLedgerKind::Policy,
                    },
                    name: delta.name,
                    change: match delta.change {
                        aos_kernel::shadow::DeltaKind::Added => GovLedgerChange::Added,
                        aos_kernel::shadow::DeltaKind::Removed => GovLedgerChange::Removed,
                        aos_kernel::shadow::DeltaKind::Changed => GovLedgerChange::Changed,
                    },
                })
                .collect(),
        };
        let mut outcome = command_success_outcome(&receipt)?;
        outcome.journal_height = Some(self.host.heights().head);
        outcome.manifest_hash = Some(self.host.kernel().manifest_hash().to_hex());
        Ok(outcome)
    }

    fn run_gov_approve(&mut self, payload: &[u8]) -> Result<ControlCommandOutcome, WorkerError> {
        let params: GovApproveParams = serde_cbor::from_slice(payload)?;
        match params.decision {
            GovDecision::Approve => self
                .host
                .kernel_mut()
                .approve_proposal(params.proposal_id, params.approver.clone())?,
            GovDecision::Reject => self
                .host
                .kernel_mut()
                .reject_proposal(params.proposal_id, params.approver.clone())?,
        }
        let proposal = self
            .host
            .kernel()
            .governance()
            .proposals()
            .get(&params.proposal_id)
            .ok_or(KernelError::ProposalNotFound(params.proposal_id))?;
        let receipt = GovApproveReceipt {
            proposal_id: params.proposal_id,
            decision: params.decision,
            patch_hash: hash_ref_from_hex(&proposal.patch_hash)?,
            approver: params.approver,
            reason: params.reason,
        };
        let mut outcome = command_success_outcome(&receipt)?;
        outcome.journal_height = Some(self.host.heights().head);
        outcome.manifest_hash = Some(self.host.kernel().manifest_hash().to_hex());
        Ok(outcome)
    }

    fn run_gov_apply(&mut self, payload: &[u8]) -> Result<ControlCommandOutcome, WorkerError> {
        let params: GovApplyParams = serde_cbor::from_slice(payload)?;
        let proposal = self
            .host
            .kernel()
            .governance()
            .proposals()
            .get(&params.proposal_id)
            .ok_or(KernelError::ProposalNotFound(params.proposal_id))?
            .clone();
        self.host.kernel_mut().apply_proposal(params.proposal_id)?;
        let receipt = GovApplyReceipt {
            proposal_id: params.proposal_id,
            manifest_hash_new: hash_ref_from_hex(&self.host.kernel().manifest_hash().to_hex())?,
            patch_hash: hash_ref_from_hex(&proposal.patch_hash)?,
        };
        self.reopen_host_and_reconcile_projection_deltas()?;
        let mut outcome = command_success_outcome(&receipt)?;
        outcome.journal_height = Some(self.host.heights().head);
        outcome.manifest_hash = Some(self.host.kernel().manifest_hash().to_hex());
        Ok(outcome)
    }

    fn run_lifecycle_command(
        &mut self,
        control: &CommandIngress,
        target_status: WorldAdminStatus,
        payload: &[u8],
    ) -> Result<ControlCommandOutcome, WorkerError> {
        let params: LifecycleCommandParams = serde_cbor::from_slice(payload)?;
        let now_ns = now_wallclock_ns();
        let info = self
            .runtime
            .world_runtime_info(self.universe, self.world, now_ns)?;
        let next_admin = next_admin_lifecycle(
            &info.meta.admin,
            self.world,
            target_status,
            &control.command_id,
            params.reason,
            now_ns,
        )?;
        self.runtime
            .set_world_admin_lifecycle(self.universe, self.world, next_admin.clone())?;
        let mut outcome = command_success_outcome(&next_admin)?;
        outcome.journal_height = Some(self.runtime.journal_head(self.universe, self.world)?);
        outcome.manifest_hash = info
            .meta
            .manifest_hash
            .or_else(|| Some(self.host.kernel().manifest_hash().to_hex()));
        Ok(outcome)
    }

    fn prepare_manifest_patch(
        &self,
        input: GovPatchInput,
        manifest_base: Option<HashRef>,
    ) -> Result<ManifestPatch, WorkerError> {
        match input {
            GovPatchInput::Hash(hash) => {
                if manifest_base.is_some() {
                    return Err(KernelError::Manifest(
                        "manifest_base is not supported with patch hash input".into(),
                    )
                    .into());
                }
                let bytes = self
                    .host
                    .store()
                    .get_blob(parse_hash_ref(hash.as_str())?)
                    .map_err(WorkerError::Store)?;
                Ok(serde_cbor::from_slice(&bytes)?)
            }
            GovPatchInput::PatchCbor(bytes) => {
                if manifest_base.is_some() {
                    return Err(KernelError::Manifest(
                        "manifest_base is not supported with patch_cbor input".into(),
                    )
                    .into());
                }
                let patch: ManifestPatch = serde_cbor::from_slice(&bytes)?;
                canonicalize_patch(self.host.store(), patch).map_err(WorkerError::Kernel)
            }
            GovPatchInput::PatchDocJson(bytes) => {
                let doc: PatchDocument = serde_json::from_slice(&bytes)?;
                if let Some(expected) = manifest_base.as_ref()
                    && expected.as_str() != doc.base_manifest_hash
                {
                    return Err(KernelError::Manifest(format!(
                        "manifest_base mismatch: expected {expected}, got {}",
                        doc.base_manifest_hash
                    ))
                    .into());
                }
                compile_patch_document(self.host.store(), doc).map_err(WorkerError::Kernel)
            }
            GovPatchInput::PatchBlobRef { blob_ref, format } => {
                let bytes = self
                    .host
                    .store()
                    .get_blob(parse_hash_ref(blob_ref.as_str())?)
                    .map_err(WorkerError::Store)?;
                match format.as_str() {
                    "manifest_patch_cbor" => {
                        self.prepare_manifest_patch(GovPatchInput::PatchCbor(bytes), manifest_base)
                    }
                    "patch_doc_json" => self
                        .prepare_manifest_patch(GovPatchInput::PatchDocJson(bytes), manifest_base),
                    other => Err(KernelError::Manifest(format!(
                        "unknown patch blob format '{other}'"
                    ))
                    .into()),
                }
            }
        }
    }

    fn materialize_query_projections(&mut self) -> Result<(), WorkerError> {
        let journal_head = self.host.heights().head;
        if self.last_materialized_head == Some(journal_head) {
            return Ok(());
        }

        let now_ns = now_wallclock_ns();
        let deltas = self.host.drain_cell_projection_deltas();
        for delta in &deltas {
            if let Some(state) = &delta.state {
                let stored = self
                    .runtime
                    .cas_put_verified(self.universe, &state.state_bytes)?;
                if stored != state.state_hash {
                    return Err(WorkerError::from(HotWorldError::InvalidHash(format!(
                        "cell projection state hash mismatch: expected {}, stored {}",
                        state.state_hash.to_hex(),
                        stored.to_hex()
                    ))));
                }
            }
        }
        let (cell_upserts, cell_deletes, workspace_upserts, workspace_deletes) =
            self.projection_delta_records(journal_head, now_ns, deltas)?;

        self.runtime.apply_query_projection_delta_guarded(
            self.universe,
            self.world,
            self.current_lease()?,
            now_ns,
            QueryProjectionDelta {
                head: HeadProjectionRecord {
                    journal_head,
                    manifest_hash: self.host.kernel().manifest_hash().to_hex(),
                    updated_at_ns: now_ns,
                },
                cell_upserts,
                cell_deletes,
                workspace_upserts,
                workspace_deletes,
            },
        )?;
        self.last_materialized_head = Some(journal_head);
        Ok(())
    }

    fn projection_delta_records(
        &self,
        journal_head: u64,
        now_ns: u64,
        deltas: Vec<CellProjectionDelta>,
    ) -> Result<
        (
            Vec<CellStateProjectionRecord>,
            Vec<CellStateProjectionDelete>,
            Vec<WorkspaceRegistryProjectionRecord>,
            Vec<WorkspaceProjectionDelete>,
        ),
        WorkerError,
    > {
        let mut cell_upserts = Vec::new();
        let mut cell_deletes = Vec::new();
        let mut workspace_upserts = Vec::new();
        let mut workspace_deletes = Vec::new();
        for delta in deltas {
            match delta.state {
                Some(state) => {
                    cell_upserts.push(self.cell_projection_record(
                        journal_head,
                        delta.workflow.clone(),
                        delta.key_hash,
                        delta.key_bytes.clone(),
                        state.clone(),
                    ));
                    if delta.workflow == "sys/Workspace@1" {
                        let workspace: String = serde_cbor::from_slice(&delta.key_bytes)?;
                        if let Some(workspace) = self.workspace_projection_from_state(
                            journal_head,
                            now_ns,
                            workspace.clone(),
                            state,
                        )? {
                            workspace_upserts.push(workspace);
                        } else {
                            workspace_deletes.push(WorkspaceProjectionDelete { workspace });
                        }
                    }
                }
                None => {
                    if delta.workflow == "sys/Workspace@1" {
                        let workspace: String = serde_cbor::from_slice(&delta.key_bytes)?;
                        workspace_deletes.push(WorkspaceProjectionDelete { workspace });
                    }
                    cell_deletes.push(CellStateProjectionDelete {
                        workflow: delta.workflow,
                        key_hash: delta.key_hash,
                    });
                }
            }
        }
        workspace_upserts.sort_by(|left, right| left.workspace.cmp(&right.workspace));
        workspace_deletes.sort_by(|left, right| left.workspace.cmp(&right.workspace));
        Ok((
            cell_upserts,
            cell_deletes,
            workspace_upserts,
            workspace_deletes,
        ))
    }

    fn cell_projection_record(
        &self,
        journal_head: u64,
        workflow: String,
        key_hash: Vec<u8>,
        key_bytes: Vec<u8>,
        state: CellProjectionDeltaState,
    ) -> CellStateProjectionRecord {
        CellStateProjectionRecord {
            journal_head,
            workflow,
            key_hash,
            key_bytes,
            state_hash: state.state_hash.to_hex(),
            size: state.size,
            last_active_ns: state.last_active_ns,
        }
    }

    fn workspace_projection_from_state(
        &self,
        journal_head: u64,
        now_ns: u64,
        workspace: String,
        state: CellProjectionDeltaState,
    ) -> Result<Option<WorkspaceRegistryProjectionRecord>, WorkerError> {
        let history: WorkspaceHistoryState = serde_cbor::from_slice(&state.state_bytes)?;
        if history.versions.is_empty() {
            return Ok(None);
        }
        let versions = history
            .versions
            .into_iter()
            .map(|(version, meta)| {
                (
                    version,
                    WorkspaceVersionProjectionRecord {
                        root_hash: meta.root_hash,
                        owner: meta.owner,
                        created_at_ns: meta.created_at,
                    },
                )
            })
            .collect();
        Ok(Some(WorkspaceRegistryProjectionRecord {
            journal_head,
            workspace,
            latest_version: history.latest,
            versions,
            updated_at_ns: now_ns,
        }))
    }

    pub(super) fn publish_effects_and_timers(&mut self) -> Result<(u32, u32), WorkerError> {
        let pending_contexts: HashMap<[u8; 32], String> = self
            .host
            .kernel()
            .pending_workflow_receipts_snapshot()
            .into_iter()
            .map(|pending| (pending.intent_hash, pending.origin_module_id))
            .collect();
        let intents = self.host.kernel_mut().drain_effects()?;
        if intents.is_empty() {
            return Ok((0, 0));
        }

        let now_ns = now_wallclock_ns();
        let mut effect_items = Vec::new();
        let mut timer_items = Vec::new();
        let mut handled_internal = false;

        for intent in intents {
            if let Some(receipt) = self.host.kernel_mut().handle_internal_intent(&intent)? {
                self.host.kernel_mut().handle_receipt(receipt)?;
                handled_internal = true;
                continue;
            }

            let shard = shard_for_hash(&intent.intent_hash, self.worker.config.shard_count);
            if intent.kind.as_str() == EffectKind::TIMER_SET {
                let params: TimerSetParams = serde_cbor::from_slice(&intent.params_cbor)?;
                timer_items.push(TimerDueItem {
                    shard,
                    universe_id: self.universe,
                    world_id: self.world,
                    intent_hash: intent.intent_hash.to_vec(),
                    time_bucket: time_bucket_for(params.deliver_at_ns),
                    deliver_at_ns: params.deliver_at_ns,
                    payload_cbor: intent.params_cbor.clone(),
                    enqueued_at_ns: now_ns,
                });
                continue;
            }

            effect_items.push(EffectDispatchItem {
                shard,
                universe_id: self.universe,
                world_id: self.world,
                intent_hash: intent.intent_hash.to_vec(),
                effect_kind: intent.kind.as_str().to_string(),
                cap_name: intent.cap_name.clone(),
                params_inline_cbor: Some(intent.params_cbor.clone()),
                params_ref: None,
                params_size: None,
                params_sha256: None,
                idempotency_key: intent.idempotency_key.to_vec(),
                origin_name: pending_contexts
                    .get(&intent.intent_hash)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string()),
                policy_context_hash: None,
                enqueued_at_ns: now_ns,
            });
        }

        if handled_internal {
            self.host.drain()?;
        }
        let published_effects = if effect_items.is_empty() {
            0
        } else {
            self.runtime.publish_effect_dispatches_guarded(
                self.universe,
                self.world,
                self.current_lease()?,
                now_ns,
                &effect_items,
            )?
        };
        let published_timers = if timer_items.is_empty() {
            0
        } else {
            self.runtime.publish_due_timers_guarded(
                self.universe,
                self.world,
                self.current_lease()?,
                now_ns,
                &timer_items,
            )?
        };
        Ok((published_effects, published_timers))
    }

    fn prune_orphan_effect_dispatches(&self) -> Result<u32, WorkerError> {
        let valid_receipt_intents: HashSet<[u8; 32]> = self
            .host
            .kernel()
            .pending_workflow_receipts_snapshot()
            .into_iter()
            .map(|pending| pending.intent_hash)
            .collect();
        let dropped = self.runtime.retain_effect_dispatches_for_world(
            self.universe,
            self.world,
            &valid_receipt_intents,
            now_wallclock_ns(),
        )?;
        if dropped > 0 {
            tracing::warn!(
                worker_id = %self.worker.config.worker_id,
                universe_id = %self.universe,
                world_id = %self.world,
                dropped_dispatches = dropped,
                "pruned orphan hosted effect dispatches with no live workflow receipt context"
            );
        }
        Ok(dropped)
    }

    async fn execute_claimed_effects(&mut self) -> Result<u32, WorkerError> {
        self.prune_orphan_effect_dispatches()?;
        let claimed = self.runtime.claim_pending_effects_for_world(
            self.universe,
            self.world,
            &self.worker.config.worker_id,
            now_wallclock_ns(),
            duration_ns(self.worker.config.effect_claim_timeout),
            self.worker.config.max_effects_per_cycle,
        )?;
        if claimed.is_empty() {
            return Ok(0);
        }
        tracing::debug!(
            worker_id = %self.worker.config.worker_id,
            universe_id = %self.universe,
            world_id = %self.world,
            claimed_effects = claimed.len(),
            max_effects_per_cycle = self.worker.config.max_effects_per_cycle,
            "claimed hosted effects"
        );

        let valid_receipt_intents: HashSet<[u8; 32]> = self
            .host
            .kernel()
            .pending_workflow_receipts_snapshot()
            .into_iter()
            .map(|pending| pending.intent_hash)
            .collect();
        let mut ack_items = Vec::new();
        let mut routed = Vec::new();
        for (seq, item) in &claimed {
            let intent_hash = parse_intent_hash(&item.intent_hash)?;
            if !valid_receipt_intents.contains(&intent_hash) {
                self.runtime.retain_effect_dispatches_for_world(
                    self.universe,
                    self.world,
                    &valid_receipt_intents,
                    now_wallclock_ns(),
                )?;
                tracing::warn!(
                    worker_id = %self.worker.config.worker_id,
                    universe_id = %self.universe,
                    world_id = %self.world,
                    intent_hash = %hex::encode(intent_hash),
                    effect_kind = %item.effect_kind,
                    "discarded orphan claimed hosted effect with no live workflow receipt context"
                );
                continue;
            }
            if item.effect_kind == EffectKind::PORTAL_SEND {
                let receipt = self.execute_claimed_portal_effect(item)?;
                self.runtime.ack_effect_dispatch_with_receipt(
                    self.universe,
                    self.world,
                    &self.worker.config.worker_id,
                    item.shard,
                    seq.clone(),
                    now_wallclock_ns(),
                    receipt_to_ingress(item.effect_kind.clone(), receipt),
                )?;
                continue;
            }
            let params_cbor = resolve_dispatch_params(&*self.runtime, self.universe, item)?;
            let idempotency_key = parse_idempotency_key(&item.idempotency_key)?;
            let intent = EffectIntent {
                kind: EffectKind::new(item.effect_kind.clone()),
                cap_name: item.cap_name.clone(),
                params_cbor,
                idempotency_key,
                intent_hash,
            };
            let route_id = self.host.resolve_effect_route_id(item.effect_kind.as_str());
            ack_items.push((item.shard, seq.clone(), item.effect_kind.clone()));
            routed.push((intent, route_id));
        }
        let executed_count = routed.len() as u32;
        let receipts = self
            .host
            .adapter_registry_mut()
            .execute_batch_routed(routed)
            .await;
        for ((shard, seq, effect_kind), receipt) in ack_items.into_iter().zip(receipts) {
            self.runtime.ack_effect_dispatch_with_receipt(
                self.universe,
                self.world,
                &self.worker.config.worker_id,
                shard,
                seq,
                now_wallclock_ns(),
                receipt_to_ingress(effect_kind, receipt),
            )?;
        }
        Ok(executed_count)
    }

    pub(super) fn execute_claimed_effects_blocking(&mut self) -> Result<u32, WorkerError> {
        self.prune_orphan_effect_dispatches()?;
        let claimed = self.runtime.claim_pending_effects_for_world(
            self.universe,
            self.world,
            &self.worker.config.worker_id,
            now_wallclock_ns(),
            duration_ns(self.worker.config.effect_claim_timeout),
            self.worker.config.max_effects_per_cycle,
        )?;
        if claimed.is_empty() {
            return Ok(0);
        }
        tracing::debug!(
            worker_id = %self.worker.config.worker_id,
            universe_id = %self.universe,
            world_id = %self.world,
            claimed_effects = claimed.len(),
            max_effects_per_cycle = self.worker.config.max_effects_per_cycle,
            "claimed hosted effects"
        );

        let runtime = Builder::new_current_thread().enable_all().build()?;
        let valid_receipt_intents: HashSet<[u8; 32]> = self
            .host
            .kernel()
            .pending_workflow_receipts_snapshot()
            .into_iter()
            .map(|pending| pending.intent_hash)
            .collect();
        let mut ack_items = Vec::new();
        let mut routed = Vec::new();
        for (seq, item) in &claimed {
            let intent_hash = parse_intent_hash(&item.intent_hash)?;
            if !valid_receipt_intents.contains(&intent_hash) {
                self.runtime.retain_effect_dispatches_for_world(
                    self.universe,
                    self.world,
                    &valid_receipt_intents,
                    now_wallclock_ns(),
                )?;
                tracing::warn!(
                    worker_id = %self.worker.config.worker_id,
                    universe_id = %self.universe,
                    world_id = %self.world,
                    intent_hash = %hex::encode(intent_hash),
                    effect_kind = %item.effect_kind,
                    "discarded orphan claimed hosted effect with no live workflow receipt context"
                );
                continue;
            }
            if item.effect_kind == EffectKind::PORTAL_SEND {
                let receipt = self.execute_claimed_portal_effect(item)?;
                self.runtime.ack_effect_dispatch_with_receipt(
                    self.universe,
                    self.world,
                    &self.worker.config.worker_id,
                    item.shard,
                    seq.clone(),
                    now_wallclock_ns(),
                    receipt_to_ingress(item.effect_kind.clone(), receipt),
                )?;
                continue;
            }
            let params_cbor = resolve_dispatch_params(&*self.runtime, self.universe, item)?;
            let idempotency_key = parse_idempotency_key(&item.idempotency_key)?;
            let intent = EffectIntent {
                kind: EffectKind::new(item.effect_kind.clone()),
                cap_name: item.cap_name.clone(),
                params_cbor,
                idempotency_key,
                intent_hash,
            };
            let route_id = self.host.resolve_effect_route_id(item.effect_kind.as_str());
            ack_items.push((item.shard, seq.clone(), item.effect_kind.clone()));
            routed.push((intent, route_id));
        }
        let executed_count = routed.len() as u32;
        let receipts = runtime.block_on(
            self.host
                .adapter_registry_mut()
                .execute_batch_routed(routed),
        );
        for ((shard, seq, effect_kind), receipt) in ack_items.into_iter().zip(receipts) {
            self.runtime.ack_effect_dispatch_with_receipt(
                self.universe,
                self.world,
                &self.worker.config.worker_id,
                shard,
                seq,
                now_wallclock_ns(),
                receipt_to_ingress(effect_kind, receipt),
            )?;
        }
        Ok(executed_count)
    }

    fn execute_claimed_portal_effect(
        &mut self,
        item: &EffectDispatchItem,
    ) -> Result<EffectReceipt, WorkerError> {
        let params_cbor = resolve_dispatch_params(&*self.runtime, self.universe, item)?;
        let params: PortalSendParams = serde_cbor::from_slice(&params_cbor)?;
        let intent_hash = parse_intent_hash(&item.intent_hash)?;
        let dest_universe = params
            .dest_universe
            .as_deref()
            .map(parse_universe_id)
            .transpose()?
            .unwrap_or(self.universe);
        let dest_world = parse_world_id(&params.dest_world)?;
        let message_id = Hash::from(intent_hash).to_hex();

        let (receipt_status, portal_status, enqueued_seq) = match params.mode {
            PortalSendMode::TypedEvent => {
                let schema = params
                    .schema
                    .ok_or(WorkerError::InvalidPortalParams("schema"))?;
                let value_cbor = params
                    .value_cbor
                    .ok_or(WorkerError::InvalidPortalParams("value_cbor"))?;
                let result = self.runtime.portal_send(
                    dest_universe,
                    dest_world,
                    now_wallclock_ns(),
                    &item.intent_hash,
                    InboxItem::DomainEvent(DomainEventIngress {
                        schema,
                        value: CborPayload::inline(value_cbor),
                        key: None,
                        correlation_id: params.correlation_id,
                    }),
                )?;
                let status = match result.status {
                    PortalSendStatus::Enqueued => "ok",
                    PortalSendStatus::AlreadyEnqueued => "already_enqueued",
                };
                (ReceiptStatus::Ok, status.to_string(), result.enqueued_seq)
            }
            PortalSendMode::Inbox => (ReceiptStatus::Error, "error".to_string(), None),
        };

        let payload_cbor = serde_cbor::to_vec(&PortalSendReceipt {
            status: portal_status,
            message_id,
            dest_world: dest_world.to_string(),
            enqueued_seq: enqueued_seq.map(|seq| seq.as_bytes().to_vec()),
        })?;
        Ok(EffectReceipt {
            intent_hash,
            adapter_id: EffectKind::PORTAL_SEND.to_string(),
            status: receipt_status,
            payload_cbor,
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }

    pub(super) fn fire_due_timers(&mut self) -> Result<u32, WorkerError> {
        let claimed = self.runtime.claim_due_timers_for_world(
            self.universe,
            self.world,
            &self.worker.config.worker_id,
            now_wallclock_ns(),
            duration_ns(self.worker.config.timer_claim_timeout),
            self.worker.config.max_timers_per_cycle,
        )?;
        if claimed.is_empty() {
            return Ok(0);
        }
        tracing::debug!(
            worker_id = %self.worker.config.worker_id,
            universe_id = %self.universe,
            world_id = %self.world,
            claimed_timers = claimed.len(),
            max_timers_per_cycle = self.worker.config.max_timers_per_cycle,
            "claimed hosted timers"
        );

        for item in &claimed {
            let params: TimerSetParams = serde_cbor::from_slice(&item.payload_cbor)?;
            let payload_cbor = serde_cbor::to_vec(&TimerSetReceipt {
                delivered_at_ns: now_wallclock_ns(),
                key: params.key,
            })?;
            self.runtime.ack_timer_delivery_with_receipt(
                self.universe,
                self.world,
                &self.worker.config.worker_id,
                &item.intent_hash,
                now_wallclock_ns(),
                ReceiptIngress {
                    intent_hash: item.intent_hash.clone(),
                    effect_kind: EffectKind::TIMER_SET.to_string(),
                    adapter_id: EffectKind::TIMER_SET.to_string(),
                    status: ReceiptStatus::Ok,
                    payload: CborPayload::inline(payload_cbor),
                    cost_cents: Some(0),
                    signature: vec![0; 64],
                    correlation_id: None,
                },
            )?;
        }
        Ok(claimed.len() as u32)
    }

    fn run_snapshot_maintenance(&mut self, idle_for_ns: u64) -> Result<bool, WorkerError> {
        let config = self.runtime.snapshot_maintenance_config();
        let (dropped_receipts, dropped_intents) =
            self.reconcile_workflow_runtime_waits_if_quiescent()?;
        let _ =
            self.persist_healed_snapshot_after_reconciliation(dropped_receipts, dropped_intents)?;
        let baseline_before = self
            .runtime
            .snapshot_active_baseline(self.universe, self.world)?;
        let head_before = self.runtime.journal_head(self.universe, self.world)?;
        let mut performed = false;
        let pending_receipts = self
            .host
            .kernel()
            .pending_workflow_receipts_snapshot()
            .len();
        let open_workflow_intents: usize = self
            .host
            .kernel()
            .workflow_instances_snapshot()
            .into_iter()
            .map(|instance| instance.inflight_intents.len())
            .sum();
        if pending_receipts > 0 || open_workflow_intents > 0 {
            tracing::debug!(
                worker_id = %self.worker.config.worker_id,
                universe_id = %self.universe,
                world_id = %self.world,
                pending_receipts,
                open_workflow_intents,
                "skipping hosted snapshot maintenance because workflow work is still in flight"
            );
            return Ok(false);
        }

        let tail_entries_after_baseline =
            head_before.saturating_sub(baseline_before.height.saturating_add(1));
        if tail_entries_after_baseline >= config.snapshot_after_journal_entries {
            self.snapshot_hosted()?;
            let active_baseline_after =
                self.host.kernel().get_journal_head().active_baseline_height;
            tracing::info!(
                worker_id = %self.worker.config.worker_id,
                universe_id = %self.universe,
                world_id = %self.world,
                idle_for_ms = idle_for_ns / 1_000_000,
                baseline_height_before = baseline_before.height,
                active_baseline_height_after = ?active_baseline_after,
                journal_head_before = head_before,
                tail_entries_after_baseline,
                "performed hosted snapshot maintenance"
            );
            performed = true;
        }

        let mut baseline = self
            .runtime
            .snapshot_active_baseline(self.universe, self.world)?;
        let next_segment_start = self.next_unsegmented_journal_height()?;
        let safe_exclusive_end_before_refresh = baseline
            .height
            .saturating_sub(config.segment_hot_tail_margin);
        let segment_export_due = next_segment_start < safe_exclusive_end_before_refresh;

        // Segment export immediately reopens from the active baseline. If that baseline is stale,
        // reopen can transiently resurrect runtime-only waits that were already cleared by later
        // journal tail entries. Refresh the baseline from the current quiescent host state first.
        if segment_export_due && baseline.height.saturating_add(1) < head_before {
            let baseline_height_before_refresh = baseline.height;
            self.snapshot_hosted()?;
            baseline = self
                .runtime
                .snapshot_active_baseline(self.universe, self.world)?;
            tracing::info!(
                worker_id = %self.worker.config.worker_id,
                universe_id = %self.universe,
                world_id = %self.world,
                idle_for_ms = idle_for_ns / 1_000_000,
                baseline_height_before = baseline_height_before_refresh,
                baseline_height_after = baseline.height,
                journal_head_before = head_before,
                "refreshed hosted snapshot baseline before segment export"
            );
            performed = true;
        }

        let safe_exclusive_end = baseline
            .height
            .saturating_sub(config.segment_hot_tail_margin);
        if next_segment_start < safe_exclusive_end {
            let target_entries = config.segment_target_entries.max(1);
            let segment_end = next_segment_start
                .saturating_add(target_entries.saturating_sub(1))
                .min(safe_exclusive_end.saturating_sub(1));
            self.runtime.segment_export_guarded(
                self.universe,
                self.world,
                self.current_lease()?,
                now_wallclock_ns(),
                SegmentExportRequest {
                    segment: aos_fdb::SegmentId::new(next_segment_start, segment_end)?,
                    delete_chunk_entries: config.segment_delete_chunk_entries.max(1),
                    hot_tail_margin: config.segment_hot_tail_margin,
                },
            )?;
            self.reopen_host_and_reconcile_projection_deltas()?;
            tracing::info!(
                worker_id = %self.worker.config.worker_id,
                universe_id = %self.universe,
                world_id = %self.world,
                idle_for_ms = idle_for_ns / 1_000_000,
                baseline_height = baseline.height,
                next_segment_start,
                segment_end,
                "exported hosted journal segment"
            );
            performed = true;
        }

        Ok(performed)
    }

    fn replay_snapshot_due(&self) -> Result<bool, WorkerError> {
        let config = self.runtime.snapshot_maintenance_config();
        let journal_head = self.runtime.journal_head(self.universe, self.world)?;
        let snapshot_height = self.host.heights().snapshot.or_else(|| {
            self.runtime
                .snapshot_active_baseline(self.universe, self.world)
                .ok()
                .map(|record| record.height)
        });
        let Some(snapshot_height) = snapshot_height else {
            return Ok(false);
        };
        let tail_entries_after_seed =
            journal_head.saturating_sub(snapshot_height.saturating_add(1));
        Ok(tail_entries_after_seed >= config.snapshot_after_journal_entries)
    }

    fn run_replay_snapshot_maintenance(&mut self) -> Result<bool, WorkerError> {
        let config = self.runtime.snapshot_maintenance_config();
        let journal_head_before = self.runtime.journal_head(self.universe, self.world)?;
        let snapshot_height_before = self.host.heights().snapshot.or_else(|| {
            self.runtime
                .snapshot_active_baseline(self.universe, self.world)
                .ok()
                .map(|record| record.height)
        });
        let Some(snapshot_height_before) = snapshot_height_before else {
            return Ok(false);
        };
        let tail_entries_after_seed =
            journal_head_before.saturating_sub(snapshot_height_before.saturating_add(1));
        if tail_entries_after_seed < config.snapshot_after_journal_entries {
            return Ok(false);
        }

        self.snapshot_hosted()?;
        let snapshot_height_after = self.host.heights().snapshot;
        let active_baseline_height_after =
            self.host.kernel().get_journal_head().active_baseline_height;
        tracing::info!(
            worker_id = %self.worker.config.worker_id,
            universe_id = %self.universe,
            world_id = %self.world,
            snapshot_height_before,
            snapshot_height_after = ?snapshot_height_after,
            active_baseline_height_after = ?active_baseline_height_after,
            journal_head_before,
            tail_entries_after_seed,
            "created hosted replay snapshot"
        );
        Ok(snapshot_height_after != Some(snapshot_height_before))
    }

    fn next_unsegmented_journal_height(&self) -> Result<u64, WorkerError> {
        let mut segments =
            self.runtime
                .segment_index_read_from(self.universe, self.world, 0, u32::MAX)?;
        segments.sort_by_key(|record| record.segment.start);
        let mut next_height = 0u64;
        for record in segments {
            if record.segment.end < next_height {
                continue;
            }
            if record.segment.start > next_height {
                break;
            }
            next_height = record.segment.end.saturating_add(1);
        }
        Ok(next_height)
    }
}

impl<P> Drop for WorldRunner<P> {
    fn drop(&mut self) {
        self.keepalive_stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.keepalive_handle.take() {
            let _ = handle.join();
        }
    }
}
