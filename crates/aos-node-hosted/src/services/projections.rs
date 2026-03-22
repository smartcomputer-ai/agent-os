use std::sync::{Mutex, MutexGuard};

use aos_node::{UniverseId, WorldId};

use crate::materializer::HeadProjectionRecord;
use crate::materializer::{
    MaterializedCellRow, MaterializedWorldRow, MaterializerSqliteStore, MaterializerStoreError,
    WorkspaceRegistryProjectionRecord,
};

pub struct HostedProjectionStore {
    inner: Mutex<MaterializerSqliteStore>,
}

impl HostedProjectionStore {
    pub fn new(store: MaterializerSqliteStore) -> Self {
        Self {
            inner: Mutex::new(store),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, MaterializerSqliteStore>, MaterializerStoreError> {
        self.inner.lock().map_err(|_| {
            MaterializerStoreError::Backend("materializer sqlite mutex poisoned".into())
        })
    }

    pub fn load_world_projections_page(
        &self,
        universe_id: UniverseId,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<MaterializedWorldRow>, MaterializerStoreError> {
        self.lock()?
            .load_world_projections_page(universe_id, after, limit)
    }

    pub fn load_world_projection(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<Option<MaterializedWorldRow>, MaterializerStoreError> {
        self.lock()?.load_world_projection(universe_id, world_id)
    }

    pub fn load_head_projection(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<Option<HeadProjectionRecord>, MaterializerStoreError> {
        self.lock()?.load_head_projection(universe_id, world_id)
    }

    pub fn load_cell_projection(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        key_bytes: &[u8],
    ) -> Result<Option<MaterializedCellRow>, MaterializerStoreError> {
        self.lock()?
            .load_cell_projection(universe_id, world_id, workflow, key_bytes)
    }

    pub fn load_cell_projections(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        limit: u32,
    ) -> Result<Vec<MaterializedCellRow>, MaterializerStoreError> {
        self.lock()?
            .load_cell_projections(universe_id, world_id, workflow, limit)
    }

    pub fn load_workspace_projection(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workspace: &str,
    ) -> Result<Option<WorkspaceRegistryProjectionRecord>, MaterializerStoreError> {
        self.lock()?
            .load_workspace_projection(universe_id, world_id, workspace)
    }

    pub fn load_journal_head(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<Option<aos_node::api::HeadInfoResponse>, MaterializerStoreError> {
        self.lock()?.load_journal_head(universe_id, world_id)
    }

    pub fn load_journal_entries(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<Option<aos_node::api::JournalEntriesResponse>, MaterializerStoreError> {
        self.lock()?
            .load_journal_entries(universe_id, world_id, from, limit)
    }

    pub fn load_journal_entries_raw(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<Option<aos_node::api::RawJournalEntriesResponse>, MaterializerStoreError> {
        self.lock()?
            .load_journal_entries_raw(universe_id, world_id, from, limit)
    }
}
