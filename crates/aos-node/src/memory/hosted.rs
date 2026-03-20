use super::*;

impl ProjectionStore for MemoryWorldPersistence {
    fn head_projection(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<HeadProjectionRecord>, PersistError> {
        MemoryWorldPersistence::head_projection(self, universe, world)
    }

    fn cell_state_projection(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        key_hash: &[u8],
    ) -> Result<Option<CellStateProjectionRecord>, PersistError> {
        MemoryWorldPersistence::cell_state_projection(self, universe, world, workflow, key_hash)
    }

    fn list_cell_state_projections(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        after_key_hash: Option<Vec<u8>>,
        limit: u32,
    ) -> Result<Vec<CellStateProjectionRecord>, PersistError> {
        MemoryWorldPersistence::list_cell_state_projections(
            self,
            universe,
            world,
            workflow,
            after_key_hash,
            limit,
        )
    }

    fn workspace_projection(
        &self,
        universe: UniverseId,
        world: WorldId,
        workspace: &str,
    ) -> Result<Option<WorkspaceRegistryProjectionRecord>, PersistError> {
        MemoryWorldPersistence::workspace_projection(self, universe, world, workspace)
    }

    fn list_workspace_projections(
        &self,
        universe: UniverseId,
        world: WorldId,
        after_workspace: Option<String>,
        limit: u32,
    ) -> Result<Vec<WorkspaceRegistryProjectionRecord>, PersistError> {
        MemoryWorldPersistence::list_workspace_projections(
            self,
            universe,
            world,
            after_workspace,
            limit,
        )
    }

    fn bootstrap_query_projections(
        &self,
        universe: UniverseId,
        world: WorldId,
        materialization: QueryProjectionMaterialization,
    ) -> Result<(), PersistError> {
        MemoryWorldPersistence::bootstrap_query_projections(self, universe, world, materialization)
    }
}

impl HostedCoordinationStore for MemoryWorldPersistence {
    fn snapshot_maintenance_config(&self) -> crate::SnapshotMaintenanceConfig {
        MemoryWorldPersistence::snapshot_maintenance_config(self)
    }

    fn heartbeat_worker(&self, heartbeat: WorkerHeartbeat) -> Result<(), PersistError> {
        MemoryWorldPersistence::heartbeat_worker(self, heartbeat)
    }

    fn list_active_workers(
        &self,
        now_ns: u64,
        limit: u32,
    ) -> Result<Vec<WorkerHeartbeat>, PersistError> {
        MemoryWorldPersistence::list_active_workers(self, now_ns, limit)
    }

    fn list_ready_worlds(
        &self,
        now_ns: u64,
        limit: u32,
        universe_filter: Option<&[UniverseId]>,
    ) -> Result<Vec<NodeWorldRuntimeInfo>, PersistError> {
        MemoryWorldPersistence::list_ready_worlds(self, now_ns, limit, universe_filter)
    }

    fn current_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<WorldLease>, PersistError> {
        MemoryWorldPersistence::current_world_lease(self, universe, world)
    }

    fn acquire_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        now_ns: u64,
        lease_ttl_ns: u64,
    ) -> Result<WorldLease, PersistError> {
        MemoryWorldPersistence::acquire_world_lease(
            self,
            universe,
            world,
            worker_id,
            now_ns,
            lease_ttl_ns,
        )
    }

    fn renew_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        lease_ttl_ns: u64,
    ) -> Result<WorldLease, PersistError> {
        MemoryWorldPersistence::renew_world_lease(
            self,
            universe,
            world,
            lease,
            now_ns,
            lease_ttl_ns,
        )
    }

    fn release_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
    ) -> Result<(), PersistError> {
        MemoryWorldPersistence::release_world_lease(self, universe, world, lease)
    }

    fn list_worker_worlds(
        &self,
        worker_id: &str,
        now_ns: u64,
        limit: u32,
        universe_filter: Option<&[UniverseId]>,
    ) -> Result<Vec<NodeWorldRuntimeInfo>, PersistError> {
        MemoryWorldPersistence::list_worker_worlds(self, worker_id, now_ns, limit, universe_filter)
    }

    fn update_command_record_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        record: CommandRecord,
    ) -> Result<(), PersistError> {
        MemoryWorldPersistence::update_command_record_guarded(
            self, universe, world, lease, now_ns, record,
        )
    }

    fn journal_append_batch_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        expected_head: JournalHeight,
        entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError> {
        MemoryWorldPersistence::journal_append_batch_guarded(
            self,
            universe,
            world,
            lease,
            now_ns,
            expected_head,
            entries,
        )
    }

    fn inbox_commit_cursor_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
    ) -> Result<(), PersistError> {
        MemoryWorldPersistence::inbox_commit_cursor_guarded(
            self, universe, world, lease, now_ns, old_cursor, new_cursor,
        )
    }

    fn drain_inbox_to_journal_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
        expected_head: JournalHeight,
        journal_entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError> {
        MemoryWorldPersistence::drain_inbox_to_journal_guarded(
            self,
            universe,
            world,
            lease,
            now_ns,
            old_cursor,
            new_cursor,
            expected_head,
            journal_entries,
        )
    }

    fn materialize_query_projections_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        materialization: QueryProjectionMaterialization,
    ) -> Result<(), PersistError> {
        MemoryWorldPersistence::materialize_query_projections_guarded(
            self,
            universe,
            world,
            lease,
            now_ns,
            materialization,
        )
    }

    fn apply_query_projection_delta_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        delta: QueryProjectionDelta,
    ) -> Result<(), PersistError> {
        MemoryWorldPersistence::apply_query_projection_delta_guarded(
            self, universe, world, lease, now_ns, delta,
        )
    }

    fn snapshot_index_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        MemoryWorldPersistence::snapshot_index_guarded(self, universe, world, lease, now_ns, record)
    }

    fn snapshot_commit_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        request: SnapshotCommitRequest,
    ) -> Result<SnapshotCommitResult, PersistError> {
        MemoryWorldPersistence::snapshot_commit_guarded(
            self, universe, world, lease, now_ns, request,
        )
    }

    fn snapshot_promote_baseline_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        MemoryWorldPersistence::snapshot_promote_baseline_guarded(
            self, universe, world, lease, now_ns, record,
        )
    }

    fn segment_index_put_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        record: SegmentIndexRecord,
    ) -> Result<(), PersistError> {
        MemoryWorldPersistence::segment_index_put_guarded(
            self, universe, world, lease, now_ns, record,
        )
    }

    fn segment_export_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        request: SegmentExportRequest,
    ) -> Result<SegmentExportResult, PersistError> {
        MemoryWorldPersistence::segment_export_guarded(
            self, universe, world, lease, now_ns, request,
        )
    }
}

impl HostedEffectQueueStore for MemoryWorldPersistence {
    fn publish_effect_dispatches_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        items: &[EffectDispatchItem],
    ) -> Result<u32, PersistError> {
        MemoryWorldPersistence::publish_effect_dispatches_guarded(
            self, universe, world, lease, now_ns, items,
        )
    }

    fn claim_pending_effects_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        now_ns: u64,
        claim_ttl_ns: u64,
        limit: u32,
    ) -> Result<Vec<(QueueSeq, EffectDispatchItem)>, PersistError> {
        MemoryWorldPersistence::claim_pending_effects_for_world(
            self,
            universe,
            world,
            worker_id,
            now_ns,
            claim_ttl_ns,
            limit,
        )
    }

    fn ack_effect_dispatch_with_receipt(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        shard: ShardId,
        seq: QueueSeq,
        now_ns: u64,
        receipt: ReceiptIngress,
    ) -> Result<(), PersistError> {
        MemoryWorldPersistence::ack_effect_dispatch_with_receipt(
            self, universe, world, worker_id, shard, seq, now_ns, receipt,
        )
    }

    fn retain_effect_dispatches_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        valid_intents: &std::collections::HashSet<[u8; 32]>,
        now_ns: u64,
    ) -> Result<u32, PersistError> {
        MemoryWorldPersistence::retain_effect_dispatches_for_world(
            self,
            universe,
            world,
            valid_intents,
            now_ns,
        )
    }

    fn requeue_expired_effect_claims(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        MemoryWorldPersistence::requeue_expired_effect_claims(self, universe, now_ns, limit)
    }
}

impl HostedTimerQueueStore for MemoryWorldPersistence {
    fn publish_due_timers_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        items: &[TimerDueItem],
    ) -> Result<u32, PersistError> {
        MemoryWorldPersistence::publish_due_timers_guarded(
            self, universe, world, lease, now_ns, items,
        )
    }

    fn claim_due_timers_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        now_ns: u64,
        claim_ttl_ns: u64,
        limit: u32,
    ) -> Result<Vec<TimerDueItem>, PersistError> {
        MemoryWorldPersistence::claim_due_timers_for_world(
            self,
            universe,
            world,
            worker_id,
            now_ns,
            claim_ttl_ns,
            limit,
        )
    }

    fn ack_timer_delivery_with_receipt(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        intent_hash: &[u8],
        now_ns: u64,
        receipt: ReceiptIngress,
    ) -> Result<(), PersistError> {
        MemoryWorldPersistence::ack_timer_delivery_with_receipt(
            self,
            universe,
            world,
            worker_id,
            intent_hash,
            now_ns,
            receipt,
        )
    }

    fn outstanding_intent_hashes_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) -> Result<Vec<[u8; 32]>, PersistError> {
        MemoryWorldPersistence::outstanding_intent_hashes_for_world(self, universe, world, now_ns)
    }

    fn requeue_expired_timer_claims(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        MemoryWorldPersistence::requeue_expired_timer_claims(self, universe, now_ns, limit)
    }
}

impl HostedPortalStore for MemoryWorldPersistence {
    fn portal_send(
        &self,
        universe: UniverseId,
        dest_world: WorldId,
        now_ns: u64,
        message_id: &[u8],
        item: InboxItem,
    ) -> Result<PortalSendResult, PersistError> {
        MemoryWorldPersistence::portal_send(self, universe, dest_world, now_ns, message_id, item)
    }

    fn sweep_effect_dedupe_gc(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        MemoryWorldPersistence::sweep_effect_dedupe_gc(self, universe, now_ns, limit)
    }

    fn sweep_timer_dedupe_gc(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        MemoryWorldPersistence::sweep_timer_dedupe_gc(self, universe, now_ns, limit)
    }

    fn sweep_portal_dedupe_gc(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        MemoryWorldPersistence::sweep_portal_dedupe_gc(self, universe, now_ns, limit)
    }
}
