use std::sync::{Arc, Mutex};

use aos_cbor::Hash;
use aos_fdb::{
    HostedCoordinationStore, InboxItem, InboxSeq, JournalHeight, PersistError,
    SegmentExportRequest, SegmentExportResult, SegmentId, SegmentIndexRecord,
    SnapshotCommitRequest, SnapshotCommitResult, SnapshotRecord, UniverseId, WorldId, WorldLease,
    WorldStore,
};
use aos_runtime::now_wallclock_ns;

pub struct LeasedWorldPersistence<P> {
    runtime: Arc<P>,
    lease: Arc<Mutex<WorldLease>>,
}

impl<P> LeasedWorldPersistence<P> {
    pub fn new(
        runtime: Arc<P>,
        _universe: UniverseId,
        _world: WorldId,
        lease: Arc<Mutex<WorldLease>>,
    ) -> Self {
        Self { runtime, lease }
    }

    fn current_lease(&self) -> Result<WorldLease, PersistError> {
        self.lease
            .lock()
            .map(|lease| lease.clone())
            .map_err(|_| PersistError::backend("leased world persistence mutex poisoned"))
    }
}

impl<P> WorldStore for LeasedWorldPersistence<P>
where
    P: HostedCoordinationStore + WorldStore + 'static,
{
    fn cas_put_verified(&self, universe: UniverseId, bytes: &[u8]) -> Result<Hash, PersistError> {
        self.runtime.cas_put_verified(universe, bytes)
    }

    fn cas_get(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, PersistError> {
        self.runtime.cas_get(universe, hash)
    }

    fn cas_has(&self, universe: UniverseId, hash: Hash) -> Result<bool, PersistError> {
        self.runtime.cas_has(universe, hash)
    }

    fn journal_append_batch(
        &self,
        universe: UniverseId,
        world: WorldId,
        expected_head: JournalHeight,
        entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError> {
        let lease = self.current_lease()?;
        self.runtime.journal_append_batch_guarded(
            universe,
            world,
            &lease,
            now_wallclock_ns(),
            expected_head,
            entries,
        )
    }

    fn journal_read_range(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_inclusive: JournalHeight,
        limit: u32,
    ) -> Result<Vec<(JournalHeight, Vec<u8>)>, PersistError> {
        self.runtime
            .journal_read_range(universe, world, from_inclusive, limit)
    }

    fn journal_head(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<JournalHeight, PersistError> {
        self.runtime.journal_head(universe, world)
    }

    fn inbox_enqueue(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError> {
        self.runtime.inbox_enqueue(universe, world, item)
    }

    fn inbox_read_after(
        &self,
        universe: UniverseId,
        world: WorldId,
        after_exclusive: Option<InboxSeq>,
        limit: u32,
    ) -> Result<Vec<(InboxSeq, InboxItem)>, PersistError> {
        self.runtime
            .inbox_read_after(universe, world, after_exclusive, limit)
    }

    fn inbox_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<InboxSeq>, PersistError> {
        self.runtime.inbox_cursor(universe, world)
    }

    fn inbox_commit_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
    ) -> Result<(), PersistError> {
        let lease = self.current_lease()?;
        self.runtime.inbox_commit_cursor_guarded(
            universe,
            world,
            &lease,
            now_wallclock_ns(),
            old_cursor,
            new_cursor,
        )
    }

    fn drain_inbox_to_journal(
        &self,
        universe: UniverseId,
        world: WorldId,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
        expected_head: JournalHeight,
        journal_entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError> {
        let lease = self.current_lease()?;
        self.runtime.drain_inbox_to_journal_guarded(
            universe,
            world,
            &lease,
            now_wallclock_ns(),
            old_cursor,
            new_cursor,
            expected_head,
            journal_entries,
        )
    }

    fn snapshot_index(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        let lease = self.current_lease()?;
        self.runtime
            .snapshot_index_guarded(universe, world, &lease, now_wallclock_ns(), record)
    }

    fn snapshot_commit(
        &self,
        universe: UniverseId,
        world: WorldId,
        request: SnapshotCommitRequest,
    ) -> Result<SnapshotCommitResult, PersistError> {
        let lease = self.current_lease()?;
        self.runtime
            .snapshot_commit_guarded(universe, world, &lease, now_wallclock_ns(), request)
    }

    fn snapshot_at_height(
        &self,
        universe: UniverseId,
        world: WorldId,
        height: JournalHeight,
    ) -> Result<SnapshotRecord, PersistError> {
        self.runtime.snapshot_at_height(universe, world, height)
    }

    fn snapshot_latest(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<SnapshotRecord, PersistError> {
        self.runtime.snapshot_latest(universe, world)
    }

    fn snapshot_active_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<SnapshotRecord, PersistError> {
        self.runtime.snapshot_active_baseline(universe, world)
    }

    fn snapshot_promote_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        let lease = self.current_lease()?;
        self.runtime.snapshot_promote_baseline_guarded(
            universe,
            world,
            &lease,
            now_wallclock_ns(),
            record,
        )
    }

    fn snapshot_repair_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        self.runtime.snapshot_repair_record(universe, world, record)
    }

    fn segment_index_put(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SegmentIndexRecord,
    ) -> Result<(), PersistError> {
        let lease = self.current_lease()?;
        self.runtime
            .segment_index_put_guarded(universe, world, &lease, now_wallclock_ns(), record)
    }

    fn segment_export(
        &self,
        universe: UniverseId,
        world: WorldId,
        request: SegmentExportRequest,
    ) -> Result<SegmentExportResult, PersistError> {
        let lease = self.current_lease()?;
        self.runtime
            .segment_export_guarded(universe, world, &lease, now_wallclock_ns(), request)
    }

    fn segment_index_read_from(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_end_inclusive: JournalHeight,
        limit: u32,
    ) -> Result<Vec<SegmentIndexRecord>, PersistError> {
        self.runtime
            .segment_index_read_from(universe, world, from_end_inclusive, limit)
    }

    fn segment_read_entries(
        &self,
        universe: UniverseId,
        world: WorldId,
        segment: SegmentId,
    ) -> Result<Vec<(JournalHeight, Vec<u8>)>, PersistError> {
        self.runtime.segment_read_entries(universe, world, segment)
    }
}
