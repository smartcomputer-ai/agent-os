use super::*;

impl WorldStore for FdbWorldPersistence {
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
        let head_key = self.journal_head_key(universe, world);
        let meta_key = self.world_meta_key(universe, world);
        let catalog_key = self.world_catalog_key(universe, world);
        let meta_bytes = self.encode(&sample_world_meta(world))?;
        let entry_space = self.journal_entry_space(universe, world);
        let entries = entries.to_vec();
        self.run(|trx, _| {
            let head_key = head_key.clone();
            let meta_key = meta_key.clone();
            let catalog_key = catalog_key.clone();
            let meta_bytes = meta_bytes.clone();
            let entry_space = entry_space.clone();
            let entries = entries.clone();
            async move {
                if trx.get(&meta_key, false).await?.is_none() {
                    trx.set(&meta_key, &meta_bytes);
                    trx.set(&catalog_key, &meta_bytes);
                }
                let head_bytes = trx.get(&head_key, false).await?;
                let actual_head = match head_bytes {
                    Some(bytes) => decode_u64_static(bytes.as_ref())?,
                    None => 0,
                };
                if actual_head != expected_head {
                    return Err(custom_persist_error(
                        PersistConflict::HeadAdvanced {
                            expected: expected_head,
                            actual: actual_head,
                        }
                        .into(),
                    ));
                }
                let mut height = actual_head;
                for entry in &entries {
                    let key = entry_space.pack(&(to_i64_static(height, "journal height")?,));
                    trx.set(&key, entry);
                    height += 1;
                }
                trx.set(&head_key, &height.to_be_bytes());
                Ok(actual_head)
            }
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
        let mut segments = self.segment_index_read_all_from(universe, world, from_inclusive)?;
        segments.sort_by_key(|record| record.segment.start);

        let mut next_segment = 0usize;
        let mut loaded_segment: Option<(SegmentId, Vec<(JournalHeight, Vec<u8>)>)> = None;
        let mut entries = Vec::with_capacity((end_exclusive - from_inclusive) as usize);

        let mut hot_cursor = from_inclusive;
        while hot_cursor < end_exclusive {
            if let Some((segment, segment_entries)) = loaded_segment.as_ref() {
                if hot_cursor >= segment.start && hot_cursor <= segment.end {
                    let index = (hot_cursor - segment.start) as usize;
                    let (entry_height, raw) = &segment_entries[index];
                    if *entry_height != hot_cursor {
                        return Err(
                            PersistCorruption::MissingJournalEntry { height: hot_cursor }.into(),
                        );
                    }
                    entries.push((hot_cursor, raw.clone()));
                    hot_cursor += 1;
                    continue;
                }
            }
            loaded_segment = None;

            while next_segment < segments.len() && segments[next_segment].segment.end < hot_cursor {
                next_segment += 1;
            }

            if let Some(record) = segments.get(next_segment) {
                if hot_cursor < record.segment.start {
                    let hot_end = record.segment.start.min(end_exclusive);
                    entries
                        .extend(self.journal_hot_read_range(universe, world, hot_cursor, hot_end)?);
                    hot_cursor = hot_end;
                    continue;
                }
                if record.segment.start <= hot_cursor && hot_cursor <= record.segment.end {
                    let segment_entries =
                        self.segment_read_entries(universe, world, record.segment)?;
                    let index = (hot_cursor - record.segment.start) as usize;
                    let (entry_height, raw) = segment_entries
                        .get(index)
                        .ok_or(PersistCorruption::MissingJournalEntry { height: hot_cursor })?;
                    if *entry_height != hot_cursor {
                        return Err(
                            PersistCorruption::MissingJournalEntry { height: hot_cursor }.into(),
                        );
                    }
                    entries.push((hot_cursor, raw.clone()));
                    loaded_segment = Some((record.segment, segment_entries));
                    next_segment += 1;
                    hot_cursor += 1;
                    continue;
                }
            }

            entries.extend(self.journal_hot_read_range(
                universe,
                world,
                hot_cursor,
                end_exclusive,
            )?);
            break;
        }

        Ok(entries)
    }

    fn journal_head(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<JournalHeight, PersistError> {
        let head_key = self.journal_head_key(universe, world);
        self.run(|trx, _| {
            let head_key = head_key.clone();
            async move {
                let value = trx.get(&head_key, false).await?;
                Ok(match value {
                    Some(bytes) => decode_u64_static(bytes.as_ref())?,
                    None => 0,
                })
            }
        })
    }

    fn inbox_enqueue(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError> {
        let item = self.normalize_inbox_item(universe, item)?;
        let value = self.encode(&item)?;
        let inbox_space = self.inbox_entry_space(universe, world);
        let notify_key = self.notify_counter_key(universe, world);
        let meta_key = self.world_meta_key(universe, world);
        let catalog_key = self.world_catalog_key(universe, world);
        let meta_bytes = self.encode(&sample_world_meta(world))?;

        loop {
            let trx = self.db.create_trx().map_err(map_fdb_error)?;
            let versionstamp = trx.get_versionstamp();
            let inbox_key = inbox_space.pack_with_versionstamp(&Versionstamp::incomplete(0));

            let op_result: Result<(), TxRetryError> = block_on(async {
                if trx
                    .get(&meta_key, false)
                    .await
                    .map_err(TxRetryError::Fdb)?
                    .is_none()
                {
                    trx.set(&meta_key, &meta_bytes);
                    trx.set(&catalog_key, &meta_bytes);
                }
                let notify = match trx.get(&notify_key, false).await {
                    Ok(Some(bytes)) => decode_u64_static(bytes.as_ref())
                        .map_err(map_fdb_binding_error)
                        .map_err(TxRetryError::Persist)?,
                    Ok(None) => 0,
                    Err(err) => return Err(TxRetryError::Fdb(err)),
                };
                trx.atomic_op(&inbox_key, &value, MutationType::SetVersionstampedKey);
                trx.set(&notify_key, &(notify.saturating_add(1)).to_be_bytes());
                self.mark_world_pending_inbox_in_tx(&trx, universe, world, 0)
                    .await
                    .map_err(map_fdb_binding_error)
                    .map_err(TxRetryError::Persist)?;
                Ok(())
            });

            match op_result {
                Ok(()) => match block_on(trx.commit()) {
                    Ok(_) => {
                        let committed = block_on(versionstamp).map_err(map_fdb_error)?;
                        let tr_version: [u8; 10] = committed.as_ref().try_into().map_err(|_| {
                            PersistError::backend(
                                "foundationdb returned non-10-byte transaction versionstamp",
                            )
                        })?;
                        let complete = Versionstamp::complete(tr_version, 0);
                        let packed = inbox_space.pack(&(complete,));
                        let prefix_len = inbox_space.bytes().len();
                        return Ok(InboxSeq::new(packed[prefix_len..].to_vec()));
                    }
                    Err(err) => {
                        block_on(err.on_error()).map_err(map_fdb_error)?;
                    }
                },
                Err(TxRetryError::Fdb(err)) => {
                    block_on(trx.on_error(err)).map_err(map_fdb_error)?;
                }
                Err(TxRetryError::Persist(err)) => return Err(err),
            }
        }
    }

    fn inbox_read_after(
        &self,
        universe: UniverseId,
        world: WorldId,
        after_exclusive: Option<InboxSeq>,
        limit: u32,
    ) -> Result<Vec<(InboxSeq, InboxItem)>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let inbox_space = self.inbox_entry_space(universe, world);
        let (begin, end) = inbox_space.range();
        let prefix_len = inbox_space.bytes().len();
        self.run(|trx, _| {
            let inbox_space = inbox_space.clone();
            let begin = begin.clone();
            let end = end.clone();
            let after_exclusive = after_exclusive.clone();
            async move {
                let mut range = RangeOption::from((begin.clone(), end));
                if let Some(after) = after_exclusive {
                    let mut after_key = inbox_space.bytes().to_vec();
                    after_key.extend_from_slice(after.as_bytes());
                    range.begin = KeySelector::first_greater_than(after_key);
                }
                range.limit = Some(limit as usize);
                let kvs = trx.get_range(&range, 1, false).await?;
                let mut items = Vec::with_capacity(kvs.len());
                for kv in kvs.iter() {
                    let key = kv.key();
                    if !key.starts_with(inbox_space.bytes()) {
                        continue;
                    }
                    let seq = InboxSeq::new(key[prefix_len..].to_vec());
                    let item: InboxItem =
                        serde_cbor::from_slice(kv.value().as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    items.push((seq, item));
                }
                Ok(items)
            }
        })
    }

    fn inbox_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<InboxSeq>, PersistError> {
        let cursor_key = self.inbox_cursor_key(universe, world);
        self.run(|trx, _| {
            let cursor_key = cursor_key.clone();
            async move {
                Ok(trx
                    .get(&cursor_key, false)
                    .await?
                    .map(|bytes| InboxSeq::new(bytes.as_ref().to_vec())))
            }
        })
    }

    fn inbox_commit_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
    ) -> Result<(), PersistError> {
        let cursor_key = self.inbox_cursor_key(universe, world);
        let inbox_space = self.inbox_entry_space(universe, world);
        self.run(|trx, _| {
            let cursor_key = cursor_key.clone();
            let inbox_space = inbox_space.clone();
            let old_cursor = old_cursor.clone();
            let new_cursor = new_cursor.clone();
            async move {
                let actual_cursor = trx
                    .get(&cursor_key, false)
                    .await?
                    .map(|bytes| InboxSeq::new(bytes.as_ref().to_vec()));
                if actual_cursor != old_cursor {
                    return Err(custom_persist_error(
                        PersistConflict::InboxCursorAdvanced {
                            expected: old_cursor,
                            actual: actual_cursor,
                        }
                        .into(),
                    ));
                }
                if let Some(current) = &actual_cursor {
                    if new_cursor < *current {
                        return Err(custom_persist_error(PersistError::validation(
                            "inbox cursor cannot regress",
                        )));
                    }
                }
                let inbox_key = build_inbox_key(&inbox_space, &new_cursor);
                if trx.get(&inbox_key, false).await?.is_none() {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "inbox sequence {new_cursor} does not exist"
                    ))));
                }
                trx.set(&cursor_key, new_cursor.as_bytes());
                self.refresh_world_ready_state_in_tx(&trx, universe, world, 0)
                    .await?;
                Ok(())
            }
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
        let cursor_key = self.inbox_cursor_key(universe, world);
        let inbox_space = self.inbox_entry_space(universe, world);
        let head_key = self.journal_head_key(universe, world);
        let journal_space = self.journal_entry_space(universe, world);
        let journal_entries = journal_entries.to_vec();
        self.run(|trx, _| {
            let cursor_key = cursor_key.clone();
            let inbox_space = inbox_space.clone();
            let head_key = head_key.clone();
            let journal_space = journal_space.clone();
            let old_cursor = old_cursor.clone();
            let new_cursor = new_cursor.clone();
            let journal_entries = journal_entries.clone();
            async move {
                let actual_cursor = trx
                    .get(&cursor_key, false)
                    .await?
                    .map(|bytes| InboxSeq::new(bytes.as_ref().to_vec()));
                if actual_cursor != old_cursor {
                    return Err(custom_persist_error(
                        PersistConflict::InboxCursorAdvanced {
                            expected: old_cursor,
                            actual: actual_cursor,
                        }
                        .into(),
                    ));
                }
                if let Some(current) = &actual_cursor {
                    if new_cursor < *current {
                        return Err(custom_persist_error(PersistError::validation(
                            "inbox cursor cannot regress",
                        )));
                    }
                }
                let new_cursor_key = build_inbox_key(&inbox_space, &new_cursor);
                if trx.get(&new_cursor_key, false).await?.is_none() {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "inbox sequence {new_cursor} does not exist"
                    ))));
                }
                let actual_head = match trx.get(&head_key, false).await? {
                    Some(bytes) => decode_u64_static(bytes.as_ref())?,
                    None => 0,
                };
                if actual_head != expected_head {
                    return Err(custom_persist_error(
                        PersistConflict::HeadAdvanced {
                            expected: expected_head,
                            actual: actual_head,
                        }
                        .into(),
                    ));
                }
                let mut height = actual_head;
                for entry in &journal_entries {
                    let key = journal_space.pack(&(to_i64_static(height, "journal height")?,));
                    trx.set(&key, entry);
                    height += 1;
                }
                trx.set(&head_key, &height.to_be_bytes());
                trx.set(&cursor_key, new_cursor.as_bytes());
                Ok(actual_head)
            }
        })
    }

    fn snapshot_index(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        validate_snapshot_record(&record)?;
        let space = self.snapshot_by_height_space(universe, world);
        let key = space.pack(&(self.to_i64(record.height, "snapshot height")?,));
        let value = self.encode(&record)?;
        let meta_key = self.world_meta_key(universe, world);
        let catalog_key = self.world_catalog_key(universe, world);
        let meta_bytes = self.encode(&sample_world_meta(world))?;
        self.run(|trx, _| {
            let key = key.clone();
            let value = value.clone();
            let meta_key = meta_key.clone();
            let catalog_key = catalog_key.clone();
            let meta_bytes = meta_bytes.clone();
            let record = record.clone();
            async move {
                if trx.get(&meta_key, false).await?.is_none() {
                    trx.set(&meta_key, &meta_bytes);
                    trx.set(&catalog_key, &meta_bytes);
                }
                if let Some(existing) = trx.get(&key, false).await? {
                    let existing_record: SnapshotRecord = serde_cbor::from_slice(existing.as_ref())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if existing_record == record {
                        return Ok(());
                    }
                    if can_upgrade_snapshot_record(&existing_record, &record) {
                        trx.set(&key, &value);
                        return Ok(());
                    }
                    return Err(custom_persist_error(
                        PersistConflict::SnapshotExists {
                            height: record.height,
                        }
                        .into(),
                    ));
                }
                trx.set(&key, &value);
                Ok(())
            }
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

        let mut journal_entries = vec![request.snapshot_journal_entry.clone()];
        if let Some(baseline_entry) = &request.baseline_journal_entry {
            journal_entries.push(baseline_entry.clone());
        }
        self.validate_journal_batch(&journal_entries)?;

        let snapshot_key = self
            .snapshot_by_height_space(universe, world)
            .pack(&(self.to_i64(request.record.height, "snapshot height")?,));
        let baseline_key = self.baseline_active_key(universe, world);
        let meta_key = self.world_meta_key(universe, world);
        let catalog_key = self.world_catalog_key(universe, world);
        let head_key = self.journal_head_key(universe, world);
        let journal_space = self.journal_entry_space(universe, world);
        let record = request.record.clone();
        let record_bytes = self.encode(&record)?;
        let default_meta_bytes = self.encode(&sample_world_meta(world))?;
        let expected_head = request.expected_head;
        let promote_baseline = request.promote_baseline;

        let result = self.run(|trx, _| {
            let snapshot_key = snapshot_key.clone();
            let baseline_key = baseline_key.clone();
            let meta_key = meta_key.clone();
            let catalog_key = catalog_key.clone();
            let head_key = head_key.clone();
            let journal_space = journal_space.clone();
            let record = record.clone();
            let record_bytes = record_bytes.clone();
            let default_meta_bytes = default_meta_bytes.clone();
            let journal_entries = journal_entries.clone();
            async move {
                let actual_head = match trx.get(&head_key, false).await? {
                    Some(bytes) => decode_u64_static(bytes.as_ref())?,
                    None => 0,
                };
                if actual_head != expected_head {
                    return Err(custom_persist_error(
                        PersistConflict::HeadAdvanced {
                            expected: expected_head,
                            actual: actual_head,
                        }
                        .into(),
                    ));
                }

                if let Some(existing) = trx.get(&snapshot_key, false).await? {
                    let existing_record: SnapshotRecord = serde_cbor::from_slice(existing.as_ref())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if existing_record != record {
                        return Err(custom_persist_error(
                            PersistConflict::SnapshotExists {
                                height: record.height,
                            }
                            .into(),
                        ));
                    }
                } else {
                    trx.set(&snapshot_key, &record_bytes);
                }

                if promote_baseline {
                    if let Some(active_bytes) = trx.get(&baseline_key, false).await? {
                        let active: SnapshotRecord = serde_cbor::from_slice(active_bytes.as_ref())
                            .map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        if record.height < active.height {
                            return Err(custom_persist_error(PersistError::validation(format!(
                                "baseline cannot regress from {} to {}",
                                active.height, record.height
                            ))));
                        }
                        if record.height == active.height && active != record {
                            return Err(custom_persist_error(
                                PersistConflict::BaselineMismatch {
                                    height: record.height,
                                }
                                .into(),
                            ));
                        }
                    }
                }

                let first_height = actual_head;
                let mut height = actual_head;
                for entry in &journal_entries {
                    let key = journal_space.pack(&(to_i64_static(height, "journal height")?,));
                    trx.set(&key, entry);
                    height += 1;
                }
                trx.set(&head_key, &height.to_be_bytes());
                if promote_baseline {
                    let mut meta = match trx.get(&meta_key, false).await? {
                        Some(bytes) => serde_cbor::from_slice::<WorldMeta>(bytes.as_ref())
                            .map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?,
                        None => sample_world_meta(world),
                    };
                    meta.active_baseline_height = Some(record.height);
                    meta.manifest_hash = record.manifest_hash.clone();
                    let meta_bytes = serde_cbor::to_vec(&meta).map_err(|err| {
                        custom_persist_error(PersistError::backend(err.to_string()))
                    })?;
                    trx.set(&baseline_key, &record_bytes);
                    trx.set(&meta_key, &meta_bytes);
                    trx.set(&catalog_key, &meta_bytes);
                } else if trx.get(&meta_key, false).await?.is_none() {
                    trx.set(&meta_key, &default_meta_bytes);
                    trx.set(&catalog_key, &default_meta_bytes);
                }

                Ok(SnapshotCommitResult {
                    snapshot_hash,
                    first_height,
                    next_head: height,
                    baseline_promoted: promote_baseline,
                })
            }
        })?;
        self.run(|trx, _| async move {
            self.refresh_world_ready_state_in_trx(&trx, universe, world, 0)
                .await?;
            Ok(())
        })?;
        Ok(result)
    }

    fn snapshot_at_height(
        &self,
        universe: UniverseId,
        world: WorldId,
        height: JournalHeight,
    ) -> Result<SnapshotRecord, PersistError> {
        let space = self.snapshot_by_height_space(universe, world);
        let key = space.pack(&(self.to_i64(height, "snapshot height")?,));
        self.run(|trx, _| {
            let key = key.clone();
            async move {
                let value = trx.get(&key, false).await?.ok_or_else(|| {
                    custom_persist_error(PersistError::not_found(format!(
                        "snapshot at height {height}"
                    )))
                })?;
                serde_cbor::from_slice(value.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            }
        })
    }

    fn snapshot_latest(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<SnapshotRecord, PersistError> {
        let space = self.snapshot_by_height_space(universe, world);
        self.run(|trx, _| {
            let space = space.clone();
            async move {
                let (begin, end) = space.range();
                let mut range = RangeOption::from((begin, end));
                range.limit = Some(1);
                range.reverse = true;
                let entries = trx.get_range(&range, 1, false).await?;
                let value = entries.first().ok_or_else(|| {
                    custom_persist_error(PersistError::not_found("latest snapshot"))
                })?;
                serde_cbor::from_slice(value.value().as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            }
        })
    }

    fn snapshot_active_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<SnapshotRecord, PersistError> {
        let key = self.baseline_active_key(universe, world);
        self.run(|trx, _| {
            let key = key.clone();
            async move {
                let value = trx.get(&key, false).await?.ok_or_else(|| {
                    custom_persist_error(PersistError::not_found("active baseline"))
                })?;
                serde_cbor::from_slice(value.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            }
        })
    }

    fn snapshot_promote_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        validate_baseline_promotion_record(&record)?;
        let snapshot_key = self
            .snapshot_by_height_space(universe, world)
            .pack(&(self.to_i64(record.height, "snapshot height")?,));
        let baseline_key = self.baseline_active_key(universe, world);
        let meta_key = self.world_meta_key(universe, world);
        let catalog_key = self.world_catalog_key(universe, world);
        let record_bytes = self.encode(&record)?;
        self.run(|trx, _| {
            let snapshot_key = snapshot_key.clone();
            let baseline_key = baseline_key.clone();
            let meta_key = meta_key.clone();
            let catalog_key = catalog_key.clone();
            let record_bytes = record_bytes.clone();
            let record = record.clone();
            async move {
                let indexed = trx.get(&snapshot_key, false).await?.ok_or_else(|| {
                    custom_persist_error(PersistError::not_found(format!(
                        "snapshot at height {}",
                        record.height
                    )))
                })?;
                let indexed_record: SnapshotRecord = serde_cbor::from_slice(indexed.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                if indexed_record != record {
                    return Err(custom_persist_error(
                        PersistConflict::SnapshotMismatch {
                            height: record.height,
                        }
                        .into(),
                    ));
                }
                if let Some(active_bytes) = trx.get(&baseline_key, false).await? {
                    let active: SnapshotRecord = serde_cbor::from_slice(active_bytes.as_ref())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if record.height < active.height {
                        return Err(custom_persist_error(PersistError::validation(format!(
                            "baseline cannot regress from {} to {}",
                            active.height, record.height
                        ))));
                    }
                    if record.height == active.height && active != record {
                        return Err(custom_persist_error(
                            PersistConflict::BaselineMismatch {
                                height: record.height,
                            }
                            .into(),
                        ));
                    }
                }
                let mut meta = match trx.get(&meta_key, false).await? {
                    Some(bytes) => {
                        serde_cbor::from_slice::<WorldMeta>(bytes.as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?
                    }
                    None => sample_world_meta(world),
                };
                meta.active_baseline_height = Some(record.height);
                meta.manifest_hash = record.manifest_hash.clone();
                let meta_bytes = serde_cbor::to_vec(&meta)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                trx.set(&baseline_key, &record_bytes);
                trx.set(&meta_key, &meta_bytes);
                trx.set(&catalog_key, &meta_bytes);
                Ok(())
            }
        })?;
        self.run(|trx, _| async move {
            self.refresh_world_ready_state_in_trx(&trx, universe, world, 0)
                .await?;
            Ok(())
        })?;
        Ok(())
    }

    fn snapshot_repair_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        validate_snapshot_record(&record)?;
        let key = self
            .snapshot_by_height_space(universe, world)
            .pack(&(self.to_i64(record.height, "snapshot height")?,));
        let baseline_key = self.baseline_active_key(universe, world);
        let meta_key = self.world_meta_key(universe, world);
        let catalog_key = self.world_catalog_key(universe, world);
        let record_bytes = self.encode(&record)?;
        self.run(|trx, _| {
            let key = key.clone();
            let baseline_key = baseline_key.clone();
            let meta_key = meta_key.clone();
            let catalog_key = catalog_key.clone();
            let record_bytes = record_bytes.clone();
            let record = record.clone();
            async move {
                if let Some(existing) = trx.get(&key, false).await? {
                    let existing_record: SnapshotRecord = serde_cbor::from_slice(existing.as_ref())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if existing_record.height != record.height {
                        return Err(custom_persist_error(
                            PersistConflict::SnapshotMismatch {
                                height: record.height,
                            }
                            .into(),
                        ));
                    }
                }
                trx.set(&key, &record_bytes);
                if let Some(active_bytes) = trx.get(&baseline_key, false).await? {
                    let active: SnapshotRecord = serde_cbor::from_slice(active_bytes.as_ref())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if active.height == record.height {
                        let mut meta = match trx.get(&meta_key, false).await? {
                            Some(bytes) => serde_cbor::from_slice::<WorldMeta>(bytes.as_ref())
                                .map_err(|err| {
                                    custom_persist_error(PersistError::backend(err.to_string()))
                                })?,
                            None => sample_world_meta(world),
                        };
                        meta.active_baseline_height = Some(record.height);
                        meta.manifest_hash = record.manifest_hash.clone();
                        let meta_bytes = serde_cbor::to_vec(&meta).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                        trx.set(&baseline_key, &record_bytes);
                        trx.set(&meta_key, &meta_bytes);
                        trx.set(&catalog_key, &meta_bytes);
                    }
                }
                Ok(())
            }
        })?;
        self.run(|trx, _| async move {
            self.refresh_world_ready_state_in_trx(&trx, universe, world, 0)
                .await?;
            Ok(())
        })?;
        Ok(())
    }

    fn segment_index_put(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SegmentIndexRecord,
    ) -> Result<(), PersistError> {
        let key = self
            .segment_index_space(universe, world)
            .pack(&(self.to_i64(record.segment.end, "segment end height")?,));
        let value = self.encode(&record)?;
        self.run(|trx, _| {
            let key = key.clone();
            let value = value.clone();
            let record = record.clone();
            async move {
                if let Some(existing) = trx.get(&key, false).await? {
                    let existing_record: SegmentIndexRecord =
                        serde_cbor::from_slice(existing.as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if existing_record == record {
                        return Ok(());
                    }
                    return Err(custom_persist_error(
                        PersistConflict::SegmentExists {
                            end_height: record.segment.end,
                        }
                        .into(),
                    ));
                }
                trx.set(&key, &value);
                Ok(())
            }
        })
    }

    fn segment_export(
        &self,
        universe: UniverseId,
        world: WorldId,
        request: SegmentExportRequest,
    ) -> Result<SegmentExportResult, PersistError> {
        validate_segment_export_request(&request)?;
        let baseline = self.snapshot_active_baseline(universe, world)?;
        let safe_exclusive_end = baseline.height.saturating_sub(request.hot_tail_margin);
        if request.segment.end >= safe_exclusive_end {
            return Err(PersistError::validation(format!(
                "segment end {} must be strictly below active baseline {} with hot-tail margin {}",
                request.segment.end, baseline.height, request.hot_tail_margin
            )));
        }
        let head = self.journal_head(universe, world)?;
        if request.segment.end >= head {
            return Err(PersistError::validation(format!(
                "segment end {} must be below current journal head {}",
                request.segment.end, head
            )));
        }

        let expected_entries = request.segment.end - request.segment.start + 1;
        let segment_key = self.segment_index_key(universe, world, request.segment.end)?;
        let existing: Option<SegmentIndexRecord> = self.run(|trx, _| {
            let segment_key = segment_key.clone();
            async move {
                trx.get(&segment_key, false)
                    .await?
                    .map(|value| {
                        serde_cbor::from_slice(value.as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })
                    })
                    .transpose()
            }
        })?;
        let record = if let Some(existing) = existing {
            if existing.segment != request.segment {
                return Err(PersistConflict::SegmentExists {
                    end_height: request.segment.end,
                }
                .into());
            }
            let body_hash = Self::resolve_cas_hash(&existing.body_ref, "segment body_ref")?;
            if !self.cas_has(universe, body_hash)? {
                return Err(PersistCorruption::MissingSegmentBody {
                    segment: request.segment,
                    hash: body_hash,
                }
                .into());
            }
            existing
        } else {
            let entries = self.journal_read_range(
                universe,
                world,
                request.segment.start,
                expected_entries as u32,
            )?;
            let segment_bytes = encode_segment_entries(request.segment, &entries)?;
            let body_hash = self.cas_put_verified(universe, &segment_bytes)?;
            let record = SegmentIndexRecord {
                segment: request.segment,
                body_ref: body_hash.to_hex(),
                checksum: segment_checksum(&segment_bytes),
            };
            self.segment_index_put(universe, world, record.clone())?;
            record
        };

        let journal_space = self.journal_entry_space(universe, world);
        let mut chunk_start = request.segment.start;
        while chunk_start <= request.segment.end {
            let chunk_end =
                (chunk_start + request.delete_chunk_entries as u64 - 1).min(request.segment.end);
            self.run(|trx, _| {
                let journal_space = journal_space.clone();
                async move {
                    for height in chunk_start..=chunk_end {
                        let key = journal_space.pack(&(to_i64_static(height, "journal height")?,));
                        trx.clear(&key);
                    }
                    Ok(())
                }
            })?;
            chunk_start = chunk_end.saturating_add(1);
        }

        let result = SegmentExportResult {
            record,
            exported_entries: expected_entries,
            deleted_entries: expected_entries,
        };
        self.run(|trx, _| async move {
            self.refresh_world_ready_state_in_trx(&trx, universe, world, 0)
                .await?;
            Ok(())
        })?;
        Ok(result)
    }

    fn segment_index_read_from(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_end_inclusive: JournalHeight,
        limit: u32,
    ) -> Result<Vec<SegmentIndexRecord>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let space = self.segment_index_space(universe, world);
        let start_key = space.pack(&(self.to_i64(from_end_inclusive, "segment end height")?,));
        let (_, end_key) = space.range();
        self.run(|trx, _| {
            let start_key = start_key.clone();
            let end_key = end_key.clone();
            async move {
                let mut range = RangeOption::from((start_key, end_key));
                range.mode = foundationdb::options::StreamingMode::WantAll;
                let mut records = Vec::new();
                loop {
                    let remaining = (limit as usize).saturating_sub(records.len());
                    if remaining == 0 {
                        break;
                    }
                    range.limit = Some(remaining);
                    let kvs = trx.get_range(&range, 1, false).await?;
                    let Some(last_key) = kvs.last().map(|kv| kv.key().to_vec()) else {
                        break;
                    };
                    for kv in kvs.iter() {
                        records.push(serde_cbor::from_slice(kv.value().as_ref()).map_err(
                            |err| custom_persist_error(PersistError::backend(err.to_string())),
                        )?);
                    }
                    range.begin = KeySelector::first_greater_than(last_key);
                }
                Ok(records)
            }
        })
    }

    fn segment_read_entries(
        &self,
        universe: UniverseId,
        world: WorldId,
        segment: SegmentId,
    ) -> Result<Vec<(JournalHeight, Vec<u8>)>, PersistError> {
        let segment_key = self.segment_index_key(universe, world, segment.end)?;
        let record: SegmentIndexRecord = self.run(|trx, _| {
            let segment_key = segment_key.clone();
            async move {
                let value = trx.get(&segment_key, false).await?.ok_or_else(|| {
                    custom_persist_error(PersistError::not_found(format!("segment {:?}", segment)))
                })?;
                serde_cbor::from_slice(value.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            }
        })?;
        if record.segment != segment {
            return Err(PersistError::not_found(format!("segment {:?}", segment)));
        }
        let body_hash = Self::resolve_cas_hash(&record.body_ref, "segment body_ref")?;
        let bytes = match self.cas_get(universe, body_hash) {
            Ok(bytes) => bytes,
            Err(PersistError::NotFound(_)) => {
                return Err(PersistCorruption::MissingSegmentBody {
                    segment,
                    hash: body_hash,
                }
                .into());
            }
            Err(err) => return Err(err),
        };
        decode_segment_entries(&record, &bytes)
    }
}
