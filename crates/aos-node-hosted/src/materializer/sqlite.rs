use std::path::PathBuf;
use std::str::FromStr;

use aos_cbor::Hash;
use aos_node::api::{
    HeadInfoResponse, JournalEntriesResponse, JournalEntryResponse, RawJournalEntriesResponse,
    RawJournalEntryResponse,
};
use aos_node::{CborPayload, UniverseId, WorldId};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use thiserror::Error;

use crate::kafka::WorldMetaProjection;

use super::projection::{
    CellStateProjectionRecord, HeadProjectionRecord, WorkspaceRegistryProjectionRecord,
};

const MATERIALIZER_SCHEMA_VERSION: i64 = 4;

#[derive(Debug, Error)]
pub enum MaterializerStoreError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Cbor(#[from] serde_cbor::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("backend error: {0}")]
    Backend(String),
}

#[derive(Debug, Clone)]
pub struct MaterializerSqliteConfig {
    pub db_path: PathBuf,
}

impl MaterializerSqliteConfig {
    pub fn from_paths(paths: &aos_node::LocalStatePaths) -> Self {
        Self {
            db_path: paths.root().join("materializer.sqlite3"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializerSourceOffsetRow {
    pub journal_topic: String,
    pub partition: u32,
    pub last_offset: i64,
    pub updated_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializedCellRow {
    pub cell: CellStateProjectionRecord,
    pub state_payload: CborPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializedJournalStateRow {
    pub journal_head: u64,
    pub retained_from: u64,
    pub manifest_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializedWorldRow {
    pub world_id: WorldId,
    pub universe_id: aos_node::UniverseId,
    pub journal_head: u64,
    pub manifest_hash: String,
    pub active_baseline: aos_node::SnapshotRecord,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterializedJournalEntryRow {
    pub seq: u64,
    pub kind: String,
    pub record: JsonValue,
    #[serde(with = "serde_bytes")]
    pub raw_cbor: Vec<u8>,
}

pub struct MaterializerSqliteStore {
    config: MaterializerSqliteConfig,
    conn: Connection,
}

impl MaterializerSqliteStore {
    pub fn from_paths(paths: &aos_node::LocalStatePaths) -> Result<Self, MaterializerStoreError> {
        Self::new(MaterializerSqliteConfig::from_paths(paths))
    }

    pub fn new(config: MaterializerSqliteConfig) -> Result<Self, MaterializerStoreError> {
        if let Some(parent) = config.db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&config.db_path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        initialize_schema(&conn)?;
        Ok(Self { config, conn })
    }

    pub fn config(&self) -> &MaterializerSqliteConfig {
        &self.config
    }

    pub fn load_source_offsets(
        &self,
    ) -> Result<Vec<MaterializerSourceOffsetRow>, MaterializerStoreError> {
        let mut stmt = self.conn.prepare(
            "select journal_topic, partition, last_offset, updated_at_ns
             from source_offsets
             order by journal_topic asc, partition asc",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(MaterializerSourceOffsetRow {
                journal_topic: row.get(0)?,
                partition: row.get(1)?,
                last_offset: row.get(2)?,
                updated_at_ns: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn load_source_offset(
        &self,
        journal_topic: &str,
        partition: u32,
    ) -> Result<Option<MaterializerSourceOffsetRow>, MaterializerStoreError> {
        self.conn
            .query_row(
                "select journal_topic, partition, last_offset, updated_at_ns
                 from source_offsets
                 where journal_topic = ?1 and partition = ?2",
                params![journal_topic, partition],
                |row| {
                    Ok(MaterializerSourceOffsetRow {
                        journal_topic: row.get(0)?,
                        partition: row.get(1)?,
                        last_offset: row.get(2)?,
                        updated_at_ns: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn persist_source_offset(
        &self,
        row: &MaterializerSourceOffsetRow,
    ) -> Result<(), MaterializerStoreError> {
        self.conn.execute(
            "insert into source_offsets (
                journal_topic, partition, last_offset, updated_at_ns
            ) values (?1, ?2, ?3, ?4)
            on conflict(journal_topic, partition) do update set
                last_offset = excluded.last_offset,
                updated_at_ns = excluded.updated_at_ns",
            params![
                &row.journal_topic,
                row.partition,
                row.last_offset,
                row.updated_at_ns,
            ],
        )?;
        Ok(())
    }

    pub fn load_projection_token(
        &self,
        world_id: WorldId,
    ) -> Result<Option<String>, MaterializerStoreError> {
        self.conn
            .query_row(
                "select projection_token from world_projection where world_id = ?1",
                params![world_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    fn load_projection_world_state(
        &self,
        world_id: WorldId,
    ) -> Result<Option<(String, UniverseId)>, MaterializerStoreError> {
        self.conn
            .query_row(
                "select projection_token, universe_id from world_projection where world_id = ?1",
                params![world_id.to_string()],
                |row| {
                    let token: String = row.get(0)?;
                    let universe_id =
                        UniverseId::from_str(&row.get::<_, String>(1)?).map_err(|_| {
                            rusqlite::Error::FromSqlConversionFailure(
                                1,
                                rusqlite::types::Type::Text,
                                Box::new(std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    "invalid universe_id",
                                )),
                            )
                        })?;
                    Ok((token, universe_id))
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn apply_world_meta_projection(
        &mut self,
        world_id: WorldId,
        record: &WorldMetaProjection,
    ) -> Result<bool, MaterializerStoreError> {
        let tx = self.conn.transaction()?;
        let world_id_text = world_id.to_string();
        let universe_id = record.universe_id.to_string();
        let existing_token = tx
            .query_row(
                "select projection_token from world_projection where world_id = ?1",
                params![&world_id_text],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let token_changed = existing_token
            .as_ref()
            .is_some_and(|existing| existing != &record.projection_token);

        if token_changed {
            tx.execute(
                "delete from workspace_projection where world_id = ?1",
                params![&world_id_text],
            )?;
            tx.execute(
                "delete from cell_projection where world_id = ?1",
                params![&world_id_text],
            )?;
            tx.execute(
                "delete from journal_entries where world_id = ?1",
                params![&world_id_text],
            )?;
        }

        tx.execute(
            "insert into world_projection (
                universe_id, world_id, journal_head, manifest_hash, active_baseline, updated_at_ns,
                projection_token, world_epoch
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            on conflict(world_id) do update set
                universe_id = excluded.universe_id,
                journal_head = excluded.journal_head,
                manifest_hash = excluded.manifest_hash,
                active_baseline = excluded.active_baseline,
                updated_at_ns = excluded.updated_at_ns,
                projection_token = excluded.projection_token,
                world_epoch = excluded.world_epoch",
            params![
                &universe_id,
                &world_id_text,
                record.journal_head,
                &record.manifest_hash,
                serde_cbor::to_vec(&record.active_baseline)?,
                record.updated_at_ns,
                &record.projection_token,
                record.world_epoch,
            ],
        )?;

        let head = HeadProjectionRecord {
            journal_head: record.journal_head,
            manifest_hash: record.manifest_hash.clone(),
            universe_id: record.universe_id,
            updated_at_ns: record.updated_at_ns,
        };
        tx.execute(
            "insert into head_projection (
                universe_id, world_id, journal_head, manifest_hash, updated_at_ns, record
            ) values (?1, ?2, ?3, ?4, ?5, ?6)
            on conflict(world_id) do update set
                universe_id = excluded.universe_id,
                journal_head = excluded.journal_head,
                manifest_hash = excluded.manifest_hash,
                updated_at_ns = excluded.updated_at_ns,
                record = excluded.record",
            params![
                &universe_id,
                &world_id_text,
                head.journal_head,
                &head.manifest_hash,
                head.updated_at_ns,
                serde_cbor::to_vec(&head)?,
            ],
        )?;

        if token_changed || existing_token.is_none() {
            tx.execute(
                "insert into journal_state (
                    universe_id, world_id, journal_head, retained_from, manifest_hash
                ) values (?1, ?2, ?3, ?4, ?5)
                on conflict(world_id) do update set
                    universe_id = excluded.universe_id,
                    journal_head = excluded.journal_head,
                    retained_from = excluded.retained_from,
                    manifest_hash = excluded.manifest_hash",
                params![
                    &universe_id,
                    &world_id_text,
                    record.journal_head,
                    record.journal_head.saturating_add(1),
                    &record.manifest_hash,
                ],
            )?;
        }

        tx.commit()?;
        Ok(token_changed)
    }

    pub fn apply_workspace_projection(
        &self,
        world_id: WorldId,
        projection_token: &str,
        record: &WorkspaceRegistryProjectionRecord,
    ) -> Result<bool, MaterializerStoreError> {
        let Some((current_token, universe_id)) = self.load_projection_world_state(world_id)? else {
            return Ok(false);
        };
        if current_token != projection_token {
            return Ok(false);
        }
        self.upsert_workspace_projection(universe_id, world_id, record)?;
        Ok(true)
    }

    pub fn apply_cell_projection(
        &self,
        world_id: WorldId,
        projection_token: &str,
        row: &MaterializedCellRow,
    ) -> Result<bool, MaterializerStoreError> {
        let Some((current_token, universe_id)) = self.load_projection_world_state(world_id)? else {
            return Ok(false);
        };
        if current_token != projection_token {
            return Ok(false);
        }
        self.upsert_cell_projection(universe_id, world_id, row)?;
        Ok(true)
    }

    pub fn load_head_projection(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<Option<HeadProjectionRecord>, MaterializerStoreError> {
        let _ = universe_id;
        self.conn
            .query_row(
                "select record from head_projection where world_id = ?1",
                params![world_id.to_string()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .map(|bytes| serde_cbor::from_slice(&bytes).map_err(Into::into))
            .transpose()
    }

    fn upsert_workspace_projection(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        record: &WorkspaceRegistryProjectionRecord,
    ) -> Result<(), MaterializerStoreError> {
        let _ = universe_id;
        self.conn.execute(
            "insert into workspace_projection (
                universe_id, world_id, workspace, journal_head, latest_version, updated_at_ns, record
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            on conflict(world_id, workspace) do update set
                universe_id = excluded.universe_id,
                journal_head = excluded.journal_head,
                latest_version = excluded.latest_version,
                updated_at_ns = excluded.updated_at_ns,
                record = excluded.record",
            params![
                universe_id.to_string(),
                world_id.to_string(),
                record.workspace,
                record.journal_head,
                record.latest_version,
                record.updated_at_ns,
                serde_cbor::to_vec(record)?,
            ],
        )?;
        Ok(())
    }

    fn delete_workspace_projection(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workspace: &str,
    ) -> Result<(), MaterializerStoreError> {
        let _ = universe_id;
        self.conn.execute(
            "delete from workspace_projection
             where world_id = ?1 and workspace = ?2",
            params![world_id.to_string(), workspace],
        )?;
        Ok(())
    }

    pub fn apply_workspace_tombstone(
        &self,
        world_id: WorldId,
        workspace: &str,
    ) -> Result<bool, MaterializerStoreError> {
        let Some((_token, universe_id)) = self.load_projection_world_state(world_id)? else {
            return Ok(false);
        };
        self.delete_workspace_projection(universe_id, world_id, workspace)?;
        Ok(true)
    }

    pub fn load_workspace_projection(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workspace: &str,
    ) -> Result<Option<WorkspaceRegistryProjectionRecord>, MaterializerStoreError> {
        let _ = universe_id;
        self.conn
            .query_row(
                "select record from workspace_projection
                 where world_id = ?1 and workspace = ?2",
                params![world_id.to_string(), workspace],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .map(|bytes| serde_cbor::from_slice(&bytes).map_err(Into::into))
            .transpose()
    }

    pub fn load_workspace_projections(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<Vec<WorkspaceRegistryProjectionRecord>, MaterializerStoreError> {
        let _ = universe_id;
        let mut stmt = self.conn.prepare(
            "select record from workspace_projection
             where world_id = ?1
             order by workspace asc",
        )?;
        let rows = stmt.query_map(params![world_id.to_string()], |row| {
            row.get::<_, Vec<u8>>(0)
        })?;
        rows.map(|row| Ok(serde_cbor::from_slice(&row?)?)).collect()
    }

    fn upsert_cell_projection(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        row: &MaterializedCellRow,
    ) -> Result<(), MaterializerStoreError> {
        let _ = universe_id;
        self.conn.execute(
            "insert into cell_projection (
                universe_id, world_id, workflow, key_hash, key_bytes, journal_head, state_hash,
                size, last_active_ns, record, state_payload
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            on conflict(world_id, workflow, key_hash, key_bytes) do update set
                universe_id = excluded.universe_id,
                journal_head = excluded.journal_head,
                state_hash = excluded.state_hash,
                size = excluded.size,
                last_active_ns = excluded.last_active_ns,
                record = excluded.record,
                state_payload = excluded.state_payload",
            params![
                universe_id.to_string(),
                world_id.to_string(),
                &row.cell.workflow,
                &row.cell.key_hash,
                &row.cell.key_bytes,
                row.cell.journal_head,
                &row.cell.state_hash,
                row.cell.size,
                row.cell.last_active_ns,
                serde_cbor::to_vec(&row.cell)?,
                serde_cbor::to_vec(&row.state_payload)?,
            ],
        )?;
        Ok(())
    }

    fn delete_cell_projection_by_hash(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        key_hash: &[u8],
    ) -> Result<(), MaterializerStoreError> {
        let _ = universe_id;
        self.conn.execute(
            "delete from cell_projection
             where world_id = ?1 and workflow = ?2 and key_hash = ?3",
            params![world_id.to_string(), workflow, key_hash],
        )?;
        Ok(())
    }

    pub fn apply_cell_tombstone(
        &self,
        world_id: WorldId,
        workflow: &str,
        key_hash: &[u8],
    ) -> Result<bool, MaterializerStoreError> {
        let Some((_token, universe_id)) = self.load_projection_world_state(world_id)? else {
            return Ok(false);
        };
        self.delete_cell_projection_by_hash(universe_id, world_id, workflow, key_hash)?;
        Ok(true)
    }

    pub fn load_cell_projection(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        key_bytes: &[u8],
    ) -> Result<Option<MaterializedCellRow>, MaterializerStoreError> {
        let _ = universe_id;
        self.conn
            .query_row(
                "select record, state_payload from cell_projection
                 where world_id = ?1 and workflow = ?2 and key_hash = ?3 and key_bytes = ?4",
                params![
                    world_id.to_string(),
                    workflow,
                    Hash::of_bytes(key_bytes).as_bytes().to_vec(),
                    key_bytes,
                ],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?
            .map(|(record, payload)| {
                Ok(MaterializedCellRow {
                    cell: serde_cbor::from_slice(&record)?,
                    state_payload: serde_cbor::from_slice(&payload)?,
                })
            })
            .transpose()
    }

    pub fn load_cell_projections(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        limit: u32,
    ) -> Result<Vec<MaterializedCellRow>, MaterializerStoreError> {
        let _ = universe_id;
        let mut stmt = self.conn.prepare(
            "select record, state_payload from cell_projection
             where world_id = ?1 and workflow = ?2
             order by key_bytes asc
             limit ?3",
        )?;
        let rows = stmt.query_map(params![world_id.to_string(), workflow, limit,], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        rows.map(|row| {
            let (record, payload) = row?;
            Ok(MaterializedCellRow {
                cell: serde_cbor::from_slice(&record)?,
                state_payload: serde_cbor::from_slice(&payload)?,
            })
        })
        .collect()
    }

    pub fn persist_journal_state(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        state: &MaterializedJournalStateRow,
    ) -> Result<(), MaterializerStoreError> {
        let _ = universe_id;
        self.conn.execute(
            "insert into journal_state (
                universe_id, world_id, journal_head, retained_from, manifest_hash
            ) values (?1, ?2, ?3, ?4, ?5)
            on conflict(world_id) do update set
                universe_id = excluded.universe_id,
                journal_head = excluded.journal_head,
                retained_from = excluded.retained_from,
                manifest_hash = excluded.manifest_hash",
            params![
                universe_id.to_string(),
                world_id.to_string(),
                state.journal_head,
                state.retained_from,
                &state.manifest_hash,
            ],
        )?;
        Ok(())
    }

    pub fn load_journal_state(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<Option<MaterializedJournalStateRow>, MaterializerStoreError> {
        let _ = universe_id;
        self.conn
            .query_row(
                "select journal_head, retained_from, manifest_hash
                 from journal_state where world_id = ?1",
                params![world_id.to_string()],
                |row| {
                    Ok(MaterializedJournalStateRow {
                        journal_head: row.get(0)?,
                        retained_from: row.get(1)?,
                        manifest_hash: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn persist_journal_entry(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        row: &MaterializedJournalEntryRow,
    ) -> Result<(), MaterializerStoreError> {
        let _ = universe_id;
        self.conn.execute(
            "insert into journal_entries (
                universe_id, world_id, seq, kind, record_json, raw_cbor
            ) values (?1, ?2, ?3, ?4, ?5, ?6)
            on conflict(world_id, seq) do update set
                universe_id = excluded.universe_id,
                kind = excluded.kind,
                record_json = excluded.record_json,
                raw_cbor = excluded.raw_cbor",
            params![
                universe_id.to_string(),
                world_id.to_string(),
                row.seq,
                &row.kind,
                serde_json::to_vec(&row.record)?,
                &row.raw_cbor,
            ],
        )?;
        Ok(())
    }

    pub fn append_journal_entries(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
        journal_head: u64,
        manifest_hash: Option<String>,
        rows: &[MaterializedJournalEntryRow],
        retained_journal_entries_per_world: Option<u64>,
    ) -> Result<(), MaterializerStoreError> {
        let tx = self.conn.transaction()?;
        let universe_id = universe_id.to_string();
        let world_id = world_id.to_string();
        let retained_from = tx
            .query_row(
                "select retained_from from journal_state where world_id = ?1",
                params![&world_id],
                |row| row.get::<_, u64>(0),
            )
            .optional()?
            .unwrap_or(0);

        for row in rows {
            tx.execute(
                "insert into journal_entries (
                    universe_id, world_id, seq, kind, record_json, raw_cbor
                ) values (?1, ?2, ?3, ?4, ?5, ?6)
                on conflict(world_id, seq) do update set
                    universe_id = excluded.universe_id,
                    kind = excluded.kind,
                    record_json = excluded.record_json,
                    raw_cbor = excluded.raw_cbor",
                params![
                    &universe_id,
                    &world_id,
                    row.seq,
                    &row.kind,
                    serde_json::to_vec(&row.record)?,
                    &row.raw_cbor,
                ],
            )?;
        }

        let mut retained_from = retained_from;
        if let Some(limit) = retained_journal_entries_per_world {
            let next_seq = journal_head.saturating_add(1);
            let target_retained_from = next_seq.saturating_sub(limit);
            if target_retained_from > retained_from {
                tx.execute(
                    "delete from journal_entries
                     where world_id = ?1 and seq < ?2",
                    params![&world_id, target_retained_from],
                )?;
                retained_from = target_retained_from;
            }
        }

        tx.execute(
            "insert into journal_state (
                universe_id, world_id, journal_head, retained_from, manifest_hash
            ) values (?1, ?2, ?3, ?4, ?5)
            on conflict(world_id) do update set
                universe_id = excluded.universe_id,
                journal_head = excluded.journal_head,
                retained_from = excluded.retained_from,
                manifest_hash = coalesce(excluded.manifest_hash, journal_state.manifest_hash)",
            params![
                &universe_id,
                &world_id,
                journal_head,
                retained_from,
                &manifest_hash,
            ],
        )?;

        tx.commit()?;
        Ok(())
    }

    pub fn load_world_projection(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<Option<MaterializedWorldRow>, MaterializerStoreError> {
        let _ = universe_id;
        self.conn
            .query_row(
                "select world_id, universe_id, journal_head, manifest_hash, active_baseline
                 from world_projection where world_id = ?1",
                params![world_id.to_string()],
                |row| {
                    Ok(MaterializedWorldRow {
                        world_id: WorldId::from_str(&row.get::<_, String>(0)?).map_err(|_| {
                            rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Text,
                                Box::new(std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    "invalid world_id",
                                )),
                            )
                        })?,
                        universe_id: aos_node::UniverseId::from_str(&row.get::<_, String>(1)?)
                            .map_err(|_| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    1,
                                    rusqlite::types::Type::Text,
                                    Box::new(std::io::Error::new(
                                        std::io::ErrorKind::InvalidData,
                                        "invalid universe_id",
                                    )),
                                )
                            })?,
                        journal_head: row.get(2)?,
                        manifest_hash: row.get(3)?,
                        active_baseline: serde_cbor::from_slice(&row.get::<_, Vec<u8>>(4)?)
                            .map_err(|err| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    4,
                                    rusqlite::types::Type::Blob,
                                    Box::new(err),
                                )
                            })?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn load_world_projections_page(
        &self,
        universe_id: UniverseId,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<MaterializedWorldRow>, MaterializerStoreError> {
        let _ = universe_id;
        let after = after.map(|value| value.to_string());
        let mut stmt = self.conn.prepare(
            "select world_id, universe_id, journal_head, manifest_hash, active_baseline
             from world_projection
             where (?1 is null or world_id > ?1)
             order by world_id asc
             limit ?2",
        )?;
        let rows = stmt.query_map(params![after, limit], |row| {
            Ok(MaterializedWorldRow {
                world_id: WorldId::from_str(&row.get::<_, String>(0)?).map_err(|_| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "invalid world_id",
                        )),
                    )
                })?,
                universe_id: aos_node::UniverseId::from_str(&row.get::<_, String>(1)?).map_err(
                    |_| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "invalid universe_id",
                            )),
                        )
                    },
                )?,
                journal_head: row.get(2)?,
                manifest_hash: row.get(3)?,
                active_baseline: serde_cbor::from_slice(&row.get::<_, Vec<u8>>(4)?).map_err(
                    |err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Blob,
                            Box::new(err),
                        )
                    },
                )?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn prune_journal_entries_through(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        inclusive_seq: u64,
    ) -> Result<(), MaterializerStoreError> {
        let _ = universe_id;
        self.conn.execute(
            "delete from journal_entries
             where world_id = ?1 and seq <= ?2",
            params![world_id.to_string(), inclusive_seq],
        )?;
        self.conn.execute(
            "update journal_state
             set retained_from = max(retained_from, ?2)
             where world_id = ?1",
            params![world_id.to_string(), inclusive_seq.saturating_add(1),],
        )?;
        Ok(())
    }

    pub fn load_journal_head(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<Option<HeadInfoResponse>, MaterializerStoreError> {
        let Some(state) = self.load_journal_state(universe_id, world_id)? else {
            return Ok(None);
        };
        Ok(Some(HeadInfoResponse {
            journal_head: state.journal_head,
            retained_from: state.retained_from,
            manifest_hash: state.manifest_hash,
        }))
    }

    pub fn load_journal_entries(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<Option<JournalEntriesResponse>, MaterializerStoreError> {
        let Some(state) = self.load_journal_state(universe_id, world_id)? else {
            return Ok(None);
        };
        let from = from.max(state.retained_from);
        let mut stmt = self.conn.prepare(
            "select seq, kind, record_json from journal_entries
             where world_id = ?1 and seq >= ?2
             order by seq asc
             limit ?3",
        )?;
        let rows = stmt.query_map(params![world_id.to_string(), from, limit,], |row| {
            Ok(JournalEntryResponse {
                seq: row.get(0)?,
                kind: row.get(1)?,
                record: serde_json::from_slice(&row.get::<_, Vec<u8>>(2)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
            })
        })?;
        let entries = rows.collect::<Result<Vec<_>, _>>()?;
        let next_from = entries
            .last()
            .map(|entry| entry.seq.saturating_add(1))
            .unwrap_or(from);
        Ok(Some(JournalEntriesResponse {
            from,
            retained_from: state.retained_from,
            next_from,
            entries,
        }))
    }

    pub fn load_journal_entries_raw(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<Option<RawJournalEntriesResponse>, MaterializerStoreError> {
        let Some(state) = self.load_journal_state(universe_id, world_id)? else {
            return Ok(None);
        };
        let from = from.max(state.retained_from);
        let mut stmt = self.conn.prepare(
            "select seq, raw_cbor from journal_entries
             where world_id = ?1 and seq >= ?2
             order by seq asc
             limit ?3",
        )?;
        let rows = stmt.query_map(params![world_id.to_string(), from, limit,], |row| {
            Ok(RawJournalEntryResponse {
                seq: row.get(0)?,
                entry_cbor: row.get(1)?,
            })
        })?;
        let entries = rows.collect::<Result<Vec<_>, _>>()?;
        let next_from = entries
            .last()
            .map(|entry| entry.seq.saturating_add(1))
            .unwrap_or(from);
        Ok(Some(RawJournalEntriesResponse {
            from,
            retained_from: state.retained_from,
            next_from,
            entries,
        }))
    }
}

fn initialize_schema(conn: &Connection) -> Result<(), MaterializerStoreError> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.execute_batch(
        "
        create table if not exists materializer_meta (
            singleton integer primary key check (singleton = 1),
            schema_version integer not null
        );
        ",
    )?;
    let existing: Option<i64> = conn
        .query_row(
            "select schema_version from materializer_meta where singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if existing.is_some_and(|existing| existing != MATERIALIZER_SCHEMA_VERSION) {
        conn.execute_batch(
            "
            drop table if exists source_offsets;
            drop table if exists head_projection;
            drop table if exists world_projection;
            drop table if exists workspace_projection;
            drop table if exists cell_projection;
            drop table if exists journal_state;
            drop table if exists journal_entries;
            delete from materializer_meta;
            ",
        )?;
    }
    conn.execute_batch(
        "
        create table if not exists source_offsets (
            journal_topic text not null,
            partition integer not null,
            last_offset integer not null,
            updated_at_ns integer not null,
            primary key (journal_topic, partition)
        );
        create table if not exists head_projection (
            universe_id text not null,
            world_id text not null,
            journal_head integer not null,
            manifest_hash text not null,
            updated_at_ns integer not null,
            record blob not null,
            primary key (world_id)
        );
        create table if not exists world_projection (
            universe_id text not null,
            world_id text not null,
            journal_head integer not null,
            manifest_hash text not null,
            active_baseline blob not null,
            updated_at_ns integer not null,
            projection_token text not null default '',
            world_epoch integer not null default 0,
            primary key (world_id)
        );
        create table if not exists workspace_projection (
            universe_id text not null,
            world_id text not null,
            workspace text not null,
            journal_head integer not null,
            latest_version integer not null,
            updated_at_ns integer not null,
            record blob not null,
            primary key (world_id, workspace)
        );
        create table if not exists cell_projection (
            universe_id text not null,
            world_id text not null,
            workflow text not null,
            key_hash blob not null,
            key_bytes blob not null,
            journal_head integer not null,
            state_hash text not null,
            size integer not null,
            last_active_ns integer not null,
            record blob not null,
            state_payload blob not null,
            primary key (world_id, workflow, key_hash, key_bytes)
        );
        create index if not exists cell_projection_world_workflow_key_bytes_idx
        on cell_projection (world_id, workflow, key_bytes);
        create table if not exists journal_state (
            universe_id text not null,
            world_id text not null,
            journal_head integer not null,
            retained_from integer not null,
            manifest_hash text,
            primary key (world_id)
        );
        create table if not exists journal_entries (
            universe_id text not null,
            world_id text not null,
            seq integer not null,
            kind text not null,
            record_json blob not null,
            raw_cbor blob not null,
            primary key (world_id, seq)
        );
        ",
    )?;
    conn.execute(
        "insert into materializer_meta (singleton, schema_version) values (1, ?1)
         on conflict(singleton) do update set schema_version = excluded.schema_version",
        params![MATERIALIZER_SCHEMA_VERSION],
    )?;
    Ok(())
}
