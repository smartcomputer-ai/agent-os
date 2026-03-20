use super::*;

impl WorldStore for MemoryWorldPersistence {
    fn cas_put_verified(&self, universe: UniverseId, bytes: &[u8]) -> Result<Hash, PersistError> {
        self.cas.put_verified(universe, bytes)
    }

    fn cas_get(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, PersistError> {
        self.cas.get(universe, hash)
    }

    fn cas_has(&self, universe: UniverseId, hash: Hash) -> Result<bool, PersistError> {
        self.cas.has(universe, hash)
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
            self.journal_append_batch_inner(world_state, expected_head, entries)
        })
    }

    fn journal_read_range(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_inclusive: JournalHeight,
        limit: u32,
    ) -> Result<Vec<(JournalHeight, Vec<u8>)>, PersistError> {
        let head = self.journal_head(universe, world)?;
        if from_inclusive >= head || limit == 0 {
            return Ok(Vec::new());
        }

        let end_exclusive = head.min(from_inclusive.saturating_add(limit as u64));
        let mut segments =
            self.segment_index_read_from(universe, world, from_inclusive, u32::MAX)?;
        segments.sort_by_key(|record| record.segment.start);

        let mut next_segment = 0usize;
        let mut loaded_segment: Option<(crate::SegmentId, Vec<(JournalHeight, Vec<u8>)>)> = None;
        let mut entries = Vec::with_capacity((end_exclusive - from_inclusive) as usize);

        for height in from_inclusive..end_exclusive {
            if let Some((segment, segment_entries)) = loaded_segment.as_ref() {
                if height >= segment.start && height <= segment.end {
                    let index = (height - segment.start) as usize;
                    let (entry_height, raw) = &segment_entries[index];
                    if *entry_height != height {
                        return Err(PersistCorruption::MissingJournalEntry { height }.into());
                    }
                    entries.push((height, raw.clone()));
                    continue;
                }
            }
            loaded_segment = None;

            while next_segment < segments.len() && segments[next_segment].segment.end < height {
                next_segment += 1;
            }

            if let Some(record) = segments.get(next_segment) {
                if record.segment.start <= height && height <= record.segment.end {
                    let segment_entries =
                        self.segment_read_entries(universe, world, record.segment)?;
                    let index = (height - record.segment.start) as usize;
                    let (entry_height, raw) = segment_entries
                        .get(index)
                        .ok_or(PersistCorruption::MissingJournalEntry { height })?;
                    if *entry_height != height {
                        return Err(PersistCorruption::MissingJournalEntry { height }.into());
                    }
                    entries.push((height, raw.clone()));
                    loaded_segment = Some((record.segment, segment_entries));
                    next_segment += 1;
                    continue;
                }
            }

            let raw = self.with_world(universe, world, |world_state| {
                Ok(world_state.journal_entries.get(&height).cloned())
            })?;
            let raw = raw.ok_or(PersistCorruption::MissingJournalEntry { height })?;
            entries.push((height, raw));
        }

        Ok(entries)
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
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let seq = {
            let world_state = state.worlds.entry((universe, world)).or_default();
            let seq = InboxSeq::from_u64(world_state.next_inbox_seq);
            world_state.next_inbox_seq = world_state.next_inbox_seq.saturating_add(1);
            world_state.inbox_entries.insert(seq.clone(), item);
            world_state.notify_counter = world_state.notify_counter.saturating_add(1);
            seq
        };
        Self::sync_ready_state(&mut state, universe, world, 0, self.config);
        Ok(seq)
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
            Self::inbox_commit_cursor_inner(world_state, old_cursor, new_cursor)
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
            Self::inbox_commit_cursor_inner(world_state, old_cursor, new_cursor)?;
            let first_height =
                self.journal_append_batch_inner(world_state, expected_head, journal_entries)?;
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
            if let Some(existing) = world_state.snapshots.get(&record.height) {
                if existing == &record {
                    return Ok(());
                }
                if !can_upgrade_snapshot_record(existing, &record) {
                    return Err(PersistConflict::SnapshotExists {
                        height: record.height,
                    }
                    .into());
                }
            }
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

        let result = self.with_world_mut(universe, world, |world_state| {
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
                Self::update_meta_from_baseline(world_state, &request.record);
            }

            Ok(SnapshotCommitResult {
                snapshot_hash,
                first_height,
                next_head: world_state.journal_head,
                baseline_promoted: request.promote_baseline,
            })
        })?;
        let mut state = self.state.lock().unwrap();
        Self::sync_ready_state(&mut state, universe, world, 0, self.config);
        Ok(result)
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

    fn snapshot_latest(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<SnapshotRecord, PersistError> {
        self.with_world(universe, world, |world_state| {
            world_state
                .snapshots
                .last_key_value()
                .map(|entry| entry.1.clone())
                .ok_or_else(|| PersistError::not_found("latest snapshot"))
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
            Self::update_meta_from_baseline(world_state, &record);
            world_state.active_baseline = Some(record);
            Ok(())
        })?;
        let mut state = self.state.lock().unwrap();
        Self::sync_ready_state(&mut state, universe, world, 0, self.config);
        Ok(())
    }

    fn snapshot_repair_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        validate_snapshot_record(&record)?;
        self.with_world_mut(universe, world, |world_state| {
            world_state.snapshots.insert(record.height, record.clone());
            if world_state
                .active_baseline
                .as_ref()
                .is_some_and(|active| active.height == record.height)
            {
                Self::update_meta_from_baseline(world_state, &record);
                world_state.active_baseline = Some(record);
            }
            Ok(())
        })?;
        let mut state = self.state.lock().unwrap();
        Self::sync_ready_state(&mut state, universe, world, 0, self.config);
        Ok(())
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

    fn segment_export(
        &self,
        universe: UniverseId,
        world: WorldId,
        request: SegmentExportRequest,
    ) -> Result<SegmentExportResult, PersistError> {
        validate_segment_export_request(&request)?;
        let mut state = self.state.lock().unwrap();
        let record = {
            let world_state = state.worlds.entry((universe, world)).or_default();
            let baseline = world_state.active_baseline.clone().ok_or_else(|| {
                PersistError::validation("segment export requires active baseline")
            })?;
            let safe_exclusive_end = baseline.height.saturating_sub(request.hot_tail_margin);
            if request.segment.end >= safe_exclusive_end {
                return Err(PersistError::validation(format!(
                    "segment end {} must be strictly below active baseline {} with hot-tail margin {}",
                    request.segment.end, baseline.height, request.hot_tail_margin
                )));
            }
            if request.segment.end >= world_state.journal_head {
                return Err(PersistError::validation(format!(
                    "segment end {} must be below current journal head {}",
                    request.segment.end, world_state.journal_head
                )));
            }

            let existing_record = world_state.segments.get(&request.segment.end).cloned();
            if let Some(existing) = existing_record {
                if existing.segment != request.segment {
                    return Err(PersistConflict::SegmentExists {
                        end_height: request.segment.end,
                    }
                    .into());
                }
                let body_hash = Hash::from_hex_str(&existing.body_ref).map_err(|err| {
                    PersistError::validation(format!(
                        "invalid segment body_ref '{}': {err}",
                        existing.body_ref
                    ))
                })?;
                if !self.cas_has(universe, body_hash)? {
                    return Err(PersistCorruption::MissingSegmentBody {
                        segment: request.segment,
                        hash: body_hash,
                    }
                    .into());
                }
                existing
            } else {
                let mut entries = Vec::new();
                for height in request.segment.start..=request.segment.end {
                    let entry = world_state
                        .journal_entries
                        .get(&height)
                        .cloned()
                        .ok_or(PersistCorruption::MissingJournalEntry { height })?;
                    entries.push((height, entry));
                }
                let segment_bytes = encode_segment_entries(request.segment, &entries)?;
                let body_hash = self.cas_put_verified(universe, &segment_bytes)?;
                let record = SegmentIndexRecord {
                    segment: request.segment,
                    body_ref: body_hash.to_hex(),
                    checksum: segment_checksum(&segment_bytes),
                };
                world_state
                    .segments
                    .insert(request.segment.end, record.clone());
                record
            }
        };

        let world_state = state.worlds.get_mut(&(universe, world)).unwrap();
        let mut deleted_entries = 0u64;
        let mut chunk_start = request.segment.start;
        while chunk_start <= request.segment.end {
            let chunk_end =
                (chunk_start + request.delete_chunk_entries as u64 - 1).min(request.segment.end);
            for height in chunk_start..=chunk_end {
                if world_state.journal_entries.remove(&height).is_some() {
                    deleted_entries += 1;
                }
            }
            chunk_start = chunk_end.saturating_add(1);
        }

        let result = SegmentExportResult {
            record,
            exported_entries: request.segment.end - request.segment.start + 1,
            deleted_entries,
        };
        Self::sync_ready_state(&mut state, universe, world, 0, self.config);
        Ok(result)
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
                .map(|entry| entry.1.clone())
                .collect())
        })
    }

    fn segment_read_entries(
        &self,
        universe: UniverseId,
        world: WorldId,
        segment: crate::SegmentId,
    ) -> Result<Vec<(JournalHeight, Vec<u8>)>, PersistError> {
        let state = self.state.lock().unwrap();
        let world_state = state.worlds.get(&(universe, world)).ok_or_else(|| {
            PersistError::not_found(format!("world {universe}/{world} not found"))
        })?;
        let record = world_state
            .segments
            .get(&segment.end)
            .cloned()
            .ok_or_else(|| PersistError::not_found(format!("segment {:?}", segment)))?;
        if record.segment != segment {
            return Err(PersistError::not_found(format!("segment {:?}", segment)));
        }
        let body_hash = Hash::from_hex_str(&record.body_ref).map_err(|err| {
            PersistError::validation(format!(
                "invalid segment body_ref '{}': {err}",
                record.body_ref
            ))
        })?;
        drop(state);
        let bytes = self.cas_get(universe, body_hash).map_err(|err| match err {
            PersistError::NotFound(_) => PersistCorruption::MissingSegmentBody {
                segment,
                hash: body_hash,
            }
            .into(),
            other => other,
        })?;
        decode_segment_entries(&record, &bytes)
    }
}
