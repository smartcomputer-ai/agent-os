use super::*;

impl<P> WorkerSupervisor<P>
where
    P: HostedRuntimeStore + SecretStore + UniverseStore + 'static,
{
    fn should_retry_faulted_world(&self, world_ref: ActiveWorldRef, now_ns: u64) -> bool {
        self.faulted_worlds
            .get(&world_ref)
            .is_none_or(|state| now_ns >= state.next_retry_ns)
    }

    fn clear_world_fault(&mut self, world_ref: ActiveWorldRef) {
        self.faulted_worlds.remove(&world_ref);
    }

    fn record_world_fault(&mut self, world_ref: ActiveWorldRef, err: &WorkerError, now_ns: u64) {
        let attempts = self
            .faulted_worlds
            .get(&world_ref)
            .map(|state| state.attempts.saturating_add(1))
            .unwrap_or(1);
        let backoff_secs = 2_u64.saturating_pow(attempts.saturating_sub(1).min(6));
        let backoff_ns = backoff_secs.saturating_mul(1_000_000_000);
        self.faulted_worlds.insert(
            world_ref,
            FaultedWorldState {
                attempts,
                next_retry_ns: now_ns.saturating_add(backoff_ns),
            },
        );
        tracing::error!(
            worker_id = %self.worker.config.worker_id,
            universe_id = %world_ref.universe_id,
            world_id = %world_ref.world_id,
            attempts,
            retry_after_seconds = backoff_secs,
            error = %err,
            "isolated faulted hosted world; worker will continue supervising other worlds"
        );
    }

    fn finalize_faulted_terminal_admin(
        &mut self,
        world_ref: ActiveWorldRef,
    ) -> Result<bool, WorkerError> {
        let now_ns = now_wallclock_ns();
        let info =
            self.runtime
                .world_runtime_info(world_ref.universe_id, world_ref.world_id, now_ns)?;
        let next_status = match info.meta.admin.status {
            WorldAdminStatus::Archiving => Some(WorldAdminStatus::Archived),
            WorldAdminStatus::Deleting => Some(WorldAdminStatus::Deleted),
            _ => None,
        };
        let Some(next_status) = next_status else {
            return Ok(false);
        };
        let mut admin = info.meta.admin;
        admin.status = next_status;
        admin.updated_at_ns = now_ns;
        self.runtime
            .set_world_admin_lifecycle(world_ref.universe_id, world_ref.world_id, admin)?;
        self.faulted_worlds.remove(&world_ref);
        self.warm_worlds.remove(&world_ref);
        tracing::warn!(
            worker_id = %self.worker.config.worker_id,
            universe_id = %world_ref.universe_id,
            world_id = %world_ref.world_id,
            status = ?next_status,
            "finalized terminal world admin state after world-local fault"
        );
        Ok(true)
    }

    fn release_world_lease_after_fault(
        &self,
        world_ref: ActiveWorldRef,
        lease: &WorldLease,
    ) -> Result<(), WorkerError> {
        match self
            .runtime
            .release_world_lease(world_ref.universe_id, world_ref.world_id, lease)
        {
            Ok(()) | Err(PersistError::Conflict(_)) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    pub fn active_worlds(&self) -> Vec<ActiveWorldRef> {
        let mut worlds: Vec<_> = self.active_worlds.keys().copied().collect();
        worlds.sort();
        worlds
    }

    pub fn active_world_debug_state(
        &self,
        world_ref: ActiveWorldRef,
    ) -> Option<ActiveWorldDebugState> {
        let runner = self.active_worlds.get(&world_ref)?;
        let pending_receipts = runner.host.kernel().pending_workflow_receipts_snapshot();
        let queued_effects = runner.host.kernel().queued_effects_snapshot();
        let workflow_instances = runner.host.kernel().workflow_instances_snapshot();
        Some(ActiveWorldDebugState {
            pending_receipt_intent_hashes: pending_receipts
                .iter()
                .map(|receipt| {
                    Hash::from_bytes(&receipt.intent_hash)
                        .expect("intent hash is 32 bytes")
                        .to_hex()
                })
                .collect(),
            pending_receipts: pending_receipts
                .into_iter()
                .map(|receipt| {
                    let origin_module_id = receipt.origin_module_id;
                    let origin_instance_id = format!(
                        "{}::{}",
                        origin_module_id,
                        receipt
                            .origin_instance_key
                            .as_deref()
                            .map(hex::encode)
                            .unwrap_or_default()
                    );
                    PendingReceiptDebugState {
                        intent_hash: Hash::from_bytes(&receipt.intent_hash)
                            .expect("intent hash is 32 bytes")
                            .to_hex(),
                        origin_module_id,
                        origin_instance_id,
                        effect_kind: receipt.effect_kind,
                        emitted_at_seq: receipt.emitted_at_seq,
                    }
                })
                .collect(),
            queued_effect_intent_hashes: queued_effects
                .iter()
                .map(|intent| {
                    Hash::from_bytes(&intent.intent_hash)
                        .expect("intent hash is 32 bytes")
                        .to_hex()
                })
                .collect(),
            queued_effects: queued_effects
                .into_iter()
                .map(|intent| QueuedEffectDebugState {
                    intent_hash: Hash::from_bytes(&intent.intent_hash)
                        .expect("intent hash is 32 bytes")
                        .to_hex(),
                    effect_kind: intent.kind,
                    cap_name: intent.cap_name,
                })
                .collect(),
            workflow_instances: workflow_instances
                .into_iter()
                .filter(|instance| !instance.inflight_intents.is_empty())
                .map(|instance| ActiveWorkflowDebugState {
                    instance_id: instance.instance_id,
                    inflight_intent_hashes: instance
                        .inflight_intents
                        .into_iter()
                        .map(|intent| {
                            Hash::from_bytes(&intent.intent_id)
                                .expect("intent hash is 32 bytes")
                                .to_hex()
                        })
                        .collect(),
                })
                .collect(),
        })
    }

    pub async fn run_once(&mut self) -> Result<SupervisorOutcome, WorkerError> {
        let now_ns = now_wallclock_ns();
        self.evict_expired_warm_worlds(now_ns);
        self.maintenance
            .run_due(&*self.runtime, &self.worker.config, now_ns)?;
        self.runtime.heartbeat_worker(WorkerHeartbeat {
            worker_id: self.worker.config.worker_id.clone(),
            pins: self.worker.config.worker_pins.iter().cloned().collect(),
            last_seen_ns: now_ns,
            expires_at_ns: now_ns.saturating_add(duration_ns(self.worker.config.heartbeat_ttl)),
        })?;

        let active_workers = self
            .runtime
            .list_active_workers(now_ns, self.worker.config.world_scan_limit)?;
        let worlds = self.supervisor_candidates(now_ns)?;

        let mut outcome = SupervisorOutcome::default();

        for hosted in &worlds {
            let key = ActiveWorldRef {
                universe_id: hosted.universe_id,
                world_id: hosted.info.world_id,
            };
            let info = &hosted.info;
            if info.meta.active_baseline_height.is_none() {
                continue;
            }
            if self.active_worlds.contains_key(&key) && !self.worker_is_eligible_for_world(info) {
                self.drop_active_world(key, info.world_id)?;
                self.warm_worlds.remove(&key);
                outcome.worlds_released += 1;
                continue;
            }
            if !self.should_consider_world(key, info, now_ns) {
                continue;
            }
            if !self.should_retry_faulted_world(key, now_ns) {
                continue;
            }
            if !self.active_worlds.contains_key(&key)
                && self.should_own_world(key.universe_id, info, &active_workers)
            {
                match self.try_start_world(key, info.world_id, now_ns) {
                    Ok(Some(runner)) => {
                        self.clear_world_fault(key);
                        self.active_worlds.insert(key, runner);
                        outcome.worlds_started += 1;
                    }
                    Ok(None) => {}
                    Err(err) if is_world_isolatable_error(&err) => {
                        self.warm_worlds.remove(&key);
                        if !self.finalize_faulted_terminal_admin(key)? {
                            self.record_world_fault(key, &err, now_ns);
                            outcome.worlds_fenced += 1;
                        }
                    }
                    Err(err) => return Err(err),
                }
            }
        }

        let active_ids: Vec<_> = self.active_worlds.keys().copied().collect();
        let mut to_remove = Vec::new();
        for world_ref in active_ids {
            let step_result = {
                let runner = self
                    .active_worlds
                    .get_mut(&world_ref)
                    .expect("runner exists");
                runner.step().await
            };
            match step_result {
                Ok(RunnerStep::KeepRunning) => {}
                Ok(RunnerStep::Released(release)) => {
                    to_remove.push((world_ref, release));
                    outcome.worlds_released += 1;
                }
                Ok(RunnerStep::Fenced) => {
                    to_remove.push((world_ref, ReleaseDisposition::Drop));
                    outcome.worlds_fenced += 1;
                }
                Err(err) if is_world_isolatable_error(&err) => {
                    if let Some(runner) = self.active_worlds.remove(&world_ref) {
                        if let Some(lease) = runner.lease.as_ref() {
                            self.release_world_lease_after_fault(world_ref, lease)?;
                        }
                    }
                    if !self.finalize_faulted_terminal_admin(world_ref)? {
                        self.warm_worlds.remove(&world_ref);
                        self.record_world_fault(world_ref, &err, now_ns);
                        outcome.worlds_fenced += 1;
                    }
                }
                Err(err) => return Err(err),
            }
        }
        for (world_ref, release) in to_remove {
            if let Some(mut runner) = self.active_worlds.remove(&world_ref) {
                match release {
                    ReleaseDisposition::Warm => {
                        runner.suspend();
                        self.warm_worlds.insert(world_ref, runner);
                    }
                    ReleaseDisposition::Drop => {}
                }
            }
        }
        outcome.active_worlds = self.active_worlds.len();
        Ok(outcome)
    }

    pub fn run_once_blocking(&mut self) -> Result<SupervisorOutcome, WorkerError> {
        let now_ns = now_wallclock_ns();
        self.evict_expired_warm_worlds(now_ns);
        self.maintenance
            .run_due(&*self.runtime, &self.worker.config, now_ns)?;
        self.runtime.heartbeat_worker(WorkerHeartbeat {
            worker_id: self.worker.config.worker_id.clone(),
            pins: self.worker.config.worker_pins.iter().cloned().collect(),
            last_seen_ns: now_ns,
            expires_at_ns: now_ns.saturating_add(duration_ns(self.worker.config.heartbeat_ttl)),
        })?;

        let active_workers = self
            .runtime
            .list_active_workers(now_ns, self.worker.config.world_scan_limit)?;
        let worlds = self.supervisor_candidates(now_ns)?;

        let mut outcome = SupervisorOutcome::default();

        for hosted in &worlds {
            let key = ActiveWorldRef {
                universe_id: hosted.universe_id,
                world_id: hosted.info.world_id,
            };
            let info = &hosted.info;
            if info.meta.active_baseline_height.is_none() {
                continue;
            }
            if self.active_worlds.contains_key(&key) && !self.worker_is_eligible_for_world(info) {
                self.drop_active_world(key, info.world_id)?;
                self.warm_worlds.remove(&key);
                outcome.worlds_released += 1;
                continue;
            }
            if !self.should_consider_world(key, info, now_ns) {
                continue;
            }
            if !self.should_retry_faulted_world(key, now_ns) {
                continue;
            }
            if !self.active_worlds.contains_key(&key)
                && self.should_own_world(key.universe_id, info, &active_workers)
            {
                match self.try_start_world(key, info.world_id, now_ns) {
                    Ok(Some(runner)) => {
                        self.clear_world_fault(key);
                        self.active_worlds.insert(key, runner);
                        outcome.worlds_started += 1;
                    }
                    Ok(None) => {}
                    Err(err) if is_world_isolatable_error(&err) => {
                        self.warm_worlds.remove(&key);
                        if !self.finalize_faulted_terminal_admin(key)? {
                            self.record_world_fault(key, &err, now_ns);
                            outcome.worlds_fenced += 1;
                        }
                    }
                    Err(err) => return Err(err),
                }
            }
        }

        let active_ids: Vec<_> = self.active_worlds.keys().copied().collect();
        let mut to_remove = Vec::new();
        for world_ref in active_ids {
            let step_result = {
                let runner = self
                    .active_worlds
                    .get_mut(&world_ref)
                    .expect("runner exists");
                runner.step_blocking()
            };
            match step_result {
                Ok(RunnerStep::KeepRunning) => {}
                Ok(RunnerStep::Released(release)) => {
                    to_remove.push((world_ref, release));
                    outcome.worlds_released += 1;
                }
                Ok(RunnerStep::Fenced) => {
                    to_remove.push((world_ref, ReleaseDisposition::Drop));
                    outcome.worlds_fenced += 1;
                }
                Err(err) if is_world_isolatable_error(&err) => {
                    if let Some(runner) = self.active_worlds.remove(&world_ref) {
                        if let Some(lease) = runner.lease.as_ref() {
                            self.release_world_lease_after_fault(world_ref, lease)?;
                        }
                    }
                    if !self.finalize_faulted_terminal_admin(world_ref)? {
                        self.warm_worlds.remove(&world_ref);
                        self.record_world_fault(world_ref, &err, now_ns);
                        outcome.worlds_fenced += 1;
                    }
                }
                Err(err) => return Err(err),
            }
        }
        for (world_ref, release) in to_remove {
            if let Some(mut runner) = self.active_worlds.remove(&world_ref) {
                match release {
                    ReleaseDisposition::Warm => {
                        runner.suspend();
                        self.warm_worlds.insert(world_ref, runner);
                    }
                    ReleaseDisposition::Drop => {}
                }
            }
        }
        outcome.active_worlds = self.active_worlds.len();
        Ok(outcome)
    }

    fn try_start_world(
        &mut self,
        key: ActiveWorldRef,
        world_id: WorldId,
        now_ns: u64,
    ) -> Result<Option<WorldRunner<P>>, WorkerError> {
        if let Some(mut runner) = self.warm_worlds.remove(&key) {
            match self.runtime.acquire_world_lease(
                key.universe_id,
                world_id,
                &self.worker.config.worker_id,
                now_ns,
                duration_ns(self.worker.config.lease_ttl),
            ) {
                Ok(lease) => match runner.resume(lease.clone()) {
                    Ok(()) => return Ok(Some(runner)),
                    Err(err) => {
                        self.release_world_lease_after_fault(key, &lease)?;
                        return Err(err);
                    }
                },
                Err(PersistError::Conflict(conflict)) => {
                    self.warm_worlds.insert(key, runner);
                    tracing::debug!(
                        worker_id = %self.worker.config.worker_id,
                        universe_id = %key.universe_id,
                        world_id = %world_id,
                        error = %conflict,
                        "skipping warm world because lease acquisition conflicted"
                    );
                    return Ok(None);
                }
                Err(err) => {
                    self.warm_worlds.insert(key, runner);
                    tracing::error!(
                        worker_id = %self.worker.config.worker_id,
                        universe_id = %key.universe_id,
                        world_id = %world_id,
                        error = %err,
                        "failed to resume warm world"
                    );
                    return Err(err.into());
                }
            }
        }
        match self.runtime.acquire_world_lease(
            key.universe_id,
            world_id,
            &self.worker.config.worker_id,
            now_ns,
            duration_ns(self.worker.config.lease_ttl),
        ) {
            Ok(lease) => {
                let runner = WorldRunner::open(
                    self.worker.clone(),
                    Arc::clone(&self.runtime),
                    key.universe_id,
                    world_id,
                    lease,
                );
                match runner {
                    Ok(runner) => Ok(Some(runner)),
                    Err(err) => {
                        if let Some(lease) = self
                            .runtime
                            .current_world_lease(key.universe_id, world_id)?
                            && lease.holder_worker_id == self.worker.config.worker_id
                        {
                            self.release_world_lease_after_fault(key, &lease)?;
                        }
                        Err(err)
                    }
                }
            }
            Err(PersistError::Conflict(PersistConflict::LeaseHeld {
                holder_worker_id, ..
            })) if holder_worker_id == self.worker.config.worker_id => {
                let Some(lease) = self
                    .runtime
                    .current_world_lease(key.universe_id, world_id)?
                else {
                    return Ok(None);
                };
                let runner = WorldRunner::open(
                    self.worker.clone(),
                    Arc::clone(&self.runtime),
                    key.universe_id,
                    world_id,
                    lease,
                );
                match runner {
                    Ok(runner) => Ok(Some(runner)),
                    Err(err) => {
                        if let Some(lease) = self
                            .runtime
                            .current_world_lease(key.universe_id, world_id)?
                            && lease.holder_worker_id == self.worker.config.worker_id
                        {
                            self.release_world_lease_after_fault(key, &lease)?;
                        }
                        Err(err)
                    }
                }
            }
            Err(PersistError::Conflict(conflict)) => {
                tracing::debug!(
                    worker_id = %self.worker.config.worker_id,
                    universe_id = %key.universe_id,
                    world_id = %world_id,
                    error = %conflict,
                    "skipping world because lease acquisition conflicted"
                );
                Ok(None)
            }
            Err(err) => {
                tracing::error!(
                    worker_id = %self.worker.config.worker_id,
                    universe_id = %key.universe_id,
                    world_id = %world_id,
                    error = %err,
                    "failed to start world"
                );
                Err(err.into())
            }
        }
    }

    fn evict_expired_warm_worlds(&mut self, now_ns: u64) {
        let ttl_ns = duration_ns(self.worker.config.warm_retain_after);
        if ttl_ns == 0 {
            self.warm_worlds.clear();
            return;
        }
        self.warm_worlds.retain(|_, runner| {
            runner
                .suspended_since_ns
                .is_none_or(|since| now_ns.saturating_sub(since) < ttl_ns)
        });
    }

    fn drop_active_world(
        &mut self,
        key: ActiveWorldRef,
        world_id: WorldId,
    ) -> Result<(), WorkerError> {
        if let Some(runner) = self.active_worlds.remove(&key)
            && let Some(lease) = &runner.lease
        {
            match self
                .runtime
                .release_world_lease(key.universe_id, world_id, lease)
            {
                Ok(()) | Err(PersistError::Conflict(_)) => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    pub(super) fn should_consider_world(
        &self,
        world_ref: ActiveWorldRef,
        info: &WorldRuntimeInfo,
        now_ns: u64,
    ) -> bool {
        info.has_pending_inbox
            || info.has_pending_effects
            || info
                .next_timer_due_at_ns
                .is_some_and(|due_at| due_at <= now_ns)
            || info.has_pending_maintenance
            || self.active_worlds.contains_key(&world_ref)
            || info
                .lease
                .as_ref()
                .is_some_and(|lease| lease.holder_worker_id == self.worker.config.worker_id)
    }

    pub(super) fn should_own_world(
        &self,
        universe: UniverseId,
        info: &WorldRuntimeInfo,
        workers: &[WorkerHeartbeat],
    ) -> bool {
        if !info.meta.admin.status.allows_new_leases() {
            return false;
        }
        let effective_pin = effective_world_pin(info);
        let eligible_workers: Vec<_> = workers
            .iter()
            .filter(|worker| worker_is_eligible_for_pin(worker, &effective_pin))
            .collect();
        if eligible_workers.is_empty() {
            return false;
        }
        let mut best_worker = None::<(&str, u64)>;
        for worker in eligible_workers {
            let score = rendezvous_score(universe, info.world_id, &worker.worker_id);
            if best_worker.is_none_or(|(_, best_score)| score > best_score) {
                best_worker = Some((worker.worker_id.as_str(), score));
            }
        }
        best_worker
            .map(|(worker_id, _)| worker_id == self.worker.config.worker_id)
            .unwrap_or(true)
    }

    fn worker_is_eligible_for_world(&self, info: &WorldRuntimeInfo) -> bool {
        let effective_pin = effective_world_pin(info);
        self.worker
            .config
            .worker_pins
            .contains(effective_pin.as_str())
    }

    fn configured_universe_filter(&self) -> Option<Vec<UniverseId>> {
        (!self.worker.config.universe_filter.is_empty()).then(|| {
            self.worker
                .config
                .universe_filter
                .iter()
                .copied()
                .collect::<Vec<_>>()
        })
    }

    fn supervisor_candidates(&self, now_ns: u64) -> Result<Vec<NodeWorldRuntimeInfo>, WorkerError> {
        let mut worlds = HashMap::new();
        let universe_filter = self.configured_universe_filter();
        let filter = universe_filter.as_deref();

        for info in
            self.runtime
                .list_ready_worlds(now_ns, self.worker.config.ready_scan_limit, filter)?
        {
            worlds.insert(
                ActiveWorldRef {
                    universe_id: info.universe_id,
                    world_id: info.info.world_id,
                },
                info,
            );
        }

        for info in self.runtime.list_worker_worlds(
            &self.worker.config.worker_id,
            now_ns,
            self.worker.config.world_scan_limit,
            filter,
        )? {
            worlds.insert(
                ActiveWorldRef {
                    universe_id: info.universe_id,
                    world_id: info.info.world_id,
                },
                info,
            );
        }

        for world_ref in self.active_worlds.keys().copied() {
            let info = self.runtime.world_runtime_info(
                world_ref.universe_id,
                world_ref.world_id,
                now_ns,
            )?;
            worlds.insert(
                world_ref,
                NodeWorldRuntimeInfo {
                    universe_id: world_ref.universe_id,
                    info,
                },
            );
        }

        let mut worlds: Vec<_> = worlds.into_values().collect();
        worlds.sort_by_key(|entry| (entry.universe_id, entry.info.world_id));
        Ok(worlds)
    }
}
