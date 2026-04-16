use std::sync::Arc;

use aos_node::{FsCas, LocalStatePaths, UniverseId, WorldConfig};

use crate::blobstore::{HostedBlobMetaStore, HostedCas, RemoteCasStore, scoped_blobstore_config};

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
        let domain_paths = self.domain_paths(universe_id);
        domain_paths.ensure_root().map_err(|err| {
            WorkerError::Persist(aos_node::PersistError::backend(err.to_string()))
        })?;
        std::fs::create_dir_all(domain_paths.cache_root()).map_err(|err| {
            WorkerError::Persist(aos_node::PersistError::backend(format!(
                "create hosted domain cache dir: {err}"
            )))
        })?;
        let local_cas = Arc::new(FsCas::open_with_paths(&domain_paths)?);
        let remote = Arc::new(RemoteCasStore::new(scoped_blobstore_config(
            &self.blobstore_config,
            universe_id,
        ))?);
        let hosted = Arc::new(HostedCas::new(local_cas, remote));
        self.stores_by_domain
            .insert(universe_id, Arc::clone(&hosted));
        Ok(hosted)
    }

    pub(super) fn blob_meta_for_domain_mut(
        &mut self,
        universe_id: UniverseId,
    ) -> Result<&mut HostedBlobMetaStore, WorkerError> {
        if !self.blob_meta_by_domain.contains_key(&universe_id) {
            let scoped = scoped_blobstore_config(&self.blobstore_config, universe_id);
            let mut backend = HostedBlobMetaStore::new(scoped)?;
            backend.prime_latest_checkpoints(
                &self.kafka.config().journal_topic,
                self.kafka.partition_count(),
            )?;
            self.blob_meta_by_domain.insert(universe_id, backend);
        }
        let backend = self
            .blob_meta_by_domain
            .get_mut(&universe_id)
            .ok_or(WorkerError::RuntimePoisoned)?;
        backend.prime_latest_checkpoints(
            &self.kafka.config().journal_topic,
            self.kafka.partition_count(),
        )?;
        Ok(backend)
    }
}
