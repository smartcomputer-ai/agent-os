use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aos_cbor::Hash;

use crate::protocol::{
    BlobStorage, CasMeta, CborPayload, InboxItem, InboxSeq, JournalHeight, PersistConflict,
    PersistCorruption, PersistError, PersistenceConfig, SegmentIndexRecord, SnapshotCommitRequest,
    SnapshotCommitResult, SnapshotRecord, UniverseId, WorldId, WorldPersistence, cas_object_key,
    ensure_monotonic_snapshot_records, sample_world_meta, validate_baseline_promotion_record,
    validate_snapshot_commit_request, validate_snapshot_record,
};

#[derive(Debug, Clone)]
pub struct MemoryWorldPersistence {
    state: Arc<Mutex<MemoryState>>,
    config: PersistenceConfig,
}

#[derive(Debug, Default)]
struct MemoryState {
    cas: BTreeMap<UniverseId, BTreeMap<Hash, CasEntry>>,
    cas_objects: BTreeMap<String, Vec<u8>>,
    worlds: BTreeMap<(UniverseId, WorldId), WorldState>,
}

#[derive(Debug, Clone)]
struct CasEntry {
    meta: CasMeta,
}

#[derive(Debug)]
struct WorldState {
    journal_head: JournalHeight,
    journal_entries: BTreeMap<JournalHeight, Vec<u8>>,
    inbox_entries: BTreeMap<InboxSeq, InboxItem>,
    inbox_cursor: Option<InboxSeq>,
    next_inbox_seq: u64,
    snapshots: BTreeMap<JournalHeight, SnapshotRecord>,
    active_baseline: Option<SnapshotRecord>,
    segments: BTreeMap<JournalHeight, SegmentIndexRecord>,
    notify_counter: u64,
}

impl Default for WorldState {
    fn default() -> Self {
        let _ = sample_world_meta();
        Self {
            journal_head: 0,
            journal_entries: BTreeMap::new(),
            inbox_entries: BTreeMap::new(),
            inbox_cursor: None,
            next_inbox_seq: 0,
            snapshots: BTreeMap::new(),
            active_baseline: None,
            segments: BTreeMap::new(),
            notify_counter: 0,
        }
    }
}

impl MemoryWorldPersistence {
    pub fn new() -> Self {
        Self::with_config(PersistenceConfig::default())
    }

    pub fn with_config(config: PersistenceConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(MemoryState::default())),
            config,
        }
    }

    fn with_world_mut<R>(
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

    fn with_world<R>(
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

    fn cas_payload_for_read(
        &self,
        guard: &MemoryState,
        hash: Hash,
        entry: &CasEntry,
    ) -> Result<Vec<u8>, PersistError> {
        let bytes = match entry.meta.storage {
            BlobStorage::Inline => entry
                .meta
                .inline_bytes
                .clone()
                .ok_or(PersistCorruption::MissingInlineCasBytes { hash })?,
            BlobStorage::ObjectStore => {
                let key = entry
                    .meta
                    .object_key
                    .as_ref()
                    .ok_or(PersistCorruption::MissingCasObjectKey { hash })?;
                guard.cas_objects.get(key).cloned().ok_or_else(|| {
                    PersistCorruption::MissingCasObjectBody {
                        hash,
                        object_key: key.clone(),
                    }
                })?
            }
        };

        if self.config.cas.verify_reads {
            let actual = Hash::of_bytes(&bytes);
            if actual != hash {
                return Err(PersistCorruption::CasBodyHashMismatch {
                    expected: hash,
                    actual,
                }
                .into());
            }
        }
        Ok(bytes)
    }

    #[cfg(test)]
    fn debug_cas_entry(
        &self,
        universe: UniverseId,
        hash: Hash,
    ) -> Option<(CasMeta, Option<Vec<u8>>)> {
        let guard = self.state.lock().ok()?;
        let entry = guard.cas.get(&universe)?.get(&hash)?;
        let object_bytes = entry
            .meta
            .object_key
            .as_ref()
            .and_then(|key| guard.cas_objects.get(key).cloned());
        Some((entry.meta.clone(), object_bytes))
    }

    #[cfg(test)]
    fn debug_remove_journal_entry(
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

    fn validate_journal_batch(&self, entries: &[Vec<u8>]) -> Result<(), PersistError> {
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

    fn normalize_payload(
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

    fn normalize_inbox_item(
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
}

impl WorldPersistence for MemoryWorldPersistence {
    fn cas_put_verified(&self, universe: UniverseId, bytes: &[u8]) -> Result<Hash, PersistError> {
        let hash = Hash::of_bytes(bytes);
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        if let Some(existing) = guard.cas.entry(universe).or_default().get(&hash).cloned() {
            let _ = self.cas_payload_for_read(&guard, hash, &existing)?;
            return Ok(hash);
        }

        let meta = if bytes.len() <= self.config.cas.inline_threshold_bytes {
            CasMeta {
                size: bytes.len() as u64,
                storage: BlobStorage::Inline,
                object_key: None,
                inline_bytes: Some(bytes.to_vec()),
            }
        } else {
            let object_key = cas_object_key(universe, hash);
            guard
                .cas_objects
                .entry(object_key.clone())
                .or_insert_with(|| bytes.to_vec());
            CasMeta {
                size: bytes.len() as u64,
                storage: BlobStorage::ObjectStore,
                object_key: Some(object_key),
                inline_bytes: None,
            }
        };

        guard
            .cas
            .entry(universe)
            .or_default()
            .insert(hash, CasEntry { meta });
        Ok(hash)
    }

    fn cas_get(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let entry = guard
            .cas
            .get(&universe)
            .and_then(|universe_cas| universe_cas.get(&hash))
            .ok_or_else(|| PersistError::not_found(format!("cas object {hash}")))?;
        self.cas_payload_for_read(&guard, hash, entry)
    }

    fn cas_has(&self, universe: UniverseId, hash: Hash) -> Result<bool, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        Ok(guard
            .cas
            .get(&universe)
            .map(|universe_cas| universe_cas.contains_key(&hash))
            .unwrap_or(false))
    }

    fn journal_append_batch(
        &self,
        universe: UniverseId,
        world: WorldId,
        expected_head: JournalHeight,
        entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError> {
        self.validate_journal_batch(entries)?;
        self.with_world_mut(universe, world, |world_state| {
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
        })
    }

    fn journal_read_range(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_inclusive: JournalHeight,
        limit: u32,
    ) -> Result<Vec<(JournalHeight, Vec<u8>)>, PersistError> {
        self.with_world(universe, world, |world_state| {
            if from_inclusive >= world_state.journal_head || limit == 0 {
                return Ok(Vec::new());
            }
            let end_exclusive = world_state
                .journal_head
                .min(from_inclusive.saturating_add(limit as u64));
            let mut entries = Vec::with_capacity((end_exclusive - from_inclusive) as usize);
            for height in from_inclusive..end_exclusive {
                let bytes = world_state
                    .journal_entries
                    .get(&height)
                    .cloned()
                    .ok_or(PersistCorruption::MissingJournalEntry { height })?;
                entries.push((height, bytes));
            }
            Ok(entries)
        })
    }

    fn journal_head(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<JournalHeight, PersistError> {
        self.with_world(universe, world, |world_state| Ok(world_state.journal_head))
    }

    fn inbox_enqueue(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError> {
        let item = self.normalize_inbox_item(universe, item)?;
        self.with_world_mut(universe, world, |world_state| {
            let seq = InboxSeq::from_u64(world_state.next_inbox_seq);
            world_state.next_inbox_seq += 1;
            world_state.inbox_entries.insert(seq.clone(), item);
            world_state.notify_counter = world_state.notify_counter.saturating_add(1);
            Ok(seq)
        })
    }

    fn inbox_read_after(
        &self,
        universe: UniverseId,
        world: WorldId,
        after_exclusive: Option<InboxSeq>,
        limit: u32,
    ) -> Result<Vec<(InboxSeq, InboxItem)>, PersistError> {
        self.with_world(universe, world, |world_state| {
            let iter = world_state.inbox_entries.iter();
            let items = match after_exclusive {
                Some(after) => iter
                    .filter(|(seq, _)| *seq > &after)
                    .take(limit as usize)
                    .map(|(seq, item)| (seq.clone(), item.clone()))
                    .collect(),
                None => iter
                    .take(limit as usize)
                    .map(|(seq, item)| (seq.clone(), item.clone()))
                    .collect(),
            };
            Ok(items)
        })
    }

    fn inbox_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<InboxSeq>, PersistError> {
        self.with_world(universe, world, |world_state| {
            Ok(world_state.inbox_cursor.clone())
        })
    }

    fn inbox_commit_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
    ) -> Result<(), PersistError> {
        self.with_world_mut(universe, world, |world_state| {
            if world_state.inbox_cursor != old_cursor {
                return Err(PersistConflict::InboxCursorAdvanced {
                    expected: old_cursor,
                    actual: world_state.inbox_cursor.clone(),
                }
                .into());
            }
            if let Some(current) = &world_state.inbox_cursor {
                if new_cursor < *current {
                    return Err(PersistError::validation("inbox cursor cannot regress"));
                }
            }
            if !world_state.inbox_entries.contains_key(&new_cursor) {
                return Err(PersistError::not_found(format!(
                    "inbox sequence {new_cursor} does not exist"
                )));
            }
            world_state.inbox_cursor = Some(new_cursor);
            Ok(())
        })
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
        self.validate_journal_batch(journal_entries)?;
        self.with_world_mut(universe, world, |world_state| {
            if world_state.inbox_cursor != old_cursor {
                return Err(PersistConflict::InboxCursorAdvanced {
                    expected: old_cursor,
                    actual: world_state.inbox_cursor.clone(),
                }
                .into());
            }
            if let Some(current) = &world_state.inbox_cursor {
                if new_cursor < *current {
                    return Err(PersistError::validation("inbox cursor cannot regress"));
                }
            }
            if !world_state.inbox_entries.contains_key(&new_cursor) {
                return Err(PersistError::not_found(format!(
                    "inbox sequence {new_cursor} does not exist"
                )));
            }
            if world_state.journal_head != expected_head {
                return Err(PersistConflict::HeadAdvanced {
                    expected: expected_head,
                    actual: world_state.journal_head,
                }
                .into());
            }
            let first_height = world_state.journal_head;
            for entry in journal_entries {
                world_state
                    .journal_entries
                    .insert(world_state.journal_head, entry.clone());
                world_state.journal_head += 1;
            }
            world_state.inbox_cursor = Some(new_cursor);
            Ok(first_height)
        })
    }

    fn snapshot_index(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        validate_snapshot_record(&record)?;
        self.with_world_mut(universe, world, |world_state| {
            ensure_monotonic_snapshot_records(&world_state.snapshots, &record)?;
            world_state.snapshots.insert(record.height, record);
            Ok(())
        })
    }

    fn snapshot_commit(
        &self,
        universe: UniverseId,
        world: WorldId,
        request: SnapshotCommitRequest,
    ) -> Result<SnapshotCommitResult, PersistError> {
        validate_snapshot_commit_request(&request)?;
        let snapshot_hash = self.cas_put_verified(universe, &request.snapshot_bytes)?;
        let expected_ref = snapshot_hash.to_hex();
        if request.record.snapshot_ref != expected_ref {
            return Err(PersistError::validation(format!(
                "snapshot_ref {} must equal CAS hash {}",
                request.record.snapshot_ref, expected_ref
            )));
        }
        self.validate_journal_batch(std::slice::from_ref(&request.snapshot_journal_entry))?;
        if let Some(baseline) = &request.baseline_journal_entry {
            self.validate_journal_batch(&[
                request.snapshot_journal_entry.clone(),
                baseline.clone(),
            ])?;
        }

        self.with_world_mut(universe, world, |world_state| {
            ensure_monotonic_snapshot_records(&world_state.snapshots, &request.record)?;
            if world_state.journal_head != request.expected_head {
                return Err(PersistConflict::HeadAdvanced {
                    expected: request.expected_head,
                    actual: world_state.journal_head,
                }
                .into());
            }
            if request.promote_baseline {
                validate_baseline_promotion_record(&request.record)?;
                if let Some(active) = &world_state.active_baseline {
                    if request.record.height < active.height {
                        return Err(PersistError::validation(format!(
                            "baseline cannot regress from {} to {}",
                            active.height, request.record.height
                        )));
                    }
                    if request.record.height == active.height && request.record != *active {
                        return Err(PersistConflict::BaselineMismatch {
                            height: request.record.height,
                        }
                        .into());
                    }
                }
            }

            world_state
                .snapshots
                .insert(request.record.height, request.record.clone());

            let first_height = world_state.journal_head;
            world_state.journal_entries.insert(
                world_state.journal_head,
                request.snapshot_journal_entry.clone(),
            );
            world_state.journal_head += 1;

            if request.promote_baseline {
                world_state.journal_entries.insert(
                    world_state.journal_head,
                    request
                        .baseline_journal_entry
                        .clone()
                        .expect("validated baseline journal entry"),
                );
                world_state.journal_head += 1;
                world_state.active_baseline = Some(request.record.clone());
            }

            Ok(SnapshotCommitResult {
                snapshot_hash,
                first_height,
                next_head: world_state.journal_head,
                baseline_promoted: request.promote_baseline,
            })
        })
    }

    fn snapshot_at_height(
        &self,
        universe: UniverseId,
        world: WorldId,
        height: JournalHeight,
    ) -> Result<SnapshotRecord, PersistError> {
        self.with_world(universe, world, |world_state| {
            world_state
                .snapshots
                .get(&height)
                .cloned()
                .ok_or_else(|| PersistError::not_found(format!("snapshot at height {height}")))
        })
    }

    fn snapshot_active_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<SnapshotRecord, PersistError> {
        self.with_world(universe, world, |world_state| {
            world_state
                .active_baseline
                .clone()
                .ok_or_else(|| PersistError::not_found("active baseline"))
        })
    }

    fn snapshot_promote_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        validate_baseline_promotion_record(&record)?;
        self.with_world_mut(universe, world, |world_state| {
            let indexed = world_state.snapshots.get(&record.height).ok_or_else(|| {
                PersistError::not_found(format!("snapshot at height {}", record.height))
            })?;
            if indexed != &record {
                return Err(PersistConflict::SnapshotMismatch {
                    height: record.height,
                }
                .into());
            }
            if let Some(active) = &world_state.active_baseline {
                if record.height < active.height {
                    return Err(PersistError::validation(format!(
                        "baseline cannot regress from {} to {}",
                        active.height, record.height
                    )));
                }
                if record.height == active.height && record != *active {
                    return Err(PersistConflict::BaselineMismatch {
                        height: record.height,
                    }
                    .into());
                }
            }
            world_state.active_baseline = Some(record);
            Ok(())
        })
    }

    fn segment_index_put(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SegmentIndexRecord,
    ) -> Result<(), PersistError> {
        if record.segment.end < record.segment.start {
            return Err(PersistError::validation(format!(
                "segment end {} must be >= start {}",
                record.segment.end, record.segment.start
            )));
        }
        self.with_world_mut(universe, world, |world_state| {
            if let Some(existing) = world_state.segments.get(&record.segment.end) {
                if existing == &record {
                    return Ok(());
                }
                return Err(PersistConflict::SegmentExists {
                    end_height: record.segment.end,
                }
                .into());
            }
            world_state.segments.insert(record.segment.end, record);
            Ok(())
        })
    }

    fn segment_index_read_from(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_end_inclusive: JournalHeight,
        limit: u32,
    ) -> Result<Vec<SegmentIndexRecord>, PersistError> {
        self.with_world(universe, world, |world_state| {
            Ok(world_state
                .segments
                .range(from_end_inclusive..)
                .take(limit as usize)
                .map(|(_, record)| record.clone())
                .collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CborPayload, InboxItem, PersistConflict, PersistCorruption, PersistenceConfig,
        SnapshotCommitRequest, SnapshotRecord, WorldPersistence, cas_object_key,
    };
    use uuid::Uuid;

    fn universe() -> UniverseId {
        UniverseId::from(Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap())
    }

    fn world() -> WorldId {
        WorldId::from(Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap())
    }

    fn timer_ingress(seed: u8) -> InboxItem {
        InboxItem::TimerFired(crate::TimerFiredIngress {
            timer_id: format!("timer-{seed}"),
            payload: CborPayload::inline(vec![seed]),
            correlation_id: None,
        })
    }

    fn control_ingress(bytes: &[u8]) -> InboxItem {
        InboxItem::Control(crate::ControlIngress {
            cmd: "event-send".into(),
            payload: CborPayload::inline(bytes.to_vec()),
            correlation_id: None,
        })
    }

    fn snapshot(height: JournalHeight, snapshot_ref: &str) -> SnapshotRecord {
        SnapshotRecord {
            snapshot_ref: snapshot_ref.to_string(),
            height,
            logical_time_ns: height * 10,
            receipt_horizon_height: Some(height),
            manifest_hash: Some("sha256:manifest".into()),
        }
    }

    #[test]
    fn cas_put_verified_is_idempotent_and_hash_correct() {
        let persistence = MemoryWorldPersistence::new();
        let bytes = b"hello persistence";

        let first = persistence.cas_put_verified(universe(), bytes).unwrap();
        let second = persistence.cas_put_verified(universe(), bytes).unwrap();

        assert_eq!(first, Hash::of_bytes(bytes));
        assert_eq!(first, second);
        assert!(persistence.cas_has(universe(), first).unwrap());
        assert_eq!(persistence.cas_get(universe(), first).unwrap(), bytes);
    }

    #[test]
    fn small_cas_blob_is_stored_inline() {
        let persistence = MemoryWorldPersistence::with_config(PersistenceConfig {
            cas: crate::CasConfig {
                inline_threshold_bytes: 8,
                verify_reads: true,
            },
            ..PersistenceConfig::default()
        });
        let bytes = b"small";
        let hash = persistence.cas_put_verified(universe(), bytes).unwrap();

        let (meta, object_bytes) = persistence.debug_cas_entry(universe(), hash).unwrap();
        assert_eq!(meta.storage, BlobStorage::Inline);
        assert_eq!(meta.inline_bytes, Some(bytes.to_vec()));
        assert_eq!(meta.object_key, None);
        assert_eq!(object_bytes, None);
    }

    #[test]
    fn large_cas_blob_is_externalized_under_deterministic_object_key() {
        let persistence = MemoryWorldPersistence::with_config(PersistenceConfig {
            cas: crate::CasConfig {
                inline_threshold_bytes: 4,
                verify_reads: true,
            },
            ..PersistenceConfig::default()
        });
        let bytes = b"definitely larger than four bytes";
        let hash = persistence.cas_put_verified(universe(), bytes).unwrap();

        let (meta, object_bytes) = persistence.debug_cas_entry(universe(), hash).unwrap();
        assert_eq!(meta.storage, BlobStorage::ObjectStore);
        assert_eq!(meta.inline_bytes, None);
        assert_eq!(
            meta.object_key.as_deref(),
            Some(cas_object_key(universe(), hash).as_str())
        );
        assert_eq!(object_bytes, Some(bytes.to_vec()));
        assert_eq!(persistence.cas_get(universe(), hash).unwrap(), bytes);
    }

    #[test]
    fn cas_read_detects_external_object_corruption() {
        let persistence = MemoryWorldPersistence::with_config(PersistenceConfig {
            cas: crate::CasConfig {
                inline_threshold_bytes: 1,
                verify_reads: true,
            },
            ..PersistenceConfig::default()
        });
        let bytes = b"this must be external";
        let hash = persistence.cas_put_verified(universe(), bytes).unwrap();
        let object_key = cas_object_key(universe(), hash);

        {
            let mut guard = persistence.state.lock().unwrap();
            guard.cas_objects.insert(object_key, b"tampered".to_vec());
        }

        let err = persistence.cas_get(universe(), hash).unwrap_err();
        assert!(matches!(
            err,
            PersistError::Corrupt(PersistCorruption::CasBodyHashMismatch { .. })
        ));
    }

    #[test]
    fn journal_append_batch_conflicts_on_head_mismatch() {
        let persistence = MemoryWorldPersistence::new();
        let first = persistence
            .journal_append_batch(universe(), world(), 0, &[b"a".to_vec(), b"b".to_vec()])
            .unwrap();
        assert_eq!(first, 0);
        let err = persistence
            .journal_append_batch(universe(), world(), 0, &[b"c".to_vec()])
            .unwrap_err();
        assert!(matches!(
            err,
            PersistError::Conflict(PersistConflict::HeadAdvanced {
                expected: 0,
                actual: 2
            })
        ));
        assert_eq!(persistence.journal_head(universe(), world()).unwrap(), 2);
    }

    #[test]
    fn journal_read_range_detects_missing_entry_in_requested_window() {
        let persistence = MemoryWorldPersistence::new();
        persistence
            .journal_append_batch(
                universe(),
                world(),
                0,
                &[b"one".to_vec(), b"two".to_vec(), b"three".to_vec()],
            )
            .unwrap();
        persistence.debug_remove_journal_entry(universe(), world(), 1);

        let err = persistence
            .journal_read_range(universe(), world(), 0, 3)
            .unwrap_err();
        assert!(matches!(
            err,
            PersistError::Corrupt(PersistCorruption::MissingJournalEntry { height: 1 })
        ));
    }

    #[test]
    fn inbox_cursor_is_monotonic_and_compare_and_swap() {
        let persistence = MemoryWorldPersistence::new();
        let first = persistence
            .inbox_enqueue(universe(), world(), timer_ingress(1))
            .unwrap();
        let second = persistence
            .inbox_enqueue(universe(), world(), timer_ingress(2))
            .unwrap();

        persistence
            .inbox_commit_cursor(universe(), world(), None, first.clone())
            .unwrap();

        let err = persistence
            .inbox_commit_cursor(universe(), world(), None, second.clone())
            .unwrap_err();
        assert!(matches!(
            err,
            PersistError::Conflict(PersistConflict::InboxCursorAdvanced { .. })
        ));

        let err = persistence
            .inbox_commit_cursor(universe(), world(), Some(first.clone()), first.clone())
            .unwrap();
        assert_eq!(err, ());

        let err = persistence
            .inbox_commit_cursor(
                universe(),
                world(),
                Some(first.clone()),
                InboxSeq::from_u64(0),
            )
            .unwrap();
        assert_eq!(err, ());

        persistence
            .inbox_commit_cursor(universe(), world(), Some(first), second.clone())
            .unwrap();
        assert_eq!(
            persistence.inbox_cursor(universe(), world()).unwrap(),
            Some(second)
        );
    }

    #[test]
    fn inbox_enqueue_externalizes_large_payloads_to_cas() {
        let persistence = MemoryWorldPersistence::with_config(PersistenceConfig {
            inbox: crate::InboxConfig {
                inline_payload_threshold_bytes: 4,
            },
            ..PersistenceConfig::default()
        });
        let payload = b"payload larger than threshold".to_vec();
        let seq = persistence
            .inbox_enqueue(universe(), world(), control_ingress(&payload))
            .unwrap();

        let stored = persistence
            .inbox_read_after(universe(), world(), None, 1)
            .unwrap()
            .into_iter()
            .find(|(item_seq, _)| *item_seq == seq)
            .map(|(_, item)| item)
            .unwrap();

        match stored {
            InboxItem::Control(control) => {
                assert_eq!(control.payload.inline_cbor, None);
                let hash =
                    Hash::from_hex_str(control.payload.cbor_ref.as_deref().unwrap()).unwrap();
                assert_eq!(
                    control.payload.cbor_sha256.as_deref(),
                    Some(hash.to_hex().as_str())
                );
                assert_eq!(control.payload.cbor_size, Some(payload.len() as u64));
                assert_eq!(persistence.cas_get(universe(), hash).unwrap(), payload);
            }
            other => panic!("unexpected inbox item: {other:?}"),
        }
    }

    #[test]
    fn drain_inbox_to_journal_updates_head_and_cursor_atomically() {
        let persistence = MemoryWorldPersistence::new();
        let first = persistence
            .inbox_enqueue(universe(), world(), timer_ingress(1))
            .unwrap();
        let second = persistence
            .inbox_enqueue(universe(), world(), timer_ingress(2))
            .unwrap();

        let first_height = persistence
            .drain_inbox_to_journal(
                universe(),
                world(),
                None,
                second.clone(),
                0,
                &[b"j1".to_vec(), b"j2".to_vec()],
            )
            .unwrap();
        assert_eq!(first_height, 0);
        assert_eq!(
            persistence.inbox_cursor(universe(), world()).unwrap(),
            Some(second)
        );
        assert_eq!(persistence.journal_head(universe(), world()).unwrap(), 2);
        assert_eq!(
            persistence
                .journal_read_range(universe(), world(), 0, 2)
                .unwrap(),
            vec![(0, b"j1".to_vec()), (1, b"j2".to_vec())]
        );

        let err = persistence
            .drain_inbox_to_journal(
                universe(),
                world(),
                Some(first),
                InboxSeq::from_u64(1),
                0,
                &[b"j3".to_vec()],
            )
            .unwrap_err();
        assert!(matches!(
            err,
            PersistError::Conflict(PersistConflict::InboxCursorAdvanced { .. })
        ));
        assert_eq!(persistence.journal_head(universe(), world()).unwrap(), 2);
    }

    #[test]
    fn snapshot_index_and_baseline_are_monotonic() {
        let persistence = MemoryWorldPersistence::new();
        let s1 = snapshot(2, "sha256:s1");
        let s2 = snapshot(4, "sha256:s2");

        persistence
            .snapshot_index(universe(), world(), s1.clone())
            .unwrap();
        persistence
            .snapshot_promote_baseline(universe(), world(), s1.clone())
            .unwrap();
        persistence
            .snapshot_index(universe(), world(), s2.clone())
            .unwrap();
        persistence
            .snapshot_promote_baseline(universe(), world(), s2.clone())
            .unwrap();

        let err = persistence
            .snapshot_promote_baseline(universe(), world(), s1.clone())
            .unwrap_err();
        assert!(matches!(err, PersistError::Validation(_)));
        assert_eq!(
            persistence
                .snapshot_active_baseline(universe(), world())
                .unwrap(),
            s2
        );
    }

    #[test]
    fn snapshot_commit_appends_journal_and_promotes_baseline_atomically() {
        let persistence = MemoryWorldPersistence::new();
        let snapshot_bytes = b"kernel-snapshot".to_vec();
        let snapshot_hash = Hash::of_bytes(&snapshot_bytes);
        let record = SnapshotRecord {
            snapshot_ref: snapshot_hash.to_hex(),
            height: 0,
            logical_time_ns: 0,
            receipt_horizon_height: Some(0),
            manifest_hash: Some("sha256:manifest".into()),
        };

        let result = persistence
            .snapshot_commit(
                universe(),
                world(),
                SnapshotCommitRequest {
                    expected_head: 0,
                    snapshot_bytes,
                    record: record.clone(),
                    snapshot_journal_entry: b"snapshot".to_vec(),
                    baseline_journal_entry: Some(b"baseline".to_vec()),
                    promote_baseline: true,
                },
            )
            .unwrap();

        assert_eq!(result.snapshot_hash, snapshot_hash);
        assert_eq!(result.first_height, 0);
        assert_eq!(result.next_head, 2);
        assert!(result.baseline_promoted);
        assert_eq!(
            persistence
                .journal_read_range(universe(), world(), 0, 8)
                .unwrap(),
            vec![(0, b"snapshot".to_vec()), (1, b"baseline".to_vec())]
        );
        assert_eq!(
            persistence
                .snapshot_active_baseline(universe(), world())
                .unwrap(),
            record
        );
    }

    #[test]
    fn snapshot_promotion_requires_receipt_horizon_equal_height() {
        let persistence = MemoryWorldPersistence::new();
        let record = SnapshotRecord {
            snapshot_ref: "sha256:snapshot".into(),
            height: 2,
            logical_time_ns: 20,
            receipt_horizon_height: None,
            manifest_hash: Some("sha256:manifest".into()),
        };

        persistence
            .snapshot_index(universe(), world(), record.clone())
            .unwrap();
        let err = persistence
            .snapshot_promote_baseline(universe(), world(), record)
            .unwrap_err();
        assert!(matches!(err, PersistError::Validation(_)));
    }

    #[test]
    fn segment_index_is_immutable_per_end_height() {
        let persistence = MemoryWorldPersistence::new();
        let first = SegmentIndexRecord {
            segment: crate::SegmentId::new(0, 9).unwrap(),
            object_key: "segments/u/w/0-9.log".into(),
            checksum: "sha256:first".into(),
        };
        let replacement = SegmentIndexRecord {
            segment: crate::SegmentId::new(0, 9).unwrap(),
            object_key: "segments/u/w/0-9-v2.log".into(),
            checksum: "sha256:second".into(),
        };

        persistence
            .segment_index_put(universe(), world(), first.clone())
            .unwrap();
        let err = persistence
            .segment_index_put(universe(), world(), replacement)
            .unwrap_err();
        assert!(matches!(err, PersistError::Conflict(_)));
        assert_eq!(
            persistence
                .segment_index_read_from(universe(), world(), 0, 8)
                .unwrap(),
            vec![first]
        );
    }
}
