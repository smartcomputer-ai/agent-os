use std::collections::BTreeMap;
use std::time::Duration;

use aos_node::{
    BackendError, JournalBackend, JournalCommit, JournalDisposition, JournalFlush, PersistError,
    WorldDurableHead, WorldId, WorldJournalCursor, WorldLogFrame,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::types::HostedSqliteJournalConfig;

const SQLITE_SCHEMA_VERSION: i64 = 1;

#[derive(Debug)]
pub(crate) struct HostedSqliteBackend {
    config: HostedSqliteJournalConfig,
    conn: Connection,
    fail_next_batch_commit: bool,
}

impl HostedSqliteBackend {
    pub(crate) fn new(config: HostedSqliteJournalConfig) -> Result<Self, BackendError> {
        if let Some(parent) = config.db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| BackendError::Persist(PersistError::backend(err.to_string())))?;
        }
        let conn = Connection::open(&config.db_path).map_err(sqlite_backend_err)?;
        conn.busy_timeout(Duration::from_millis(config.busy_timeout_ms))
            .map_err(sqlite_backend_err)?;
        initialize_schema(&conn)?;
        Ok(Self {
            config,
            conn,
            fail_next_batch_commit: false,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn config(&self) -> &HostedSqliteJournalConfig {
        &self.config
    }

    pub fn debug_fail_next_batch_commit(&mut self) {
        self.fail_next_batch_commit = true;
    }
}

impl JournalBackend for HostedSqliteBackend {
    fn refresh_all(&mut self) -> Result<(), BackendError> {
        Ok(())
    }

    fn refresh_world(&mut self, _world_id: WorldId) -> Result<(), BackendError> {
        Ok(())
    }

    fn world_ids(&self) -> Vec<WorldId> {
        let Ok(mut stmt) = self
            .conn
            .prepare("select world_id from journal_world_heads order by world_id asc")
        else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) else {
            return Vec::new();
        };
        rows.filter_map(Result::ok)
            .filter_map(|value| value.parse::<WorldId>().ok())
            .collect()
    }

    fn durable_head(&self, world_id: WorldId) -> Result<WorldDurableHead, BackendError> {
        Ok(WorldDurableHead {
            next_world_seq: load_world_head(&self.conn, world_id)?
                .map(|head| head.next_world_seq)
                .unwrap_or(0),
        })
    }

    fn world_frames(&self, world_id: WorldId) -> Result<Vec<WorldLogFrame>, BackendError> {
        load_world_frames_query(
            &self.conn,
            "select frame from journal_frames where world_id = ?1 order by world_seq_start asc",
            params![world_id.to_string()],
        )
    }

    fn world_tail_frames(
        &self,
        world_id: WorldId,
        after_world_seq: u64,
        cursor: Option<&WorldJournalCursor>,
    ) -> Result<Vec<WorldLogFrame>, BackendError> {
        if let Some(WorldJournalCursor::Sqlite { frame_offset }) = cursor {
            return load_world_frames_query(
                &self.conn,
                "select frame from journal_frames
                 where world_id = ?1 and frame_offset > ?2 and world_seq_end > ?3
                 order by world_seq_start asc",
                params![world_id.to_string(), frame_offset, after_world_seq],
            );
        }
        load_world_frames_query(
            &self.conn,
            "select frame from journal_frames
             where world_id = ?1 and world_seq_end > ?2
             order by world_seq_start asc",
            params![world_id.to_string(), after_world_seq],
        )
    }

    fn commit_flush(&mut self, flush: JournalFlush) -> Result<JournalCommit, BackendError> {
        if self.fail_next_batch_commit {
            self.fail_next_batch_commit = false;
            return Err(BackendError::Persist(PersistError::backend(
                "sqlite journal failpoint: abort before commit",
            )));
        }
        if flush.frames.is_empty() && flush.dispositions.is_empty() {
            return Ok(JournalCommit::default());
        }

        let tx = self.conn.transaction().map_err(sqlite_backend_err)?;
        let mut next_world_seq = BTreeMap::new();
        let mut world_cursors = BTreeMap::new();

        for frame in flush.frames {
            let expected = if let Some(value) = next_world_seq.get(&frame.world_id) {
                *value
            } else {
                load_world_head_tx(&tx, frame.world_id)?
                    .map(|head| head.next_world_seq)
                    .unwrap_or(0)
            };
            if frame.world_seq_start < expected {
                return Err(BackendError::NonContiguousWorldSeq {
                    universe_id: frame.universe_id,
                    world_id: frame.world_id,
                    expected,
                    actual: frame.world_seq_start,
                });
            }

            let frame_bytes = serde_cbor::to_vec(&frame)?;
            tx.execute(
                "insert into journal_frames (
                    world_id, universe_id, world_epoch, world_seq_start, world_seq_end, frame
                 ) values (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    frame.world_id.to_string(),
                    frame.universe_id.to_string(),
                    frame.world_epoch,
                    frame.world_seq_start,
                    frame.world_seq_end,
                    frame_bytes,
                ],
            )
            .map_err(sqlite_backend_err)?;
            let frame_offset = tx.last_insert_rowid() as u64;
            let updated_next_world_seq = frame.world_seq_end.saturating_add(1);
            tx.execute(
                "insert into journal_world_heads (
                    world_id, next_world_seq, last_frame_offset
                 ) values (?1, ?2, ?3)
                 on conflict(world_id) do update set
                    next_world_seq = excluded.next_world_seq,
                    last_frame_offset = excluded.last_frame_offset",
                params![
                    frame.world_id.to_string(),
                    updated_next_world_seq,
                    frame_offset,
                ],
            )
            .map_err(sqlite_backend_err)?;
            next_world_seq.insert(frame.world_id, updated_next_world_seq);
            world_cursors.insert(frame.world_id, WorldJournalCursor::Sqlite { frame_offset });
        }

        for disposition in flush.dispositions {
            let world_id = disposition_world_id(&disposition);
            let bytes = serde_cbor::to_vec(&disposition)?;
            tx.execute(
                "insert into journal_dispositions (world_id, disposition) values (?1, ?2)",
                params![world_id.to_string(), bytes],
            )
            .map_err(sqlite_backend_err)?;
        }

        tx.commit().map_err(sqlite_backend_err)?;
        Ok(JournalCommit { world_cursors })
    }
}

#[derive(Debug, Clone, Copy)]
struct WorldHead {
    next_world_seq: u64,
}

fn disposition_world_id(disposition: &JournalDisposition) -> WorldId {
    match disposition {
        JournalDisposition::RejectedSubmission { world_id, .. }
        | JournalDisposition::CommandFailure { world_id, .. } => *world_id,
    }
}

fn load_world_head(
    conn: &Connection,
    world_id: WorldId,
) -> Result<Option<WorldHead>, BackendError> {
    conn.query_row(
        "select next_world_seq from journal_world_heads where world_id = ?1",
        params![world_id.to_string()],
        |row| {
            Ok(WorldHead {
                next_world_seq: row.get(0)?,
            })
        },
    )
    .optional()
    .map_err(sqlite_backend_err)
}

fn load_world_head_tx(
    tx: &Transaction<'_>,
    world_id: WorldId,
) -> Result<Option<WorldHead>, BackendError> {
    tx.query_row(
        "select next_world_seq from journal_world_heads where world_id = ?1",
        params![world_id.to_string()],
        |row| {
            Ok(WorldHead {
                next_world_seq: row.get(0)?,
            })
        },
    )
    .optional()
    .map_err(sqlite_backend_err)
}

fn load_world_frames_query<P: rusqlite::Params>(
    conn: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<WorldLogFrame>, BackendError> {
    let mut stmt = conn.prepare(sql).map_err(sqlite_backend_err)?;
    let rows = stmt
        .query_map(params, |row| row.get::<_, Vec<u8>>(0))
        .map_err(sqlite_backend_err)?;
    rows.map(|row| {
        let bytes = row.map_err(sqlite_backend_err)?;
        serde_cbor::from_slice::<WorldLogFrame>(&bytes).map_err(BackendError::from)
    })
    .collect()
}

fn initialize_schema(conn: &Connection) -> Result<(), BackendError> {
    conn.execute_batch(
        "
        pragma journal_mode = WAL;
        pragma synchronous = FULL;
        pragma foreign_keys = ON;

        create table if not exists journal_meta (
            singleton integer primary key check (singleton = 1),
            schema_version integer not null
        );

        create table if not exists journal_frames (
            frame_offset integer primary key,
            world_id text not null,
            universe_id text not null,
            world_epoch integer not null,
            world_seq_start integer not null,
            world_seq_end integer not null,
            frame blob not null
        );

        create index if not exists journal_frames_world_seq_idx
            on journal_frames(world_id, world_seq_start);

        create index if not exists journal_frames_world_offset_idx
            on journal_frames(world_id, frame_offset);

        create table if not exists journal_world_heads (
            world_id text primary key,
            next_world_seq integer not null,
            last_frame_offset integer not null
        );

        create table if not exists journal_dispositions (
            disposition_offset integer primary key,
            world_id text not null,
            disposition blob not null
        );
        ",
    )
    .map_err(sqlite_backend_err)?;

    let existing = conn
        .query_row(
            "select schema_version from journal_meta where singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(sqlite_backend_err)?;
    match existing {
        Some(version) if version == SQLITE_SCHEMA_VERSION => Ok(()),
        Some(version) => Err(BackendError::Persist(PersistError::backend(format!(
            "sqlite journal schema version {version} does not match expected {SQLITE_SCHEMA_VERSION}"
        )))),
        None => {
            conn.execute(
                "insert into journal_meta (singleton, schema_version) values (1, ?1)",
                params![SQLITE_SCHEMA_VERSION],
            )
            .map_err(sqlite_backend_err)?;
            Ok(())
        }
    }
}

fn sqlite_backend_err(err: rusqlite::Error) -> BackendError {
    BackendError::Persist(PersistError::backend(format!(
        "sqlite journal backend: {err}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    use aos_kernel::journal::{CustomRecord, JournalRecord};
    use aos_node::{UniverseId, WorldLogFrame};
    use tempfile::tempdir;

    fn frame(
        universe_id: UniverseId,
        world_id: WorldId,
        world_seq_start: u64,
        world_seq_end: u64,
        tag: &str,
    ) -> WorldLogFrame {
        WorldLogFrame {
            format_version: 1,
            universe_id,
            world_id,
            world_epoch: 1,
            world_seq_start,
            world_seq_end,
            records: vec![JournalRecord::Custom(CustomRecord {
                tag: tag.into(),
                data: vec![1, 2, 3],
            })],
        }
    }

    #[test]
    fn sqlite_commit_flush_persists_frames_and_heads() {
        let temp = tempdir().unwrap();
        let config = HostedSqliteJournalConfig {
            db_path: temp.path().join("journal.sqlite3"),
            busy_timeout_ms: 1_000,
        };
        let mut backend = HostedSqliteBackend::new(config).unwrap();
        let universe_id = UniverseId::from(uuid::Uuid::new_v4());
        let world_id = WorldId::from(uuid::Uuid::new_v4());

        let commit = backend
            .commit_flush(JournalFlush {
                frames: vec![frame(universe_id, world_id, 0, 0, "frame-0")],
                dispositions: Vec::new(),
                source_acks: Vec::new(),
            })
            .unwrap();

        assert_eq!(backend.world_frames(world_id).unwrap().len(), 1);
        assert_eq!(backend.durable_head(world_id).unwrap().next_world_seq, 1);
        assert!(matches!(
            commit.world_cursors.get(&world_id),
            Some(WorldJournalCursor::Sqlite { frame_offset: 1 })
        ));
    }

    #[test]
    fn sqlite_world_tail_frames_respects_sqlite_cursor() {
        let temp = tempdir().unwrap();
        let config = HostedSqliteJournalConfig {
            db_path: temp.path().join("journal.sqlite3"),
            busy_timeout_ms: 1_000,
        };
        let mut backend = HostedSqliteBackend::new(config).unwrap();
        let universe_id = UniverseId::from(uuid::Uuid::new_v4());
        let world_id = WorldId::from(uuid::Uuid::new_v4());

        let first = backend
            .commit_flush(JournalFlush {
                frames: vec![
                    frame(universe_id, world_id, 0, 0, "frame-0"),
                    frame(universe_id, world_id, 1, 1, "frame-1"),
                ],
                dispositions: Vec::new(),
                source_acks: Vec::new(),
            })
            .unwrap();
        backend
            .commit_flush(JournalFlush {
                frames: vec![frame(universe_id, world_id, 2, 2, "frame-2")],
                dispositions: Vec::new(),
                source_acks: Vec::new(),
            })
            .unwrap();

        let tail = backend
            .world_tail_frames(world_id, 1, first.world_cursors.get(&world_id))
            .unwrap();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].world_seq_start, 2);
    }
}
