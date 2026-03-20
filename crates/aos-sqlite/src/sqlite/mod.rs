mod admin;
mod schema;
mod util;
mod world;

use std::sync::Mutex;

use aos_node::{PersistError, UniverseId};
use rusqlite::{Connection, Transaction};

use crate::fs_cas::FsCas;
use crate::paths::LocalStatePaths;

pub struct SqliteNodeStore {
    connection: Mutex<Connection>,
    cas: FsCas,
    universe_id: UniverseId,
}

impl SqliteNodeStore {
    pub fn open_with_paths(paths: &LocalStatePaths) -> Result<Self, PersistError> {
        paths
            .purge_legacy_state()
            .map_err(|err| PersistError::backend(format!("purge legacy local state: {err}")))?;
        if let Some(parent) = paths.sqlite_db().parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| PersistError::backend(format!("create sqlite dir: {err}")))?;
        }
        let connection = Connection::open(paths.sqlite_db())
            .map_err(|err| PersistError::backend(format!("open sqlite node db: {err}")))?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|err| PersistError::backend(format!("enable sqlite WAL: {err}")))?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|err| PersistError::backend(format!("enable sqlite foreign keys: {err}")))?;
        schema::initialize(&connection)?;
        let universe_id = schema::ensure_local_universe(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
            cas: FsCas::open_with_paths(paths)?,
            universe_id,
        })
    }

    pub fn local_universe_id(&self) -> UniverseId {
        self.universe_id
    }

    pub(super) fn ensure_local_universe(&self, universe: UniverseId) -> Result<(), PersistError> {
        if universe != self.universe_id {
            return Err(PersistError::not_found(format!("universe {universe}")));
        }
        Ok(())
    }

    pub(super) fn read<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, PersistError>,
    ) -> Result<T, PersistError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| PersistError::backend("sqlite node mutex poisoned"))?;
        operation(&connection)
    }

    pub(super) fn write<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T, PersistError>,
    ) -> Result<T, PersistError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| PersistError::backend("sqlite node mutex poisoned"))?;
        let tx = connection
            .transaction()
            .map_err(|err| PersistError::backend(format!("begin sqlite transaction: {err}")))?;
        let value = operation(&tx)?;
        tx.commit()
            .map_err(|err| PersistError::backend(format!("commit sqlite transaction: {err}")))?;
        Ok(value)
    }
}
