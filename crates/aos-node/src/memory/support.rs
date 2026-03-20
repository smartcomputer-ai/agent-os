use super::*;
use crate::memory::state::MemoryPersistenceSnapshot;

impl MemoryWorldPersistence {
    pub fn new() -> Self {
        Self::with_config(PersistenceConfig::default())
    }

    pub fn with_config(config: PersistenceConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(MemoryState::default())),
            cas: MemoryCasStore::new(config.cas),
            config,
        }
    }

    pub fn from_snapshot(config: PersistenceConfig, snapshot: &[u8]) -> Result<Self, PersistError> {
        let snapshot: MemoryPersistenceSnapshot = serde_cbor::from_slice(snapshot)
            .map_err(|err| PersistError::backend(err.to_string()))?;
        Ok(Self {
            state: Arc::new(Mutex::new(snapshot.state)),
            cas: MemoryCasStore::from_state(config.cas, snapshot.cas_state)?,
            config,
        })
    }

    pub fn export_snapshot(&self) -> Result<Vec<u8>, PersistError> {
        let state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?
            .clone();
        let snapshot = MemoryPersistenceSnapshot {
            state,
            cas_state: self.cas.export_state()?,
        };
        serde_cbor::to_vec(&snapshot).map_err(|err| PersistError::backend(err.to_string()))
    }

    pub fn cas(&self) -> &MemoryCasStore {
        &self.cas
    }

    pub(super) fn ensure_universe_record(
        state: &mut MemoryState,
        universe: UniverseId,
        created_at_ns: u64,
    ) -> Result<UniverseRecord, PersistError> {
        let record = if let Some(record) = state.universes.get(&universe).cloned() {
            record
        } else {
            let handle = default_universe_handle(universe);
            Self::ensure_universe_handle_available(state, universe, &handle)?;
            let record = UniverseRecord {
                universe_id: universe,
                created_at_ns,
                meta: UniverseMeta {
                    handle: handle.clone(),
                },
                admin: UniverseAdminLifecycle::default(),
            };
            state.universes.insert(universe, record.clone());
            state.universe_handles.insert(handle, universe);
            record
        };
        if matches!(record.admin.status, UniverseAdminStatus::Deleted) {
            return Err(PersistConflict::UniverseAdminBlocked {
                universe_id: universe,
                status: record.admin.status,
                action: "create_world".into(),
            }
            .into());
        }
        Ok(record)
    }

    pub(super) fn ensure_universe_handle_available(
        state: &MemoryState,
        universe: UniverseId,
        handle: &str,
    ) -> Result<(), PersistError> {
        if let Some(existing) = state.universe_handles.get(handle) {
            if *existing != universe
                && !state.universes.get(existing).is_some_and(|record| {
                    matches!(record.admin.status, UniverseAdminStatus::Deleted)
                })
            {
                return Err(PersistConflict::UniverseHandleExists {
                    handle: handle.to_string(),
                    universe_id: *existing,
                }
                .into());
            }
        }
        Ok(())
    }

    pub(super) fn ensure_world_handle_available(
        state: &MemoryState,
        universe: UniverseId,
        world: WorldId,
        handle: &str,
    ) -> Result<(), PersistError> {
        if let Some(existing) = state
            .world_handles
            .get(&universe)
            .and_then(|handles| handles.get(handle))
        {
            if *existing != world
                && !state
                    .worlds
                    .get(&(universe, *existing))
                    .is_some_and(|world_state| {
                        matches!(world_state.meta.admin.status, WorldAdminStatus::Deleted)
                    })
            {
                return Err(PersistConflict::WorldHandleExists {
                    universe_id: universe,
                    handle: handle.to_string(),
                    world_id: *existing,
                }
                .into());
            }
        }
        Ok(())
    }

    pub(super) fn ensure_secret_universe_record(
        state: &mut MemoryState,
        universe: UniverseId,
    ) -> Result<(), PersistError> {
        let Some(record) = state.universes.get(&universe) else {
            return Err(PersistError::not_found(format!("universe {universe}")));
        };
        if matches!(record.admin.status, UniverseAdminStatus::Deleted) {
            return Err(PersistConflict::UniverseAdminBlocked {
                universe_id: universe,
                status: record.admin.status,
                action: "manage_secret".into(),
            }
            .into());
        }
        Ok(())
    }

    pub(super) fn validate_secret_binding(
        record: &SecretBindingRecord,
    ) -> Result<(), PersistError> {
        if record.binding_id.trim().is_empty() {
            return Err(PersistError::validation("binding_id must be non-empty"));
        }
        if matches!(record.source_kind, SecretBindingSourceKind::WorkerEnv)
            && record
                .env_var
                .as_ref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(PersistError::validation(
                "worker_env bindings require a non-empty env_var",
            ));
        }
        Ok(())
    }

    pub(super) fn with_world_mut<R>(
        &self,
        universe: UniverseId,
        world: WorldId,
        f: impl FnOnce(&mut WorldState) -> Result<R, PersistError>,
    ) -> Result<R, PersistError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let world_state = guard.worlds.entry((universe, world)).or_default();
        f(world_state)
    }

    pub(super) fn with_world<R>(
        &self,
        universe: UniverseId,
        world: WorldId,
        f: impl FnOnce(&WorldState) -> Result<R, PersistError>,
    ) -> Result<R, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let world_state = guard.worlds.get(&(universe, world)).ok_or_else(|| {
            PersistError::not_found(format!("world {world} in universe {universe}"))
        })?;
        f(world_state)
    }

    #[cfg(test)]
    pub(super) fn debug_remove_journal_entry(
        &self,
        universe: UniverseId,
        world: WorldId,
        height: JournalHeight,
    ) {
        let mut guard = self.state.lock().unwrap();
        if let Some(world_state) = guard.worlds.get_mut(&(universe, world)) {
            world_state.journal_entries.remove(&height);
        }
    }

    pub(super) fn validate_journal_batch(&self, entries: &[Vec<u8>]) -> Result<(), PersistError> {
        if entries.is_empty() {
            return Err(PersistError::validation(
                "journal append batch cannot be empty",
            ));
        }
        if entries.len() > self.config.journal.max_batch_entries {
            return Err(PersistError::validation(format!(
                "journal append batch entry count {} exceeds limit {}",
                entries.len(),
                self.config.journal.max_batch_entries
            )));
        }
        let total_bytes: usize = entries.iter().map(|entry| entry.len()).sum();
        if total_bytes > self.config.journal.max_batch_bytes {
            return Err(PersistError::validation(format!(
                "journal append batch bytes {} exceeds limit {}",
                total_bytes, self.config.journal.max_batch_bytes
            )));
        }
        Ok(())
    }

    pub(super) fn normalize_payload(
        &self,
        universe: UniverseId,
        payload: &mut CborPayload,
    ) -> Result<(), PersistError> {
        payload.validate()?;
        if let Some(bytes) = payload.inline_cbor.take() {
            if bytes.len() > self.config.inbox.inline_payload_threshold_bytes {
                let hash = self.cas_put_verified(universe, &bytes)?;
                *payload = CborPayload::externalized(hash, bytes.len() as u64);
            } else {
                payload.inline_cbor = Some(bytes);
            }
        }
        Ok(())
    }

    pub(super) fn normalize_inbox_item(
        &self,
        universe: UniverseId,
        mut item: InboxItem,
    ) -> Result<InboxItem, PersistError> {
        match &mut item {
            InboxItem::DomainEvent(ingress) => {
                self.normalize_payload(universe, &mut ingress.value)?
            }
            InboxItem::Receipt(ingress) => {
                self.normalize_payload(universe, &mut ingress.payload)?
            }
            InboxItem::Inbox(ingress) => self.normalize_payload(universe, &mut ingress.payload)?,
            InboxItem::TimerFired(ingress) => {
                self.normalize_payload(universe, &mut ingress.payload)?
            }
            InboxItem::Control(ingress) => {
                self.normalize_payload(universe, &mut ingress.payload)?
            }
        }
        Ok(item)
    }

    pub(super) fn normalize_command_record(
        &self,
        universe: UniverseId,
        mut record: CommandRecord,
    ) -> Result<CommandRecord, PersistError> {
        if let Some(payload) = record.result_payload.as_mut() {
            self.normalize_payload(universe, payload)?;
        }
        Ok(record)
    }

    pub(super) fn normalize_effect_dispatch_item(
        &self,
        universe: UniverseId,
        mut item: EffectDispatchItem,
    ) -> Result<EffectDispatchItem, PersistError> {
        if let Some(bytes) = item.params_inline_cbor.take() {
            if bytes.len() > self.config.inbox.inline_payload_threshold_bytes {
                let hash = self.cas_put_verified(universe, &bytes)?;
                item.params_ref = Some(hash.to_hex());
                item.params_size = Some(bytes.len() as u64);
                item.params_sha256 = Some(hash.to_hex());
            } else {
                item.params_inline_cbor = Some(bytes);
            }
        }
        Ok(item)
    }

    pub(super) fn next_effect_seq(
        &self,
        state: &mut MemoryState,
        universe: UniverseId,
        shard: u16,
    ) -> QueueSeq {
        let shard_counters = state.effect_seq_by_shard.entry(universe).or_default();
        let next = shard_counters.entry(shard).or_default();
        let seq = QueueSeq::from_u64(*next);
        *next = next.saturating_add(1);
        seq
    }

    pub(super) fn recompute_next_timer_due(
        world: WorldId,
        state: &MemoryState,
        universe: UniverseId,
    ) -> Option<u64> {
        state
            .timers_due
            .get(&universe)
            .into_iter()
            .flat_map(|entries| entries.values())
            .filter(|item| item.world_id == world)
            .map(|item| item.deliver_at_ns)
            .min()
    }

    pub(super) fn world_runtime_info_from_state(
        world_id: WorldId,
        world_state: &WorldState,
        now_ns: u64,
    ) -> WorldRuntimeInfo {
        let _ = now_ns;
        WorldRuntimeInfo {
            world_id,
            meta: world_state.meta.clone(),
            notify_counter: world_state.notify_counter,
            has_pending_inbox: world_state.ready_state.has_pending_inbox,
            has_pending_effects: world_state.ready_state.has_pending_effects,
            next_timer_due_at_ns: world_state.ready_state.next_timer_due_at_ns,
            has_pending_maintenance: world_state.ready_state.has_pending_maintenance,
            lease: world_state.lease.clone(),
        }
    }

    pub(super) fn ready_shard(world: WorldId) -> u16 {
        let uuid = world.as_uuid();
        let bytes = uuid.as_bytes();
        u16::from_be_bytes([bytes[0], bytes[1]])
    }

    pub(super) fn recompute_ready_state(
        world_state: &WorldState,
        config: PersistenceConfig,
    ) -> ReadyState {
        let has_pending_inbox = world_state
            .inbox_entries
            .iter()
            .next_back()
            .is_some_and(|(latest, _)| world_state.inbox_cursor.as_ref() != Some(latest));
        let first_hot_journal_height = world_state.journal_entries.keys().next().copied();
        ReadyState {
            has_pending_inbox,
            has_pending_effects: world_state.pending_effects_count > 0,
            next_timer_due_at_ns: world_state.next_timer_due_at_ns,
            has_pending_maintenance: world_state.meta.admin.status.requires_maintenance_wakeup()
                || maintenance_due(
                    world_state.journal_head,
                    world_state
                        .active_baseline
                        .as_ref()
                        .map(|record| record.height),
                    first_hot_journal_height,
                    config.snapshot_maintenance,
                ),
        }
    }

    pub(super) fn refresh_ready_hint(
        state: &mut MemoryState,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) {
        let ready_state = match state.worlds.get(&(universe, world)) {
            Some(world_state) => world_state.ready_state.clone(),
            None => return,
        };
        let shard = Self::ready_shard(world);
        state
            .ready_hints
            .retain(|(_, _, existing_universe, existing_world), _| {
                *existing_universe != universe || *existing_world != world
            });
        if ready_state.is_ready() {
            let priority = ready_state.priority(now_ns);
            state.ready_hints.insert(
                (priority, shard, universe, world),
                ReadyHint {
                    world_id: world,
                    priority,
                    ready_state,
                    updated_at_ns: now_ns,
                },
            );
        }
    }

    pub(super) fn sync_ready_state(
        state: &mut MemoryState,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
        config: PersistenceConfig,
    ) {
        if let Some(world_state) = state.worlds.get_mut(&(universe, world)) {
            world_state.ready_state = Self::recompute_ready_state(world_state, config);
        }
        Self::refresh_ready_hint(state, universe, world, now_ns);
    }

    pub(super) fn effect_terminal_record(
        &self,
        status: DispatchStatus,
        now_ns: u64,
    ) -> EffectDedupeRecord {
        let gc_after_ns = now_ns.saturating_add(self.config.dedupe_gc.effect_retention_ns);
        EffectDedupeRecord {
            status,
            completed_at_ns: Some(now_ns),
            gc_after_ns: Some(gc_after_ns),
        }
    }

    pub(super) fn timer_terminal_record(
        &self,
        status: DeliveredStatus,
        now_ns: u64,
    ) -> TimerDedupeRecord {
        let gc_after_ns = now_ns.saturating_add(self.config.dedupe_gc.timer_retention_ns);
        TimerDedupeRecord {
            status,
            completed_at_ns: Some(now_ns),
            gc_after_ns: Some(gc_after_ns),
        }
    }

    pub(super) fn portal_terminal_record(
        &self,
        enqueued_seq: Option<InboxSeq>,
        now_ns: u64,
    ) -> PortalDedupeRecord {
        let gc_after_ns = now_ns.saturating_add(self.config.dedupe_gc.portal_retention_ns);
        PortalDedupeRecord {
            enqueued_seq,
            completed_at_ns: Some(now_ns),
            gc_after_ns: Some(gc_after_ns),
        }
    }

    pub(super) fn update_meta_from_baseline(world_state: &mut WorldState, record: &SnapshotRecord) {
        world_state.meta.active_baseline_height = Some(record.height);
        world_state.meta.manifest_hash = record.manifest_hash.clone();
    }

    pub(super) fn apply_query_projection_materialization(
        world_state: &mut WorldState,
        materialization: QueryProjectionMaterialization,
    ) {
        world_state.head_projection = Some(materialization.head);
        world_state.cell_state_projections = materialization
            .workflows
            .into_iter()
            .map(|workflow| {
                (
                    workflow.workflow,
                    workflow
                        .cells
                        .into_iter()
                        .map(|cell| (cell.key_hash.clone(), cell))
                        .collect(),
                )
            })
            .collect();
        world_state.workspace_projections = materialization
            .workspaces
            .into_iter()
            .map(|workspace| (workspace.workspace.clone(), workspace))
            .collect();
    }

    pub(super) fn apply_query_projection_delta(
        world_state: &mut WorldState,
        delta: QueryProjectionDelta,
    ) {
        world_state.head_projection = Some(delta.head);
        for cell in delta.cell_upserts {
            world_state
                .cell_state_projections
                .entry(cell.workflow.clone())
                .or_default()
                .insert(cell.key_hash.clone(), cell);
        }
        for cell in delta.cell_deletes {
            let remove_workflow =
                if let Some(cells) = world_state.cell_state_projections.get_mut(&cell.workflow) {
                    cells.remove(&cell.key_hash);
                    cells.is_empty()
                } else {
                    false
                };
            if remove_workflow {
                world_state.cell_state_projections.remove(&cell.workflow);
            }
        }
        for workspace in delta.workspace_upserts {
            world_state
                .workspace_projections
                .insert(workspace.workspace.clone(), workspace);
        }
        for workspace in delta.workspace_deletes {
            world_state
                .workspace_projections
                .remove(&workspace.workspace);
        }
    }

    pub(super) fn resolve_cas_hash(reference: &str, field: &str) -> Result<Hash, PersistError> {
        Hash::from_hex_str(reference).map_err(|err| {
            PersistError::validation(format!(
                "invalid {field} hash reference '{reference}': {err}"
            ))
        })
    }

    pub(super) fn validate_seed_cas_roots(
        cas: &MemoryCasStore,
        universe: UniverseId,
        seed: &WorldSeed,
    ) -> Result<(), PersistError> {
        let snapshot_hash = Self::resolve_cas_hash(&seed.baseline.snapshot_ref, "snapshot_ref")?;
        let manifest_ref = seed
            .baseline
            .manifest_hash
            .as_deref()
            .ok_or_else(|| PersistError::validation("seed baseline requires manifest_hash"))?;
        let manifest_hash = Self::resolve_cas_hash(manifest_ref, "manifest_hash")?;
        if !cas.has(universe, snapshot_hash)? {
            return Err(PersistError::not_found(format!(
                "snapshot {} in universe {}",
                seed.baseline.snapshot_ref, universe
            )));
        }
        if !cas.has(universe, manifest_hash)? {
            return Err(PersistError::not_found(format!(
                "manifest {} in universe {}",
                manifest_ref, universe
            )));
        }
        Ok(())
    }

    pub(super) fn lineage_from_seed(created_at_ns: u64, seed: &WorldSeed) -> WorldLineage {
        match &seed.imported_from {
            Some(imported_from) => WorldLineage::Import {
                created_at_ns,
                source: imported_from.source.clone(),
                external_world_id: imported_from.external_world_id.clone(),
                external_snapshot_ref: imported_from.external_snapshot_ref.clone(),
            },
            None => WorldLineage::Genesis { created_at_ns },
        }
    }

    pub(super) fn create_world_state_from_seed(
        state: &mut MemoryState,
        cas: &MemoryCasStore,
        config: PersistenceConfig,
        universe: UniverseId,
        world: WorldId,
        seed: &WorldSeed,
        handle: String,
        placement_pin: Option<String>,
        created_at_ns: u64,
        lineage: WorldLineage,
    ) -> Result<WorldRecord, PersistError> {
        Self::validate_seed_cas_roots(cas, universe, seed)?;
        if state.worlds.contains_key(&(universe, world)) {
            return Err(PersistConflict::WorldExists { world_id: world }.into());
        }
        Self::ensure_world_handle_available(state, universe, world, &handle)?;

        let mut world_state = WorldState::default();
        world_state.meta.handle = handle.clone();
        world_state.journal_head = seed.baseline.height.saturating_add(1);
        world_state
            .snapshots
            .insert(seed.baseline.height, seed.baseline.clone());
        world_state.active_baseline = Some(seed.baseline.clone());
        world_state.meta.created_at_ns = created_at_ns;
        world_state.meta.placement_pin = placement_pin;
        world_state.meta.lineage = Some(lineage);
        Self::update_meta_from_baseline(&mut world_state, &seed.baseline);
        let snapshot_hash = Self::resolve_cas_hash(&seed.baseline.snapshot_ref, "snapshot_ref")?;
        let snapshot_bytes = cas.get(universe, snapshot_hash)?;
        for (state_hash, state_bytes) in state_blobs_from_snapshot(&snapshot_bytes)? {
            let stored = cas.put_verified(universe, &state_bytes)?;
            if stored != state_hash {
                return Err(PersistError::backend(format!(
                    "snapshot state hash mismatch: expected {}, stored {}",
                    state_hash.to_hex(),
                    stored.to_hex()
                )));
            }
        }
        let materialization =
            materialization_from_snapshot(&seed.baseline, &snapshot_bytes, created_at_ns)?;
        Self::apply_query_projection_materialization(&mut world_state, materialization);
        world_state.ready_state = Self::recompute_ready_state(&world_state, config);

        let record = WorldRecord {
            world_id: world,
            meta: world_state.meta.clone(),
            active_baseline: seed.baseline.clone(),
            journal_head: world_state.journal_head,
        };
        state.worlds.insert((universe, world), world_state);
        state
            .world_handles
            .entry(universe)
            .or_default()
            .insert(handle, world);
        Self::sync_ready_state(state, universe, world, 0, config);
        Ok(record)
    }

    pub(super) fn resolve_snapshot_selector_from_state(
        world_state: &WorldState,
        selector: &SnapshotSelector,
    ) -> Result<SnapshotRecord, PersistError> {
        match selector {
            SnapshotSelector::ActiveBaseline => world_state
                .active_baseline
                .clone()
                .ok_or_else(|| PersistError::not_found("active baseline")),
            SnapshotSelector::ByHeight { height } => world_state
                .snapshots
                .get(height)
                .cloned()
                .ok_or_else(|| PersistError::not_found(format!("snapshot at height {height}"))),
            SnapshotSelector::ByRef { snapshot_ref } => world_state
                .snapshots
                .values()
                .find(|record| record.snapshot_ref == *snapshot_ref)
                .cloned()
                .ok_or_else(|| PersistError::not_found(format!("snapshot {snapshot_ref}"))),
        }
    }

    pub(super) fn seed_for_fork_policy(
        &self,
        universe: UniverseId,
        baseline: &SnapshotRecord,
        policy: &crate::ForkPendingEffectPolicy,
    ) -> Result<WorldSeed, PersistError> {
        let snapshot_hash = Hash::from_hex_str(&baseline.snapshot_ref).map_err(|err| {
            PersistError::validation(format!(
                "invalid snapshot_ref hash reference '{}': {err}",
                baseline.snapshot_ref
            ))
        })?;
        let snapshot_bytes = self.cas_get(universe, snapshot_hash)?;
        let snapshot_ref = match rewrite_snapshot_for_fork_policy(&snapshot_bytes, policy)? {
            Some(bytes) => self.cas_put_verified(universe, &bytes)?.to_hex(),
            None => baseline.snapshot_ref.clone(),
        };
        let mut seed = WorldSeed {
            baseline: baseline.clone(),
            seed_kind: crate::SeedKind::Import,
            imported_from: None,
        };
        seed.baseline.snapshot_ref = snapshot_ref;
        Ok(seed)
    }

    pub(super) fn assert_live_lease(
        world_state: &WorldState,
        lease: &WorldLease,
        now_ns: u64,
    ) -> Result<(), PersistError> {
        let Some(current) = &world_state.lease else {
            return Err(PersistConflict::LeaseMismatch {
                expected_worker_id: lease.holder_worker_id.clone(),
                expected_epoch: lease.epoch,
                actual_worker_id: None,
                actual_epoch: None,
            }
            .into());
        };
        if current.holder_worker_id != lease.holder_worker_id || current.epoch != lease.epoch {
            return Err(PersistConflict::LeaseMismatch {
                expected_worker_id: lease.holder_worker_id.clone(),
                expected_epoch: lease.epoch,
                actual_worker_id: Some(current.holder_worker_id.clone()),
                actual_epoch: Some(current.epoch),
            }
            .into());
        }
        if current.expires_at_ns < now_ns {
            return Err(PersistConflict::LeaseHeld {
                holder_worker_id: current.holder_worker_id.clone(),
                epoch: current.epoch,
                expires_at_ns: current.expires_at_ns,
            }
            .into());
        }
        Ok(())
    }

    pub(super) fn journal_append_batch_inner(
        &self,
        world_state: &mut WorldState,
        expected_head: JournalHeight,
        entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError> {
        if world_state.journal_head != expected_head {
            return Err(PersistConflict::HeadAdvanced {
                expected: expected_head,
                actual: world_state.journal_head,
            }
            .into());
        }
        let first_height = world_state.journal_head;
        for entry in entries {
            world_state
                .journal_entries
                .insert(world_state.journal_head, entry.clone());
            world_state.journal_head += 1;
        }
        Ok(first_height)
    }

    pub(super) fn inbox_commit_cursor_inner(
        world_state: &mut WorldState,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
    ) -> Result<(), PersistError> {
        if world_state.inbox_cursor != old_cursor {
            return Err(PersistConflict::InboxCursorAdvanced {
                expected: old_cursor,
                actual: world_state.inbox_cursor.clone(),
            }
            .into());
        }
        if let Some(current) = &world_state.inbox_cursor
            && new_cursor < *current
        {
            return Err(PersistError::validation("inbox cursor cannot regress"));
        }
        if !world_state.inbox_entries.contains_key(&new_cursor) {
            return Err(PersistError::not_found(format!(
                "inbox sequence {new_cursor} does not exist"
            )));
        }
        world_state.inbox_cursor = Some(new_cursor);
        Ok(())
    }
}
