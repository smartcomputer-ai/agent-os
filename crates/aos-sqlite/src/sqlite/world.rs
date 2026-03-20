use aos_cbor::Hash;
use aos_node::{
    CommandIngress, CommandRecord, InboxItem, InboxSeq, JournalHeight, NodeCatalog,
    PersistConflict, PersistError, SegmentExportRequest, SegmentExportResult, SegmentId,
    SegmentIndexRecord, SnapshotCommitRequest, SnapshotCommitResult, SnapshotRecord, UniverseId,
    WorldAdminLifecycle, WorldId, WorldIngressStore, WorldRuntimeInfo, WorldStore,
    can_upgrade_snapshot_record, encode_segment_entries, segment_checksum,
    validate_baseline_promotion_record, validate_segment_export_request,
    validate_snapshot_commit_request, validate_snapshot_record,
};
use rusqlite::{OptionalExtension, params};

use super::SqliteNodeStore;
use super::util::{
    decode, encode, get_world_row, load_active_baseline, load_latest_snapshot, load_snapshot_at,
    normalize_command_record, normalize_inbox_item, request_hash, seq_from_i64, seq_to_i64,
    world_runtime_info,
};

impl WorldStore for SqliteNodeStore {
    fn cas_put_verified(&self, universe: UniverseId, bytes: &[u8]) -> Result<Hash, PersistError> {
        self.ensure_local_universe(universe)?;
        self.cas.put_verified(bytes)
    }

    fn cas_get(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, PersistError> {
        self.ensure_local_universe(universe)?;
        self.cas.get(hash)
    }

    fn cas_has(&self, universe: UniverseId, hash: Hash) -> Result<bool, PersistError> {
        self.ensure_local_universe(universe)?;
        Ok(self.cas.has(hash))
    }

    fn journal_append_batch(
        &self,
        universe: UniverseId,
        world: WorldId,
        expected_head: JournalHeight,
        entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            if entries.is_empty() {
                return Err(PersistError::validation("journal append batch cannot be empty"));
            }
            let row = get_world_row(tx, universe, world)?;
            if row.journal_head != expected_head {
                return Err(PersistConflict::HeadAdvanced {
                    expected: expected_head,
                    actual: row.journal_head,
                }
                .into());
            }
            let first_height = row.journal_head;
            for (offset, entry) in entries.iter().enumerate() {
                tx.execute(
                    "insert into local_journal_entries (world_id, height, bytes) values (?1, ?2, ?3)",
                    params![world.to_string(), row.journal_head + offset as u64, entry],
                )
                .map_err(|err| PersistError::backend(format!("insert journal entry: {err}")))?;
            }
            tx.execute(
                "update local_worlds set journal_head = ?2 where world_id = ?1",
                params![world.to_string(), row.journal_head + entries.len() as u64],
            )
            .map_err(|err| PersistError::backend(format!("update journal head: {err}")))?;
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
        self.ensure_local_universe(universe)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "select height, bytes from local_journal_entries where world_id = ?1 and height >= ?2 order by height asc limit ?3",
                )
                .map_err(|err| PersistError::backend(format!("prepare journal read: {err}")))?;
            let rows = stmt
                .query_map(
                    params![world.to_string(), from_inclusive, limit],
                    |row| Ok((row.get::<_, u64>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )
                .map_err(|err| PersistError::backend(format!("query journal read: {err}")))?;
            rows.map(|row| row.map_err(|err| PersistError::backend(format!("read journal row: {err}"))))
                .collect()
        })
    }

    fn journal_head(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<JournalHeight, PersistError> {
        self.ensure_local_universe(universe)?;
        self.read(|conn| Ok(get_world_row(conn, universe, world)?.journal_head))
    }

    fn inbox_enqueue(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            let row = get_world_row(tx, universe, world)?;
            if !row.meta.admin.status.accepts_direct_ingress() {
                return Err(PersistConflict::WorldAdminBlocked {
                    world_id: world,
                    status: row.meta.admin.status,
                    action: "enqueue_ingress".into(),
                }
                .into());
            }
            let item = normalize_inbox_item(&self.cas, universe, item)?;
            let seq = row.next_inbox_seq;
            tx.execute(
                "insert into local_inbox_entries (world_id, seq, item) values (?1, ?2, ?3)",
                params![world.to_string(), seq, encode(&item)?],
            )
            .map_err(|err| PersistError::backend(format!("insert inbox entry: {err}")))?;
            tx.execute(
                "update local_worlds set next_inbox_seq = ?2, notify_counter = notify_counter + 1 where world_id = ?1",
                params![world.to_string(), seq + 1],
            )
            .map_err(|err| PersistError::backend(format!("update inbox seq: {err}")))?;
            Ok(InboxSeq::from_u64(seq))
        })
    }

    fn inbox_read_after(
        &self,
        universe: UniverseId,
        world: WorldId,
        after_exclusive: Option<InboxSeq>,
        limit: u32,
    ) -> Result<Vec<(InboxSeq, InboxItem)>, PersistError> {
        self.ensure_local_universe(universe)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.read(|conn| {
            let after = after_exclusive
                .as_ref()
                .map(seq_to_i64)
                .transpose()?
                .unwrap_or(-1);
            let mut stmt = conn
                .prepare(
                    "select seq, item from local_inbox_entries where world_id = ?1 and seq > ?2 order by seq asc limit ?3",
                )
                .map_err(|err| PersistError::backend(format!("prepare inbox read: {err}")))?;
            let rows = stmt
                .query_map(
                    params![world.to_string(), after, limit],
                    |row| {
                        let seq: i64 = row.get(0)?;
                        let bytes: Vec<u8> = row.get(1)?;
                        let item: InboxItem = decode(&bytes).map_err(super::util::to_sql_error)?;
                        Ok((seq_from_i64(seq), item))
                    },
                )
                .map_err(|err| PersistError::backend(format!("query inbox read: {err}")))?;
            rows.map(|row| row.map_err(|err| PersistError::backend(format!("read inbox row: {err}"))))
                .collect()
        })
    }

    fn inbox_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<InboxSeq>, PersistError> {
        self.ensure_local_universe(universe)?;
        self.read(|conn| {
            Ok(get_world_row(conn, universe, world)?
                .inbox_cursor
                .map(InboxSeq::from_u64))
        })
    }

    fn inbox_commit_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
    ) -> Result<(), PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            let row = get_world_row(tx, universe, world)?;
            let current = row.inbox_cursor.map(InboxSeq::from_u64);
            if current != old_cursor {
                return Err(PersistConflict::InboxCursorAdvanced {
                    expected: old_cursor,
                    actual: current,
                }
                .into());
            }
            let new_seq = seq_to_i64(&new_cursor)?;
            tx.execute(
                "delete from local_inbox_entries where world_id = ?1 and seq <= ?2",
                params![world.to_string(), new_seq],
            )
            .map_err(|err| {
                PersistError::backend(format!("delete committed inbox entries: {err}"))
            })?;
            tx.execute(
                "update local_worlds set inbox_cursor = ?2 where world_id = ?1",
                params![world.to_string(), new_seq],
            )
            .map_err(|err| PersistError::backend(format!("update inbox cursor: {err}")))?;
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
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            let row = get_world_row(tx, universe, world)?;
            let current = row.inbox_cursor.map(InboxSeq::from_u64);
            if current != old_cursor {
                return Err(PersistConflict::InboxCursorAdvanced {
                    expected: old_cursor,
                    actual: current,
                }
                .into());
            }
            if row.journal_head != expected_head {
                return Err(PersistConflict::HeadAdvanced {
                    expected: expected_head,
                    actual: row.journal_head,
                }
                .into());
            }
            let first_height = row.journal_head;
            for (offset, entry) in journal_entries.iter().enumerate() {
                tx.execute(
                    "insert into local_journal_entries (world_id, height, bytes) values (?1, ?2, ?3)",
                    params![world.to_string(), row.journal_head + offset as u64, entry],
                )
                .map_err(|err| PersistError::backend(format!("insert drain journal entry: {err}")))?;
            }
            let new_seq = seq_to_i64(&new_cursor)?;
            tx.execute(
                "delete from local_inbox_entries where world_id = ?1 and seq <= ?2",
                params![world.to_string(), new_seq],
            )
            .map_err(|err| PersistError::backend(format!("delete drained inbox entries: {err}")))?;
            tx.execute(
                "update local_worlds set inbox_cursor = ?2, journal_head = ?3 where world_id = ?1",
                params![
                    world.to_string(),
                    new_seq,
                    row.journal_head + journal_entries.len() as u64
                ],
            )
            .map_err(|err| PersistError::backend(format!("update drain state: {err}")))?;
            Ok(first_height)
        })
    }

    fn snapshot_index(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        self.ensure_local_universe(universe)?;
        validate_snapshot_record(&record)?;
        self.write(|tx| {
            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "select record from local_snapshots where world_id = ?1 and height = ?2",
                    params![world.to_string(), record.height],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|err| PersistError::backend(format!("query snapshot index: {err}")))?;
            if let Some(existing) = existing {
                let existing: SnapshotRecord = decode(&existing)?;
                if existing == record {
                    return Ok(());
                }
                if !can_upgrade_snapshot_record(&existing, &record) {
                    return Err(PersistConflict::SnapshotExists {
                        height: record.height,
                    }
                    .into());
                }
                tx.execute(
                    "update local_snapshots set record = ?3 where world_id = ?1 and height = ?2",
                    params![world.to_string(), record.height, encode(&record)?],
                )
                .map_err(|err| PersistError::backend(format!("update snapshot index: {err}")))?;
            } else {
                tx.execute(
                    "insert into local_snapshots (world_id, height, record) values (?1, ?2, ?3)",
                    params![world.to_string(), record.height, encode(&record)?],
                )
                .map_err(|err| PersistError::backend(format!("insert snapshot index: {err}")))?;
            }
            Ok(())
        })
    }

    fn snapshot_commit(
        &self,
        universe: UniverseId,
        world: WorldId,
        request: SnapshotCommitRequest,
    ) -> Result<SnapshotCommitResult, PersistError> {
        self.ensure_local_universe(universe)?;
        validate_snapshot_commit_request(&request)?;
        self.write(|tx| {
            let snapshot_hash = self.cas.put_verified(&request.snapshot_bytes)?;
            if request.record.snapshot_ref != snapshot_hash.to_hex() {
                return Err(PersistError::validation(format!(
                    "snapshot_ref {} must equal CAS hash {}",
                    request.record.snapshot_ref,
                    snapshot_hash.to_hex()
                )));
            }
            let row = get_world_row(tx, universe, world)?;
            if row.journal_head != request.expected_head {
                return Err(PersistConflict::HeadAdvanced {
                    expected: request.expected_head,
                    actual: row.journal_head,
                }
                .into());
            }
            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "select record from local_snapshots where world_id = ?1 and height = ?2",
                    params![world.to_string(), request.record.height],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|err| PersistError::backend(format!("query snapshot commit existing: {err}")))?;
            if let Some(existing) = existing {
                let existing: SnapshotRecord = decode(&existing)?;
                if existing != request.record {
                    return Err(PersistConflict::SnapshotExists { height: request.record.height }.into());
                }
            } else {
                tx.execute(
                    "insert into local_snapshots (world_id, height, record) values (?1, ?2, ?3)",
                    params![world.to_string(), request.record.height, encode(&request.record)?],
                )
                .map_err(|err| PersistError::backend(format!("insert snapshot commit record: {err}")))?;
            }

            let first_height = row.journal_head;
            tx.execute(
                "insert into local_journal_entries (world_id, height, bytes) values (?1, ?2, ?3)",
                params![world.to_string(), row.journal_head, request.snapshot_journal_entry.clone()],
            )
            .map_err(|err| PersistError::backend(format!("insert snapshot journal entry: {err}")))?;
            let mut next_head = row.journal_head + 1;

            if request.promote_baseline {
                validate_baseline_promotion_record(&request.record)?;
                if let Some(active_height) = row.meta.active_baseline_height {
                    if request.record.height < active_height {
                        return Err(PersistError::validation(format!(
                            "baseline cannot regress from {} to {}",
                            active_height, request.record.height
                        )));
                    }
                }
                let baseline_entry = request
                    .baseline_journal_entry
                    .clone()
                    .ok_or_else(|| PersistError::validation("baseline journal entry required when promoting baseline"))?;
                tx.execute(
                    "insert into local_journal_entries (world_id, height, bytes) values (?1, ?2, ?3)",
                    params![world.to_string(), next_head, baseline_entry],
                )
                .map_err(|err| PersistError::backend(format!("insert baseline journal entry: {err}")))?;
                next_head += 1;
                tx.execute(
                    "update local_worlds set journal_head = ?2, active_baseline_height = ?3, manifest_hash = ?4 where world_id = ?1",
                    params![
                        world.to_string(),
                        next_head,
                        request.record.height,
                        request.record.manifest_hash.clone()
                    ],
                )
                .map_err(|err| PersistError::backend(format!("promote baseline during commit: {err}")))?;
            } else {
                tx.execute(
                    "update local_worlds set journal_head = ?2 where world_id = ?1",
                    params![world.to_string(), next_head],
                )
                .map_err(|err| PersistError::backend(format!("advance journal head during snapshot commit: {err}")))?;
            }

            Ok(SnapshotCommitResult {
                snapshot_hash,
                first_height,
                next_head,
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
        self.ensure_local_universe(universe)?;
        self.read(|conn| load_snapshot_at(conn, universe, world, height))
    }

    fn snapshot_latest(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<SnapshotRecord, PersistError> {
        self.ensure_local_universe(universe)?;
        self.read(|conn| load_latest_snapshot(conn, universe, world))
    }

    fn snapshot_active_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<SnapshotRecord, PersistError> {
        self.ensure_local_universe(universe)?;
        self.read(|conn| load_active_baseline(conn, universe, world))
    }

    fn snapshot_promote_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        self.ensure_local_universe(universe)?;
        validate_baseline_promotion_record(&record)?;
        self.write(|tx| {
            let indexed = load_snapshot_at(tx, universe, world, record.height)?;
            if indexed != record {
                return Err(PersistConflict::SnapshotMismatch { height: record.height }.into());
            }
            let row = get_world_row(tx, universe, world)?;
            if let Some(active_height) = row.meta.active_baseline_height {
                if record.height < active_height {
                    return Err(PersistError::validation(format!(
                        "baseline cannot regress from {} to {}",
                        active_height, record.height
                    )));
                }
            }
            tx.execute(
                "update local_worlds set active_baseline_height = ?2, manifest_hash = ?3 where world_id = ?1",
                params![
                    world.to_string(),
                    record.height,
                    record.manifest_hash.clone()
                ],
            )
            .map_err(|err| PersistError::backend(format!("promote baseline: {err}")))?;
            Ok(())
        })
    }

    fn snapshot_repair_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        self.ensure_local_universe(universe)?;
        validate_snapshot_record(&record)?;
        self.write(|tx| {
            tx.execute(
                "insert into local_snapshots (world_id, height, record) values (?1, ?2, ?3)
                 on conflict(world_id, height) do update set record = excluded.record",
                params![world.to_string(), record.height, encode(&record)?],
            )
            .map_err(|err| PersistError::backend(format!("repair snapshot record: {err}")))?;
            Ok(())
        })
    }

    fn segment_index_put(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SegmentIndexRecord,
    ) -> Result<(), PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "select record from local_segments where world_id = ?1 and end_height = ?2",
                    params![world.to_string(), record.segment.end],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|err| PersistError::backend(format!("query segment record: {err}")))?;
            if let Some(existing) = existing {
                let existing: SegmentIndexRecord = decode(&existing)?;
                if existing == record {
                    return Ok(());
                }
                return Err(PersistConflict::SegmentExists {
                    end_height: record.segment.end,
                }
                .into());
            }
            tx.execute(
                "insert into local_segments (world_id, end_height, record) values (?1, ?2, ?3)",
                params![world.to_string(), record.segment.end, encode(&record)?],
            )
            .map_err(|err| PersistError::backend(format!("insert segment index: {err}")))?;
            Ok(())
        })
    }

    fn segment_export(
        &self,
        universe: UniverseId,
        world: WorldId,
        request: SegmentExportRequest,
    ) -> Result<SegmentExportResult, PersistError> {
        self.ensure_local_universe(universe)?;
        validate_segment_export_request(&request)?;
        self.write(|tx| {
            let active = load_active_baseline(tx, universe, world)?;
            let row = get_world_row(tx, universe, world)?;
            let safe_exclusive_end = active.height.saturating_sub(request.hot_tail_margin);
            if request.segment.end >= safe_exclusive_end {
                return Err(PersistError::validation("segment end must stay below active baseline hot-tail safety window"));
            }
            if request.segment.end >= row.journal_head {
                return Err(PersistError::validation("segment end must be below current journal head"));
            }

            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "select record from local_segments where world_id = ?1 and end_height = ?2",
                    params![world.to_string(), request.segment.end],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|err| PersistError::backend(format!("query existing segment export: {err}")))?;
            let record = if let Some(existing) = existing {
                let existing: SegmentIndexRecord = decode(&existing)?;
                if existing.segment != request.segment {
                    return Err(PersistConflict::SegmentExists {
                        end_height: request.segment.end,
                    }
                    .into());
                }
                existing
            } else {
                let mut stmt = tx
                    .prepare(
                        "select height, bytes from local_journal_entries where world_id = ?1 and height between ?2 and ?3 order by height asc",
                    )
                    .map_err(|err| PersistError::backend(format!("prepare segment export read: {err}")))?;
                let rows = stmt
                    .query_map(
                        params![
                            world.to_string(),
                            request.segment.start,
                            request.segment.end
                        ],
                        |row| Ok((row.get::<_, u64>(0)?, row.get::<_, Vec<u8>>(1)?)),
                    )
                    .map_err(|err| PersistError::backend(format!("query segment export rows: {err}")))?;
                let entries: Result<Vec<_>, _> = rows
                    .map(|row| row.map_err(|err| PersistError::backend(format!("read segment export row: {err}"))))
                    .collect();
                let entries = entries?;
                let bytes = encode_segment_entries(request.segment, &entries)?;
                let body_hash = self.cas.put_verified(&bytes)?;
                let record = SegmentIndexRecord {
                    segment: request.segment,
                    body_ref: body_hash.to_hex(),
                    checksum: segment_checksum(&bytes),
                };
                tx.execute(
                    "insert into local_segments (world_id, end_height, record) values (?1, ?2, ?3)",
                    params![world.to_string(), request.segment.end, encode(&record)?],
                )
                .map_err(|err| PersistError::backend(format!("insert segment export record: {err}")))?;
                record
            };

            let deleted_entries = tx
                .execute(
                    "delete from local_journal_entries where world_id = ?1 and height between ?2 and ?3",
                    params![world.to_string(), request.segment.start, request.segment.end],
                )
                .map_err(|err| PersistError::backend(format!("delete exported journal entries: {err}")))? as u64;
            Ok(SegmentExportResult {
                record,
                exported_entries: request.segment.end - request.segment.start + 1,
                deleted_entries,
            })
        })
    }

    fn segment_index_read_from(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_end_inclusive: JournalHeight,
        limit: u32,
    ) -> Result<Vec<SegmentIndexRecord>, PersistError> {
        self.ensure_local_universe(universe)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "select record from local_segments where world_id = ?1 and end_height >= ?2 order by end_height asc limit ?3",
                )
                .map_err(|err| PersistError::backend(format!("prepare segment index read: {err}")))?;
            let rows = stmt
                .query_map(params![world.to_string(), from_end_inclusive, limit], |row| {
                    let bytes: Vec<u8> = row.get(0)?;
                    decode(&bytes).map_err(super::util::to_sql_error)
                })
                .map_err(|err| PersistError::backend(format!("query segment index read: {err}")))?;
            rows.map(|row| row.map_err(|err| PersistError::backend(format!("read segment row: {err}"))))
                .collect()
        })
    }

    fn segment_read_entries(
        &self,
        universe: UniverseId,
        world: WorldId,
        segment: SegmentId,
    ) -> Result<Vec<(JournalHeight, Vec<u8>)>, PersistError> {
        self.ensure_local_universe(universe)?;
        self.read(|conn| {
            let record: SegmentIndexRecord = conn
                .query_row(
                    "select record from local_segments where world_id = ?1 and end_height = ?2",
                    params![world.to_string(), segment.end],
                    |row| {
                        let bytes: Vec<u8> = row.get(0)?;
                        decode(&bytes).map_err(super::util::to_sql_error)
                    },
                )
                .map_err(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => {
                        PersistError::not_found(format!("segment {:?}", segment))
                    }
                    other => PersistError::backend(format!("load segment record: {other}")),
                })?;
            let hash = Hash::from_hex_str(&record.body_ref).map_err(|err| {
                PersistError::backend(format!(
                    "invalid segment body_ref '{}': {err}",
                    record.body_ref
                ))
            })?;
            let bytes = self.cas.get(hash)?;
            aos_node::decode_segment_entries(&record, &bytes)
        })
    }
}

impl NodeCatalog for SqliteNodeStore {
    fn world_runtime_info(
        &self,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) -> Result<WorldRuntimeInfo, PersistError> {
        self.ensure_local_universe(universe)?;
        self.read(|conn| world_runtime_info(conn, universe, world, now_ns))
    }

    fn world_runtime_info_by_handle(
        &self,
        universe: UniverseId,
        handle: &str,
        now_ns: u64,
    ) -> Result<WorldRuntimeInfo, PersistError> {
        self.ensure_local_universe(universe)?;
        let handle = aos_node::normalize_handle(handle)?;
        self.read(|conn| {
            let world_id: String = conn
                .query_row(
                    "select world_id from local_world_handles where handle = ?1",
                    params![handle.clone()],
                    |row| row.get(0),
                )
                .map_err(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => {
                        PersistError::not_found(format!("world handle '{handle}'"))
                    }
                    other => PersistError::backend(format!("load world by handle: {other}")),
                })?;
            world_runtime_info(
                conn,
                universe,
                world_id
                    .parse()
                    .map_err(|_| PersistError::backend("invalid world id"))?,
                now_ns,
            )
        })
    }

    fn list_worlds(
        &self,
        universe: UniverseId,
        now_ns: u64,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, PersistError> {
        self.ensure_local_universe(universe)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.read(|conn| {
            let sql = if after.is_some() {
                "select world_id from local_worlds where world_id > ?1 order by world_id asc limit ?2"
            } else {
                "select world_id from local_worlds order by world_id asc limit ?1"
            };
            let mut stmt = conn
                .prepare(sql)
                .map_err(|err| PersistError::backend(format!("prepare list worlds: {err}")))?;
            let mut infos = Vec::new();
            if let Some(after) = after {
                let rows = stmt
                    .query_map(params![after.to_string(), limit], |row| row.get::<_, String>(0))
                    .map_err(|err| PersistError::backend(format!("query list worlds: {err}")))?;
                for row in rows {
                    let world_id =
                        row.map_err(|err| PersistError::backend(format!("read world id row: {err}")))?;
                    infos.push(world_runtime_info(
                        conn,
                        universe,
                        world_id.parse().map_err(|_| PersistError::backend("invalid world id"))?,
                        now_ns,
                    )?);
                }
            } else {
                let rows = stmt
                    .query_map(params![limit], |row| row.get::<_, String>(0))
                    .map_err(|err| PersistError::backend(format!("query list worlds: {err}")))?;
                for row in rows {
                    let world_id =
                        row.map_err(|err| PersistError::backend(format!("read world id row: {err}")))?;
                    infos.push(world_runtime_info(
                        conn,
                        universe,
                        world_id.parse().map_err(|_| PersistError::backend("invalid world id"))?,
                        now_ns,
                    )?);
                }
            }
            Ok(infos)
        })
    }

    fn set_world_placement_pin(
        &self,
        universe: UniverseId,
        world: WorldId,
        placement_pin: Option<String>,
    ) -> Result<(), PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            let row = get_world_row(tx, universe, world)?;
            if row.meta.admin.status.blocks_world_operations() {
                return Err(PersistConflict::WorldAdminBlocked {
                    world_id: world,
                    status: row.meta.admin.status,
                    action: "set_world_placement_pin".into(),
                }
                .into());
            }
            tx.execute(
                "update local_worlds set placement_pin = ?2 where world_id = ?1",
                params![world.to_string(), placement_pin],
            )
            .map_err(|err| PersistError::backend(format!("update world placement pin: {err}")))?;
            Ok(())
        })
    }

    fn set_world_admin_lifecycle(
        &self,
        universe: UniverseId,
        world: WorldId,
        admin: WorldAdminLifecycle,
    ) -> Result<(), PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            let previous = get_world_row(tx, universe, world)?;
            if !matches!(
                previous.meta.admin.status,
                aos_node::WorldAdminStatus::Deleted
            ) && matches!(admin.status, aos_node::WorldAdminStatus::Deleted)
            {
                tx.execute(
                    "delete from local_world_handles where world_id = ?1",
                    params![world.to_string()],
                )
                .map_err(|err| PersistError::backend(format!("delete world handle: {err}")))?;
            }
            tx.execute(
                "update local_worlds set admin = ?2 where world_id = ?1",
                params![world.to_string(), encode(&admin)?],
            )
            .map_err(|err| PersistError::backend(format!("update world admin lifecycle: {err}")))?;
            Ok(())
        })
    }
}

impl WorldIngressStore for SqliteNodeStore {
    fn enqueue_ingress(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            let row = get_world_row(tx, universe, world)?;
            let item = normalize_inbox_item(&self.cas, universe, item)?;
            let seq = row.next_inbox_seq;
            tx.execute(
                "insert into local_inbox_entries (world_id, seq, item) values (?1, ?2, ?3)",
                params![world.to_string(), seq, encode(&item)?],
            )
            .map_err(|err| PersistError::backend(format!("enqueue ingress: {err}")))?;
            tx.execute(
                "update local_worlds set next_inbox_seq = ?2, notify_counter = notify_counter + 1 where world_id = ?1",
                params![world.to_string(), seq + 1],
            )
            .map_err(|err| PersistError::backend(format!("advance ingress seq: {err}")))?;
            Ok(InboxSeq::from_u64(seq))
        })
    }
}

impl aos_node::CommandStore for SqliteNodeStore {
    fn command_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        command_id: &str,
    ) -> Result<Option<CommandRecord>, PersistError> {
        self.ensure_local_universe(universe)?;
        self.read(|conn| {
            conn.query_row(
                "select record from local_command_records where world_id = ?1 and command_id = ?2",
                params![world.to_string(), command_id],
                |row| {
                    let bytes: Vec<u8> = row.get(0)?;
                    decode(&bytes).map_err(super::util::to_sql_error)
                },
            )
            .optional()
            .map_err(|err| PersistError::backend(format!("load command record: {err}")))
        })
    }

    fn submit_command(
        &self,
        universe: UniverseId,
        world: WorldId,
        ingress: CommandIngress,
        initial_record: CommandRecord,
    ) -> Result<CommandRecord, PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            let row = get_world_row(tx, universe, world)?;
            if !row.meta.admin.status.accepts_command_ingress() {
                return Err(PersistConflict::WorldAdminBlocked {
                    world_id: world,
                    status: row.meta.admin.status,
                    action: "submit_command".into(),
                }
                .into());
            }
            let request_hash = request_hash(&ingress)?;
            let existing: Option<(String, Vec<u8>)> = tx
                .query_row(
                    "select request_hash, record from local_command_records where world_id = ?1 and command_id = ?2",
                    params![world.to_string(), ingress.command_id.clone()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|err| PersistError::backend(format!("query existing command: {err}")))?;
            if let Some((existing_hash, record_bytes)) = existing {
                if existing_hash != request_hash {
                    return Err(PersistConflict::CommandRequestMismatch {
                        command_id: ingress.command_id,
                    }
                    .into());
                }
                return decode(&record_bytes);
            }
            let normalized_record = normalize_command_record(&self.cas, universe, initial_record)?;
            let item =
                normalize_inbox_item(&self.cas, universe, InboxItem::Control(ingress.clone()))?;
            let seq = row.next_inbox_seq;
            tx.execute(
                "insert into local_inbox_entries (world_id, seq, item) values (?1, ?2, ?3)",
                params![world.to_string(), seq, encode(&item)?],
            )
            .map_err(|err| PersistError::backend(format!("insert command inbox entry: {err}")))?;
            tx.execute(
                "insert into local_command_records (world_id, command_id, request_hash, record) values (?1, ?2, ?3, ?4)",
                params![
                    world.to_string(),
                    ingress.command_id.clone(),
                    request_hash,
                    encode(&normalized_record)?
                ],
            )
            .map_err(|err| PersistError::backend(format!("insert command record: {err}")))?;
            tx.execute(
                "update local_worlds set next_inbox_seq = ?2, notify_counter = notify_counter + 1 where world_id = ?1",
                params![world.to_string(), seq + 1],
            )
            .map_err(|err| PersistError::backend(format!("advance command inbox seq: {err}")))?;
            Ok(normalized_record)
        })
    }

    fn update_command_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: CommandRecord,
    ) -> Result<(), PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            let existing_hash: String = tx
                .query_row(
                    "select request_hash from local_command_records where world_id = ?1 and command_id = ?2",
                    params![world.to_string(), record.command_id.clone()],
                    |row| row.get(0),
                )
                .map_err(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => {
                        PersistError::not_found(format!("command {}", record.command_id))
                    }
                    other => PersistError::backend(format!("load command record hash: {other}")),
                })?;
            let normalized = normalize_command_record(&self.cas, universe, record)?;
            tx.execute(
                "update local_command_records set record = ?3 where world_id = ?1 and command_id = ?2 and request_hash = ?4",
                params![
                    world.to_string(),
                    normalized.command_id.clone(),
                    encode(&normalized)?,
                    existing_hash
                ],
            )
            .map_err(|err| PersistError::backend(format!("update command record: {err}")))?;
            Ok(())
        })
    }
}
