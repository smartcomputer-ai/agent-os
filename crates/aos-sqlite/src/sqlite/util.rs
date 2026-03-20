use aos_cbor::Hash;
use aos_node::{
    CborPayload, CommandIngress, CommandRecord, InboxItem, InboxSeq, PersistConflict, PersistError,
    PersistenceConfig, SnapshotRecord, UniverseAdminStatus, UniverseId, UniverseMeta,
    UniverseRecord, WorldAdminLifecycle, WorldId, WorldLineage, WorldMeta, WorldRuntimeInfo,
    maintenance_due, validate_world_seed,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::fs_cas::FsCas;

#[derive(Debug, Clone)]
pub(super) struct WorldRow {
    pub world_id: WorldId,
    pub meta: WorldMeta,
    pub journal_head: u64,
    pub inbox_cursor: Option<u64>,
    pub next_inbox_seq: u64,
    pub notify_counter: u64,
    pub pending_effects_count: u64,
    pub next_timer_due_at_ns: Option<u64>,
}

pub(super) fn default_persistence_config() -> PersistenceConfig {
    PersistenceConfig::default()
}

pub(super) fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, PersistError> {
    serde_cbor::to_vec(value).map_err(|err| PersistError::backend(err.to_string()))
}

pub(super) fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, PersistError> {
    serde_cbor::from_slice(bytes).map_err(|err| PersistError::backend(err.to_string()))
}

pub(super) fn seq_to_i64(seq: &InboxSeq) -> Result<i64, PersistError> {
    let bytes = seq.as_bytes();
    if bytes.len() != 8 {
        return Err(PersistError::validation(format!(
            "local sqlite backend requires 8-byte inbox sequences, got {} bytes",
            bytes.len()
        )));
    }
    Ok(u64::from_be_bytes(bytes.try_into().unwrap()) as i64)
}

pub(super) fn seq_from_i64(value: i64) -> InboxSeq {
    InboxSeq::from_u64(value as u64)
}

pub(super) fn parse_universe_id(value: &str) -> Result<UniverseId, PersistError> {
    value
        .parse()
        .map_err(|_| PersistError::backend(format!("invalid persisted universe id '{value}'")))
}

pub(super) fn parse_world_id(value: &str) -> Result<WorldId, PersistError> {
    value
        .parse()
        .map_err(|_| PersistError::backend(format!("invalid persisted world id '{value}'")))
}

pub(super) fn request_hash(ingress: &CommandIngress) -> Result<String, PersistError> {
    Hash::of_cbor(&(
        ingress.command.as_str(),
        ingress.actor.as_deref(),
        &ingress.payload,
    ))
    .map(|hash| hash.to_hex())
    .map_err(|err| PersistError::backend(err.to_string()))
}

pub(super) fn normalize_payload(
    cas: &FsCas,
    universe: UniverseId,
    payload: &mut CborPayload,
) -> Result<(), PersistError> {
    payload.validate()?;
    if let Some(bytes) = payload.inline_cbor.take() {
        if bytes.len()
            > default_persistence_config()
                .inbox
                .inline_payload_threshold_bytes
        {
            let _ = universe;
            let hash = cas.put_verified(&bytes)?;
            *payload = CborPayload::externalized(hash, bytes.len() as u64);
        } else {
            payload.inline_cbor = Some(bytes);
        }
    }
    Ok(())
}

pub(super) fn normalize_inbox_item(
    cas: &FsCas,
    universe: UniverseId,
    mut item: InboxItem,
) -> Result<InboxItem, PersistError> {
    match &mut item {
        InboxItem::DomainEvent(event) => normalize_payload(cas, universe, &mut event.value)?,
        InboxItem::Receipt(receipt) => normalize_payload(cas, universe, &mut receipt.payload)?,
        InboxItem::Inbox(inbox) => normalize_payload(cas, universe, &mut inbox.payload)?,
        InboxItem::TimerFired(timer) => normalize_payload(cas, universe, &mut timer.payload)?,
        InboxItem::Control(control) => normalize_payload(cas, universe, &mut control.payload)?,
    }
    Ok(item)
}

pub(super) fn normalize_command_record(
    cas: &FsCas,
    universe: UniverseId,
    mut record: CommandRecord,
) -> Result<CommandRecord, PersistError> {
    if let Some(payload) = record.result_payload.as_mut() {
        normalize_payload(cas, universe, payload)?;
    }
    Ok(record)
}

pub(super) fn ensure_universe_handle_available(
    _conn: &Connection,
    _universe: UniverseId,
    _handle: &str,
) -> Result<(), PersistError> {
    Ok(())
}

pub(super) fn ensure_world_handle_available(
    conn: &Connection,
    universe: UniverseId,
    world: WorldId,
    handle: &str,
) -> Result<(), PersistError> {
    let existing: Option<String> = conn
        .query_row(
            "select world_id from local_world_handles where handle = ?1",
            params![handle],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| PersistError::backend(format!("query world handle availability: {err}")))?;
    if let Some(existing) = existing {
        let existing = parse_world_id(&existing)?;
        if existing != world {
            return Err(PersistConflict::WorldHandleExists {
                universe_id: universe,
                handle: handle.to_string(),
                world_id: existing,
            }
            .into());
        }
    }
    Ok(())
}

pub(super) fn ensure_universe_for_world(
    tx: &Transaction<'_>,
    universe: UniverseId,
) -> Result<UniverseRecord, PersistError> {
    let record = get_universe_row(tx, universe)?
        .ok_or_else(|| PersistError::not_found(format!("universe {universe}")))?;
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

pub(super) fn get_universe_row(
    conn: &Connection,
    universe: UniverseId,
) -> Result<Option<UniverseRecord>, PersistError> {
    conn.query_row(
        "select universe_id, universe_handle, created_at_ns, admin from local_meta where singleton = 1",
        [],
        |row| decode_universe_row(row, universe),
    )
    .optional()
    .map_err(|err| PersistError::backend(format!("load universe row: {err}")))
}

pub(super) fn decode_universe_row(
    row: &Row<'_>,
    universe_id: UniverseId,
) -> rusqlite::Result<UniverseRecord> {
    let stored_universe_id: String = row.get(0)?;
    let handle: String = row.get(1)?;
    let created_at_ns: u64 = row.get(2)?;
    let admin_bytes: Vec<u8> = row.get(3)?;
    let stored_universe_id = parse_universe_id(&stored_universe_id).map_err(to_sql_error)?;
    if stored_universe_id != universe_id {
        return Err(to_sql_error(PersistError::not_found(format!(
            "universe {universe_id}"
        ))));
    }
    let admin = decode(&admin_bytes).map_err(to_sql_error)?;
    Ok(UniverseRecord {
        universe_id,
        created_at_ns,
        meta: UniverseMeta { handle },
        admin,
    })
}

pub(super) fn decode_world_row(row: &Row<'_>) -> rusqlite::Result<WorldRow> {
    let world_id: String = row.get(0)?;
    let handle: String = row.get(1)?;
    let manifest_hash: Option<String> = row.get(2)?;
    let active_baseline_height: Option<u64> = row.get(3)?;
    let placement_pin: Option<String> = row.get(4)?;
    let created_at_ns: u64 = row.get(5)?;
    let lineage_bytes: Option<Vec<u8>> = row.get(6)?;
    let admin_bytes: Vec<u8> = row.get(7)?;
    let journal_head: u64 = row.get(8)?;
    let inbox_cursor: Option<i64> = row.get(9)?;
    let next_inbox_seq: u64 = row.get(10)?;
    let notify_counter: u64 = row.get(11)?;
    let pending_effects_count: u64 = row.get(12)?;
    let next_timer_due_at_ns: Option<u64> = row.get(13)?;
    Ok(WorldRow {
        world_id: parse_world_id(&world_id).map_err(to_sql_error)?,
        meta: WorldMeta {
            handle,
            manifest_hash,
            active_baseline_height,
            placement_pin,
            created_at_ns,
            lineage: lineage_bytes
                .as_deref()
                .map(decode)
                .transpose()
                .map_err(to_sql_error)?,
            admin: decode(&admin_bytes).map_err(to_sql_error)?,
        },
        journal_head,
        inbox_cursor: inbox_cursor.map(|value| value as u64),
        next_inbox_seq,
        notify_counter,
        pending_effects_count,
        next_timer_due_at_ns,
    })
}

pub(super) fn get_world_row(
    conn: &Connection,
    _universe: UniverseId,
    world: WorldId,
) -> Result<WorldRow, PersistError> {
    conn.query_row(
        "select world_id, handle, manifest_hash, active_baseline_height, placement_pin, created_at_ns, lineage, admin, journal_head, inbox_cursor, next_inbox_seq, notify_counter, pending_effects_count, next_timer_due_at_ns
         from local_worlds where world_id = ?1",
        params![world.to_string()],
        decode_world_row,
    )
    .map_err(|err| match err {
        rusqlite::Error::QueryReturnedNoRows => {
            PersistError::not_found(format!("world {world}"))
        }
        other => PersistError::backend(format!("load world row: {other}")),
    })
}

pub(super) fn world_runtime_info(
    conn: &Connection,
    universe: UniverseId,
    world: WorldId,
    _now_ns: u64,
) -> Result<WorldRuntimeInfo, PersistError> {
    let row = get_world_row(conn, universe, world)?;
    let pending_inbox = conn
        .query_row(
            "select exists(select 1 from local_inbox_entries where world_id = ?1 limit 1)",
            params![world.to_string()],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|err| PersistError::backend(format!("query pending inbox: {err}")))?
        != 0;
    let first_hot_journal: Option<u64> = conn
        .query_row(
            "select min(height) from local_journal_entries where world_id = ?1",
            params![world.to_string()],
            |r| r.get(0),
        )
        .map_err(|err| PersistError::backend(format!("query first hot journal entry: {err}")))?;
    let has_pending_maintenance = row.meta.admin.status.requires_maintenance_wakeup()
        || maintenance_due(
            row.journal_head,
            row.meta.active_baseline_height,
            first_hot_journal,
            default_persistence_config().snapshot_maintenance,
        );
    Ok(WorldRuntimeInfo {
        world_id: row.world_id,
        meta: row.meta,
        notify_counter: row.notify_counter,
        has_pending_inbox: pending_inbox,
        has_pending_effects: row.pending_effects_count > 0,
        next_timer_due_at_ns: row.next_timer_due_at_ns,
        has_pending_maintenance,
        lease: None,
    })
}

pub(super) fn ensure_seed_cas_roots_exist(
    cas: &FsCas,
    universe: UniverseId,
    seed: &aos_node::WorldSeed,
) -> Result<(), PersistError> {
    validate_world_seed(seed)?;
    let snapshot_hash = Hash::from_hex_str(&seed.baseline.snapshot_ref)
        .map_err(|err| PersistError::validation(format!("invalid snapshot_ref: {err}")))?;
    let manifest_ref = seed
        .baseline
        .manifest_hash
        .as_ref()
        .ok_or_else(|| PersistError::validation("seed baseline requires manifest_hash"))?;
    let manifest_hash = Hash::from_hex_str(manifest_ref)
        .map_err(|err| PersistError::validation(format!("invalid manifest_hash: {err}")))?;
    if !cas.has(snapshot_hash) {
        return Err(PersistError::not_found(format!(
            "snapshot {} in universe {}",
            seed.baseline.snapshot_ref, universe
        )));
    }
    if !cas.has(manifest_hash) {
        return Err(PersistError::not_found(format!(
            "manifest {} in universe {}",
            manifest_ref, universe
        )));
    }
    Ok(())
}

pub(super) fn insert_world_from_seed(
    tx: &Transaction<'_>,
    cas: &FsCas,
    universe: UniverseId,
    world: WorldId,
    seed: &aos_node::WorldSeed,
    handle: String,
    placement_pin: Option<String>,
    created_at_ns: u64,
    lineage: WorldLineage,
) -> Result<aos_node::WorldRecord, PersistError> {
    ensure_seed_cas_roots_exist(cas, universe, seed)?;
    let existing: Option<i64> = tx
        .query_row(
            "select 1 from local_worlds where world_id = ?1",
            params![world.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| PersistError::backend(format!("query world exists: {err}")))?;
    if existing.is_some() {
        return Err(PersistConflict::WorldExists { world_id: world }.into());
    }

    let snapshot_hash = Hash::from_hex_str(&seed.baseline.snapshot_ref)
        .map_err(|err| PersistError::validation(format!("invalid snapshot_ref: {err}")))?;
    let snapshot_bytes = cas.get(snapshot_hash)?;
    for (state_hash, state_bytes) in aos_node::state_blobs_from_snapshot(&snapshot_bytes)? {
        let stored = cas.put_verified(&state_bytes)?;
        if stored != state_hash {
            return Err(PersistError::backend(format!(
                "snapshot state hash mismatch: expected {}, stored {}",
                state_hash.to_hex(),
                stored.to_hex()
            )));
        }
    }

    tx.execute(
        "insert into local_worlds (
            world_id, handle, manifest_hash, active_baseline_height, placement_pin,
            created_at_ns, lineage, admin, journal_head, inbox_cursor, next_inbox_seq,
            notify_counter, pending_effects_count, next_timer_due_at_ns
        ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, null, 0, 0, 0, null)",
        params![
            world.to_string(),
            handle.clone(),
            seed.baseline.manifest_hash.clone(),
            seed.baseline.height,
            placement_pin,
            created_at_ns,
            Some(encode(&lineage)?),
            encode(&WorldAdminLifecycle::default())?,
            seed.baseline.height.saturating_add(1),
        ],
    )
    .map_err(map_sql_conflict)?;
    tx.execute(
        "insert into local_world_handles (handle, world_id) values (?1, ?2)",
        params![handle.clone(), world.to_string()],
    )
    .map_err(map_sql_conflict)?;
    tx.execute(
        "insert into local_snapshots (world_id, height, record) values (?1, ?2, ?3)",
        params![
            world.to_string(),
            seed.baseline.height,
            encode(&seed.baseline)?
        ],
    )
    .map_err(|err| PersistError::backend(format!("insert seed snapshot: {err}")))?;
    Ok(aos_node::WorldRecord {
        world_id: world,
        meta: WorldMeta {
            handle,
            manifest_hash: seed.baseline.manifest_hash.clone(),
            active_baseline_height: Some(seed.baseline.height),
            placement_pin,
            created_at_ns,
            lineage: Some(lineage),
            admin: WorldAdminLifecycle::default(),
        },
        active_baseline: seed.baseline.clone(),
        journal_head: seed.baseline.height.saturating_add(1),
    })
}

pub(super) fn resolve_snapshot_selector(
    conn: &Connection,
    universe: UniverseId,
    world: WorldId,
    selector: &aos_node::SnapshotSelector,
) -> Result<SnapshotRecord, PersistError> {
    match selector {
        aos_node::SnapshotSelector::ActiveBaseline => {
            let row = get_world_row(conn, universe, world)?;
            let Some(height) = row.meta.active_baseline_height else {
                return Err(PersistError::not_found("active baseline"));
            };
            load_snapshot_at(conn, universe, world, height)
        }
        aos_node::SnapshotSelector::ByHeight { height } => {
            load_snapshot_at(conn, universe, world, *height)
        }
        aos_node::SnapshotSelector::ByRef { snapshot_ref } => {
            let mut stmt = conn
                .prepare(
                    "select record from local_snapshots where world_id = ?1 order by height asc",
                )
                .map_err(|err| PersistError::backend(format!("prepare snapshots by ref: {err}")))?;
            let rows = stmt
                .query_map(params![world.to_string()], |row| {
                    let bytes: Vec<u8> = row.get(0)?;
                    decode(&bytes).map_err(to_sql_error)
                })
                .map_err(|err| PersistError::backend(format!("query snapshots by ref: {err}")))?;
            for row in rows {
                let record: SnapshotRecord =
                    row.map_err(|err| PersistError::backend(format!("read snapshot row: {err}")))?;
                if record.snapshot_ref == *snapshot_ref {
                    return Ok(record);
                }
            }
            Err(PersistError::not_found(format!("snapshot {snapshot_ref}")))
        }
    }
}

pub(super) fn load_snapshot_at(
    conn: &Connection,
    _universe: UniverseId,
    world: WorldId,
    height: u64,
) -> Result<SnapshotRecord, PersistError> {
    conn.query_row(
        "select record from local_snapshots where world_id = ?1 and height = ?2",
        params![world.to_string(), height],
        |row| {
            let bytes: Vec<u8> = row.get(0)?;
            decode(&bytes).map_err(to_sql_error)
        },
    )
    .map_err(|err| match err {
        rusqlite::Error::QueryReturnedNoRows => {
            PersistError::not_found(format!("snapshot at height {height}"))
        }
        other => PersistError::backend(format!("load snapshot: {other}")),
    })
}

pub(super) fn load_latest_snapshot(
    conn: &Connection,
    _universe: UniverseId,
    world: WorldId,
) -> Result<SnapshotRecord, PersistError> {
    conn.query_row(
        "select record from local_snapshots where world_id = ?1 order by height desc limit 1",
        params![world.to_string()],
        |row| {
            let bytes: Vec<u8> = row.get(0)?;
            decode(&bytes).map_err(to_sql_error)
        },
    )
    .map_err(|err| match err {
        rusqlite::Error::QueryReturnedNoRows => PersistError::not_found("latest snapshot"),
        other => PersistError::backend(format!("load latest snapshot: {other}")),
    })
}

pub(super) fn load_active_baseline(
    conn: &Connection,
    universe: UniverseId,
    world: WorldId,
) -> Result<SnapshotRecord, PersistError> {
    let row = get_world_row(conn, universe, world)?;
    let Some(height) = row.meta.active_baseline_height else {
        return Err(PersistError::not_found("active baseline"));
    };
    load_snapshot_at(conn, universe, world, height)
}

pub(super) fn map_sql_conflict(err: rusqlite::Error) -> PersistError {
    PersistError::backend(format!("sqlite write: {err}"))
}

pub(super) fn to_sql_error(err: PersistError) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(err.to_string())))
}
