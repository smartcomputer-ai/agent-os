use crate::protocol::{
    BaseNodeStore, CellStateProjectionRecord, CommandRecord, EffectDispatchItem,
    HeadProjectionRecord, InboxItem, InboxSeq, JournalHeight, NodeWorldRuntimeInfo, PersistError,
    PortalSendResult, QueryProjectionDelta, QueryProjectionMaterialization, QueueSeq,
    ReceiptIngress, SegmentExportRequest, SegmentExportResult, SegmentIndexRecord, ShardId,
    SnapshotCommitRequest, SnapshotCommitResult, SnapshotMaintenanceConfig, SnapshotRecord,
    TimerDueItem, UniverseId, WorkerHeartbeat, WorkspaceRegistryProjectionRecord, WorldId,
    WorldLease, WorldStore,
};

pub trait ProjectionStore: WorldStore {
    fn head_projection(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<HeadProjectionRecord>, PersistError>;

    fn cell_state_projection(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        key_hash: &[u8],
    ) -> Result<Option<CellStateProjectionRecord>, PersistError>;

    fn list_cell_state_projections(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        after_key_hash: Option<Vec<u8>>,
        limit: u32,
    ) -> Result<Vec<CellStateProjectionRecord>, PersistError>;

    fn workspace_projection(
        &self,
        universe: UniverseId,
        world: WorldId,
        workspace: &str,
    ) -> Result<Option<WorkspaceRegistryProjectionRecord>, PersistError>;

    fn list_workspace_projections(
        &self,
        universe: UniverseId,
        world: WorldId,
        after_workspace: Option<String>,
        limit: u32,
    ) -> Result<Vec<WorkspaceRegistryProjectionRecord>, PersistError>;

    fn bootstrap_query_projections(
        &self,
        universe: UniverseId,
        world: WorldId,
        materialization: QueryProjectionMaterialization,
    ) -> Result<(), PersistError>;
}

pub trait HostedCoordinationStore: WorldStore {
    fn snapshot_maintenance_config(&self) -> SnapshotMaintenanceConfig;

    fn heartbeat_worker(&self, heartbeat: WorkerHeartbeat) -> Result<(), PersistError>;

    fn list_active_workers(
        &self,
        now_ns: u64,
        limit: u32,
    ) -> Result<Vec<WorkerHeartbeat>, PersistError>;

    fn list_ready_worlds(
        &self,
        now_ns: u64,
        limit: u32,
        universe_filter: Option<&[UniverseId]>,
    ) -> Result<Vec<NodeWorldRuntimeInfo>, PersistError>;

    fn current_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<WorldLease>, PersistError>;

    fn acquire_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        now_ns: u64,
        lease_ttl_ns: u64,
    ) -> Result<WorldLease, PersistError>;

    fn renew_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        lease_ttl_ns: u64,
    ) -> Result<WorldLease, PersistError>;

    fn release_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
    ) -> Result<(), PersistError>;

    fn list_worker_worlds(
        &self,
        worker_id: &str,
        now_ns: u64,
        limit: u32,
        universe_filter: Option<&[UniverseId]>,
    ) -> Result<Vec<NodeWorldRuntimeInfo>, PersistError>;

    fn update_command_record_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        record: CommandRecord,
    ) -> Result<(), PersistError>;

    fn journal_append_batch_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        expected_head: JournalHeight,
        entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError>;

    fn inbox_commit_cursor_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
    ) -> Result<(), PersistError>;

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
    ) -> Result<JournalHeight, PersistError>;

    fn materialize_query_projections_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        materialization: QueryProjectionMaterialization,
    ) -> Result<(), PersistError>;

    fn apply_query_projection_delta_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        delta: QueryProjectionDelta,
    ) -> Result<(), PersistError>;

    fn snapshot_index_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        record: SnapshotRecord,
    ) -> Result<(), PersistError>;

    fn snapshot_commit_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        request: SnapshotCommitRequest,
    ) -> Result<SnapshotCommitResult, PersistError>;

    fn snapshot_promote_baseline_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        record: SnapshotRecord,
    ) -> Result<(), PersistError>;

    fn segment_index_put_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        record: SegmentIndexRecord,
    ) -> Result<(), PersistError>;

    fn segment_export_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        request: SegmentExportRequest,
    ) -> Result<SegmentExportResult, PersistError>;
}

pub trait HostedEffectQueueStore: WorldStore {
    fn publish_effect_dispatches_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        items: &[EffectDispatchItem],
    ) -> Result<u32, PersistError>;

    fn claim_pending_effects_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        now_ns: u64,
        claim_ttl_ns: u64,
        limit: u32,
    ) -> Result<Vec<(QueueSeq, EffectDispatchItem)>, PersistError>;

    fn ack_effect_dispatch_with_receipt(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        shard: ShardId,
        seq: QueueSeq,
        now_ns: u64,
        receipt: ReceiptIngress,
    ) -> Result<(), PersistError>;

    fn retain_effect_dispatches_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        valid_intents: &std::collections::HashSet<[u8; 32]>,
        now_ns: u64,
    ) -> Result<u32, PersistError>;

    fn requeue_expired_effect_claims(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError>;
}

pub trait HostedTimerQueueStore: WorldStore {
    fn publish_due_timers_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        items: &[TimerDueItem],
    ) -> Result<u32, PersistError>;

    fn claim_due_timers_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        now_ns: u64,
        claim_ttl_ns: u64,
        limit: u32,
    ) -> Result<Vec<TimerDueItem>, PersistError>;

    fn ack_timer_delivery_with_receipt(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        intent_hash: &[u8],
        now_ns: u64,
        receipt: ReceiptIngress,
    ) -> Result<(), PersistError>;

    fn outstanding_intent_hashes_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) -> Result<Vec<[u8; 32]>, PersistError>;

    fn requeue_expired_timer_claims(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError>;
}

pub trait HostedPortalStore: WorldStore {
    fn portal_send(
        &self,
        universe: UniverseId,
        dest_world: WorldId,
        now_ns: u64,
        message_id: &[u8],
        item: InboxItem,
    ) -> Result<PortalSendResult, PersistError>;

    fn sweep_effect_dedupe_gc(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError>;

    fn sweep_timer_dedupe_gc(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError>;

    fn sweep_portal_dedupe_gc(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError>;
}

pub trait HostedRuntimeStore:
    BaseNodeStore
    + ProjectionStore
    + HostedCoordinationStore
    + HostedEffectQueueStore
    + HostedTimerQueueStore
    + HostedPortalStore
{
}

impl<T> HostedRuntimeStore for T where
    T: BaseNodeStore
        + ProjectionStore
        + HostedCoordinationStore
        + HostedEffectQueueStore
        + HostedTimerQueueStore
        + HostedPortalStore
{
}
