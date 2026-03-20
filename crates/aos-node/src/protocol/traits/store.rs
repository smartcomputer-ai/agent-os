use std::io::{Read, Write};

use aos_cbor::Hash;

use crate::protocol::{
    CasRootRecord, InboxItem, InboxSeq, JournalHeight, PersistError, SegmentExportRequest,
    SegmentExportResult, SegmentId, SegmentIndexRecord, SnapshotCommitRequest,
    SnapshotCommitResult, SnapshotRecord, UniverseId, WorldId,
};

/// Low-level CAS contract shared by node persistence backends.
pub trait CasStore: Send + Sync {
    fn put_verified(&self, universe: UniverseId, bytes: &[u8]) -> Result<Hash, PersistError>;

    fn put_reader_known_hash(
        &self,
        universe: UniverseId,
        expected: Hash,
        size_bytes: u64,
        reader: &mut dyn Read,
    ) -> Result<Hash, PersistError>;

    fn stat(&self, universe: UniverseId, hash: Hash) -> Result<CasRootRecord, PersistError>;

    fn get(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, PersistError> {
        let mut bytes = Vec::new();
        self.read_to_writer(universe, hash, &mut bytes)?;
        Ok(bytes)
    }

    fn has(&self, universe: UniverseId, hash: Hash) -> Result<bool, PersistError>;

    fn read_to_writer(
        &self,
        universe: UniverseId,
        hash: Hash,
        writer: &mut dyn Write,
    ) -> Result<CasRootRecord, PersistError>;

    fn read_range_to_writer(
        &self,
        universe: UniverseId,
        hash: Hash,
        offset: u64,
        len: u64,
        writer: &mut dyn Write,
    ) -> Result<(), PersistError>;
}

/// Replay-critical single-world persistence required to open, run, and
/// checkpoint a world under node control.
pub trait WorldStore: Send + Sync {
    fn cas_put_verified(&self, universe: UniverseId, bytes: &[u8]) -> Result<Hash, PersistError>;
    fn cas_get(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, PersistError>;
    fn cas_has(&self, universe: UniverseId, hash: Hash) -> Result<bool, PersistError>;

    fn journal_append_batch(
        &self,
        universe: UniverseId,
        world: WorldId,
        expected_head: JournalHeight,
        entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError>;

    fn journal_read_range(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_inclusive: JournalHeight,
        limit: u32,
    ) -> Result<Vec<(JournalHeight, Vec<u8>)>, PersistError>;

    fn journal_head(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<JournalHeight, PersistError>;

    fn inbox_enqueue(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError>;

    fn inbox_read_after(
        &self,
        universe: UniverseId,
        world: WorldId,
        after_exclusive: Option<InboxSeq>,
        limit: u32,
    ) -> Result<Vec<(InboxSeq, InboxItem)>, PersistError>;

    fn inbox_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<InboxSeq>, PersistError>;

    fn inbox_commit_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
    ) -> Result<(), PersistError>;

    fn drain_inbox_to_journal(
        &self,
        universe: UniverseId,
        world: WorldId,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
        expected_head: JournalHeight,
        journal_entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError>;

    fn snapshot_index(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError>;

    fn snapshot_commit(
        &self,
        universe: UniverseId,
        world: WorldId,
        request: SnapshotCommitRequest,
    ) -> Result<SnapshotCommitResult, PersistError>;

    fn snapshot_at_height(
        &self,
        universe: UniverseId,
        world: WorldId,
        height: JournalHeight,
    ) -> Result<SnapshotRecord, PersistError>;

    fn snapshot_latest(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<SnapshotRecord, PersistError>;

    fn snapshot_active_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<SnapshotRecord, PersistError>;

    fn snapshot_promote_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError>;

    fn snapshot_repair_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError>;

    fn segment_index_put(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SegmentIndexRecord,
    ) -> Result<(), PersistError>;

    fn segment_export(
        &self,
        universe: UniverseId,
        world: WorldId,
        request: SegmentExportRequest,
    ) -> Result<SegmentExportResult, PersistError>;

    fn segment_index_read_from(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_end_inclusive: JournalHeight,
        limit: u32,
    ) -> Result<Vec<SegmentIndexRecord>, PersistError>;

    fn segment_read_entries(
        &self,
        universe: UniverseId,
        world: WorldId,
        segment: SegmentId,
    ) -> Result<Vec<(JournalHeight, Vec<u8>)>, PersistError>;
}
