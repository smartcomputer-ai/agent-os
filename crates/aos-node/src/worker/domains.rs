use std::sync::Arc;

use aos_node::{LocalStatePaths, UniverseId, WorldConfig};

use crate::blobstore::{HostedBlobMetaStore, HostedCas, open_hosted_cas_for_universe};

use super::types::{HostedWorkerInfra, WorkerError};

impl HostedWorkerInfra {
    pub(super) fn domain_paths(&self, universe_id: UniverseId) -> LocalStatePaths {
        self.paths.for_universe(universe_id)
    }

    pub(super) fn world_config_for_domain(
        &self,
        universe_id: UniverseId,
    ) -> Result<WorldConfig, WorkerError> {
        let mut config = self.world_config.clone();
        let domain_paths = self.domain_paths(universe_id);
        domain_paths.ensure_root().map_err(|err| {
            WorkerError::Persist(aos_node::PersistError::backend(err.to_string()))
        })?;
        std::fs::create_dir_all(domain_paths.cache_root()).map_err(|err| {
            WorkerError::Persist(aos_node::PersistError::backend(format!(
                "create hosted domain cache dir: {err}"
            )))
        })?;
        config.module_cache_dir = Some(domain_paths.wasmtime_cache_dir());
        Ok(config)
    }

    pub(super) fn store_for_domain(
        &mut self,
        universe_id: UniverseId,
    ) -> Result<Arc<HostedCas>, WorkerError> {
        if let Some(store) = self.stores_by_domain.get(&universe_id) {
            return Ok(Arc::clone(store));
        }
        let hosted =
            open_hosted_cas_for_universe(&self.paths, &self.blobstore_config, universe_id)?;
        self.stores_by_domain
            .insert(universe_id, Arc::clone(&hosted));
        Ok(hosted)
    }

    pub(super) fn checkpoint_backend_for_domain_mut(
        &mut self,
        universe_id: UniverseId,
    ) -> Result<&mut HostedBlobMetaStore, WorkerError> {
        self.checkpoints
            .backend_for_domain_mut(&self.blobstore_config, &self.journal, universe_id)
    }
}
