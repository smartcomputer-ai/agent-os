use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::{CommandRecord, SnapshotRecord, UniverseId, WorldId, WorldLogFrame};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{LocalStatePaths, LocalStoreError};

const RUNTIME_SCHEMA_VERSION: i64 = 4;

#[derive(Debug, Clone)]
pub struct LocalSqliteConfig {
    pub db_path: PathBuf,
}

impl LocalSqliteConfig {
    pub fn from_paths(paths: &LocalStatePaths) -> Self {
        Self {
            db_path: paths.runtime_db(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorldDirectoryRow {
    pub world_id: WorldId,
    pub universe_id: UniverseId,
    pub created_at_ns: u64,
    pub initial_manifest_hash: String,
    pub world_epoch: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CheckpointHeadRow {
    pub world_id: WorldId,
    pub active_baseline: SnapshotRecord,
    pub next_world_seq: u64,
    pub checkpointed_at_ns: u64,
}

pub struct LocalSqliteBackend {
    config: LocalSqliteConfig,
    conn: Connection,
}

impl LocalSqliteBackend {
    pub fn from_paths(paths: &LocalStatePaths) -> Result<Self, LocalStoreError> {
        Self::new(LocalSqliteConfig::from_paths(paths))
    }

    pub fn new(config: LocalSqliteConfig) -> Result<Self, LocalStoreError> {
        if let Some(parent) = config.db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&config.db_path)?;
        initialize_schema(&conn)?;
        Ok(Self { config, conn })
    }

    pub fn config(&self) -> &LocalSqliteConfig {
        &self.config
    }

    pub fn load_runtime_meta(&self) -> Result<(u64, u64), LocalStoreError> {
        Ok(self.conn.query_row(
            "select next_submission_seq, next_frame_offset
             from runtime_meta where singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    pub fn persist_runtime_counters(
        &self,
        next_submission_seq: u64,
        next_frame_offset: u64,
    ) -> Result<(), LocalStoreError> {
        self.conn.execute(
            "update runtime_meta
             set next_submission_seq = ?2, next_frame_offset = ?3
             where singleton = 1",
            params![1_i64, next_submission_seq, next_frame_offset],
        )?;
        Ok(())
    }

    pub fn load_world_directory(
        &self,
    ) -> Result<Vec<(WorldDirectoryRow, CheckpointHeadRow)>, LocalStoreError> {
        let mut stmt = self.conn.prepare(
            "select d.world_id, d.universe_id, d.created_at_ns, d.initial_manifest_hash, d.world_epoch, h.active_baseline, h.next_world_seq, h.checkpointed_at_ns
             from world_directory d
             join checkpoint_heads h on h.world_id = d.world_id
             order by d.world_id asc",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                WorldDirectoryRow {
                    world_id: row
                        .get::<_, String>(0)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    universe_id: row
                        .get::<_, String>(1)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    created_at_ns: row.get(2)?,
                    initial_manifest_hash: row.get(3)?,
                    world_epoch: row.get(4)?,
                },
                CheckpointHeadRow {
                    world_id: row
                        .get::<_, String>(0)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    active_baseline: serde_cbor::from_slice(&row.get::<_, Vec<u8>>(5)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    next_world_seq: row.get(6)?,
                    checkpointed_at_ns: row.get(7)?,
                },
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn load_frame_log_for_world(
        &self,
        world_id: WorldId,
    ) -> Result<Vec<WorldLogFrame>, LocalStoreError> {
        let mut stmt = self.conn.prepare(
            "select frame from journal_frames where world_id = ?1 order by world_seq_start asc",
        )?;
        let rows = stmt.query_map(params![world_id.to_string()], |row| {
            row.get::<_, Vec<u8>>(0)
        })?;
        rows.map(|row| Ok(serde_cbor::from_slice::<WorldLogFrame>(&row?)?))
            .collect()
    }

    pub fn append_journal_frame(
        &self,
        offset: u64,
        world_id: WorldId,
        frame: &WorldLogFrame,
    ) -> Result<(), LocalStoreError> {
        self.conn.execute(
            "insert into journal_frames (
                offset, world_id, world_seq_start, frame
            ) values (?1, ?2, ?3, ?4)",
            params![
                offset,
                world_id.to_string(),
                frame.world_seq_start,
                serde_cbor::to_vec(frame)?,
            ],
        )?;
        Ok(())
    }

    pub fn load_command_projection(
        &self,
        world_id: WorldId,
    ) -> Result<BTreeMap<String, CommandRecord>, LocalStoreError> {
        let mut stmt = self.conn.prepare(
            "select command_id, record from command_projection where world_id = ?1 order by command_id asc",
        )?;
        let rows = stmt.query_map(params![world_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                serde_cbor::from_slice::<CommandRecord>(&row.get::<_, Vec<u8>>(1)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
            ))
        })?;
        Ok(rows.collect::<Result<BTreeMap<_, _>, _>>()?)
    }

    pub fn persist_command_projection(
        &self,
        world_id: WorldId,
        record: &CommandRecord,
    ) -> Result<(), LocalStoreError> {
        self.conn.execute(
            "insert into command_projection (world_id, command_id, record) values (?1, ?2, ?3)
             on conflict(world_id, command_id) do update set record = excluded.record",
            params![
                world_id.to_string(),
                record.command_id,
                serde_cbor::to_vec(record)?,
            ],
        )?;
        Ok(())
    }

    pub fn persist_world_directory(
        &self,
        world_id: WorldId,
        universe_id: UniverseId,
        created_at_ns: u64,
        initial_manifest_hash: &str,
        world_epoch: u64,
    ) -> Result<(), LocalStoreError> {
        self.conn.execute(
            "insert into world_directory (
                world_id, universe_id, created_at_ns, initial_manifest_hash, world_epoch
            ) values (?1, ?2, ?3, ?4, ?5)
            on conflict(world_id) do update set
                universe_id = excluded.universe_id,
                created_at_ns = excluded.created_at_ns,
                initial_manifest_hash = excluded.initial_manifest_hash,
                world_epoch = excluded.world_epoch",
            params![
                world_id.to_string(),
                universe_id.to_string(),
                created_at_ns,
                initial_manifest_hash,
                world_epoch,
            ],
        )?;
        Ok(())
    }

    pub fn persist_checkpoint_head(
        &self,
        world_id: WorldId,
        active_baseline: &SnapshotRecord,
        next_world_seq: u64,
        checkpointed_at_ns: u64,
    ) -> Result<(), LocalStoreError> {
        self.conn.execute(
            "insert into checkpoint_heads (
                world_id, active_baseline, next_world_seq, checkpointed_at_ns
            ) values (?1, ?2, ?3, ?4)
            on conflict(world_id) do update set
                active_baseline = excluded.active_baseline,
                next_world_seq = excluded.next_world_seq,
                checkpointed_at_ns = excluded.checkpointed_at_ns",
            params![
                world_id.to_string(),
                serde_cbor::to_vec(active_baseline)?,
                next_world_seq,
                checkpointed_at_ns,
            ],
        )?;
        Ok(())
    }
}

fn initialize_schema(conn: &Connection) -> Result<(), LocalStoreError> {
    conn.execute_batch(
        "
        create table if not exists runtime_meta (
            singleton integer primary key check (singleton = 1),
            schema_version integer not null,
            next_submission_seq integer not null,
            next_frame_offset integer not null
        );
        create table if not exists world_directory (
            world_id text primary key,
            universe_id text not null,
            created_at_ns integer not null,
            initial_manifest_hash text not null,
            world_epoch integer not null
        );
        create table if not exists checkpoint_heads (
            world_id text primary key,
            active_baseline blob not null,
            next_world_seq integer not null,
            checkpointed_at_ns integer not null default 0
        );
        create table if not exists journal_frames (
            offset integer primary key,
            world_id text not null,
            world_seq_start integer not null,
            frame blob not null
        );
        create table if not exists command_projection (
            world_id text not null,
            command_id text not null,
            record blob not null,
            primary key (world_id, command_id)
        );
        ",
    )?;
    let existing: Option<i64> = conn
        .query_row(
            "select schema_version from runtime_meta where singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match existing {
        Some(existing) if existing == RUNTIME_SCHEMA_VERSION => Ok(()),
        Some(3) => {
            conn.execute(
                "alter table checkpoint_heads add column checkpointed_at_ns integer not null default 0",
                [],
            )?;
            conn.execute(
                "update runtime_meta set schema_version = ?1 where singleton = 1",
                params![RUNTIME_SCHEMA_VERSION],
            )?;
            Ok(())
        }
        Some(existing) => Err(LocalStoreError::Backend(format!(
            "local runtime schema version {existing} does not match expected {RUNTIME_SCHEMA_VERSION}"
        ))),
        None => {
            conn.execute(
                "insert into runtime_meta (
                    singleton, schema_version, next_submission_seq, next_frame_offset
                ) values (1, ?1, ?2, ?3)",
                params![RUNTIME_SCHEMA_VERSION, 0_u64, 0_u64,],
            )?;
            Ok(())
        }
    }
}
