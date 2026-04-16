use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use aos_cbor::Hash;
use aos_kernel::{LoadedManifest, ManifestLoader, Store};
use aos_node::{FsCas, LocalStatePaths, PersistError, UniverseId};

use crate::blobstore::{BlobStoreConfig, HostedCas, RemoteCasStore, scoped_blobstore_config};
use crate::worker::WorkerError;

#[derive(Debug)]
struct StandaloneCasBackend {
    paths: LocalStatePaths,
    blobstore_config: BlobStoreConfig,
    stores_by_domain: Mutex<BTreeMap<UniverseId, Arc<HostedCas>>>,
}

impl StandaloneCasBackend {
    fn store_for_domain(&self, universe_id: UniverseId) -> Result<Arc<HostedCas>, WorkerError> {
        if let Some(store) = self
            .stores_by_domain
            .lock()
            .map_err(|_| WorkerError::RuntimePoisoned)?
            .get(&universe_id)
            .cloned()
        {
            return Ok(store);
        }

        let domain_paths = self.paths.for_universe(universe_id);
        domain_paths
            .ensure_root()
            .map_err(|err| WorkerError::Persist(PersistError::backend(err.to_string())))?;
        std::fs::create_dir_all(domain_paths.cache_root()).map_err(|err| {
            WorkerError::Persist(PersistError::backend(format!(
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
            .lock()
            .map_err(|_| WorkerError::RuntimePoisoned)?
            .insert(universe_id, Arc::clone(&hosted));
        Ok(hosted)
    }
}

#[derive(Clone)]
pub struct HostedCasService {
    store_for_domain:
        Arc<dyn Fn(UniverseId) -> Result<Arc<HostedCas>, WorkerError> + Send + Sync + 'static>,
    module_cache_dir_for_domain:
        Option<Arc<dyn Fn(UniverseId) -> Result<PathBuf, WorkerError> + Send + Sync + 'static>>,
}

impl HostedCasService {
    pub fn standalone(paths: LocalStatePaths, blobstore_config: BlobStoreConfig) -> Self {
        let backend = Arc::new(StandaloneCasBackend {
            paths: paths.clone(),
            blobstore_config,
            stores_by_domain: Mutex::new(BTreeMap::new()),
        });
        Self::from_provider_with_module_cache_dir(
            move |universe_id| backend.store_for_domain(universe_id),
            move |universe_id| Ok(paths.for_universe(universe_id).wasmtime_cache_dir()),
        )
    }

    pub(crate) fn from_provider_with_module_cache_dir<F, C>(
        provider: F,
        module_cache_dir_for_domain: C,
    ) -> Self
    where
        F: Fn(UniverseId) -> Result<Arc<HostedCas>, WorkerError> + Send + Sync + 'static,
        C: Fn(UniverseId) -> Result<PathBuf, WorkerError> + Send + Sync + 'static,
    {
        Self {
            store_for_domain: Arc::new(provider),
            module_cache_dir_for_domain: Some(Arc::new(module_cache_dir_for_domain)),
        }
    }

    pub fn store_for_domain(&self, universe_id: UniverseId) -> Result<Arc<HostedCas>, WorkerError> {
        (self.store_for_domain)(universe_id)
    }

    pub fn module_cache_dir_for_domain(
        &self,
        universe_id: UniverseId,
    ) -> Result<Option<PathBuf>, WorkerError> {
        self.module_cache_dir_for_domain
            .as_ref()
            .map(|provider| provider(universe_id))
            .transpose()
    }

    pub fn put_blob(&self, universe_id: UniverseId, bytes: &[u8]) -> Result<Hash, WorkerError> {
        Ok(self.store_for_domain(universe_id)?.put_blob(bytes)?)
    }

    pub fn blob_metadata(&self, universe_id: UniverseId, hash: Hash) -> Result<bool, WorkerError> {
        Ok(self.store_for_domain(universe_id)?.has_blob(hash)?)
    }

    pub fn get_blob(&self, universe_id: UniverseId, hash: Hash) -> Result<Vec<u8>, WorkerError> {
        Ok(self.store_for_domain(universe_id)?.get(hash)?)
    }

    pub fn load_manifest(
        &self,
        universe_id: UniverseId,
        manifest_hash: &str,
    ) -> Result<LoadedManifest, WorkerError> {
        let hash = aos_cbor::Hash::from_hex_str(manifest_hash).map_err(|_| {
            WorkerError::Persist(PersistError::validation(format!(
                "invalid manifest hash '{manifest_hash}'"
            )))
        })?;
        Ok(ManifestLoader::load_from_hash(
            self.store_for_domain(universe_id)?.as_ref(),
            hash,
        )?)
    }
}
