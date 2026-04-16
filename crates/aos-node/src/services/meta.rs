use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aos_node::{CheckpointBackend, CommandRecord, UniverseId, WorldCheckpointRef, WorldId};

use crate::blobstore::{BlobStoreConfig, HostedBlobMetaStore, scoped_blobstore_config};
use crate::worker::WorkerError;

#[derive(Debug)]
struct StandaloneMetaBackend {
    blobstore_config: BlobStoreConfig,
    stores_by_domain: Mutex<BTreeMap<UniverseId, HostedBlobMetaStore>>,
}

impl StandaloneMetaBackend {
    fn with_store<T>(
        &self,
        universe_id: UniverseId,
        mut f: impl FnMut(&mut HostedBlobMetaStore) -> Result<T, WorkerError>,
    ) -> Result<T, WorkerError> {
        let mut stores = self
            .stores_by_domain
            .lock()
            .map_err(|_| WorkerError::RuntimePoisoned)?;
        let store = stores.entry(universe_id).or_insert_with(|| {
            HostedBlobMetaStore::new(scoped_blobstore_config(&self.blobstore_config, universe_id))
                .expect("open blob meta store")
        });
        store.prime_latest_checkpoints()?;
        f(store)
    }
}

#[derive(Clone)]
pub struct HostedMetaService {
    get_command_record: Arc<
        dyn Fn(UniverseId, WorldId, &str) -> Result<Option<CommandRecord>, WorkerError>
            + Send
            + Sync
            + 'static,
    >,
    put_command_record: Arc<
        dyn Fn(UniverseId, WorldId, CommandRecord) -> Result<(), WorkerError>
            + Send
            + Sync
            + 'static,
    >,
    latest_world_checkpoint: Arc<
        dyn Fn(UniverseId, WorldId) -> Result<Option<WorldCheckpointRef>, WorkerError>
            + Send
            + Sync
            + 'static,
    >,
}

impl HostedMetaService {
    pub fn standalone(blobstore_config: BlobStoreConfig) -> Self {
        let backend = Arc::new(StandaloneMetaBackend {
            blobstore_config,
            stores_by_domain: Mutex::new(BTreeMap::new()),
        });
        Self::from_callbacks(
            {
                let backend = Arc::clone(&backend);
                move |universe_id, world_id, command_id| {
                    backend.with_store(universe_id, |store: &mut HostedBlobMetaStore| {
                        store
                            .get_command_record(world_id, command_id)
                            .map_err(WorkerError::from)
                    })
                }
            },
            {
                let backend = Arc::clone(&backend);
                move |universe_id, world_id, record| {
                    backend.with_store(universe_id, |store: &mut HostedBlobMetaStore| {
                        store
                            .put_command_record(world_id, record.clone())
                            .map_err(WorkerError::from)
                    })
                }
            },
            {
                let backend = Arc::clone(&backend);
                move |universe_id, world_id| {
                    backend.with_store(universe_id, |store: &mut HostedBlobMetaStore| {
                        store
                            .latest_world_checkpoint(world_id)
                            .map_err(WorkerError::from)
                    })
                }
            },
        )
    }

    pub(crate) fn from_callbacks<G, P, W>(
        get_command_record: G,
        put_command_record: P,
        latest_world_checkpoint: W,
    ) -> Self
    where
        G: Fn(UniverseId, WorldId, &str) -> Result<Option<CommandRecord>, WorkerError>
            + Send
            + Sync
            + 'static,
        P: Fn(UniverseId, WorldId, CommandRecord) -> Result<(), WorkerError>
            + Send
            + Sync
            + 'static,
        W: Fn(UniverseId, WorldId) -> Result<Option<WorldCheckpointRef>, WorkerError>
            + Send
            + Sync
            + 'static,
    {
        Self {
            get_command_record: Arc::new(get_command_record),
            put_command_record: Arc::new(put_command_record),
            latest_world_checkpoint: Arc::new(latest_world_checkpoint),
        }
    }

    pub fn get_command_record(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<Option<CommandRecord>, WorkerError> {
        (self.get_command_record)(universe_id, world_id, command_id)
    }

    pub fn put_command_record(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        record: CommandRecord,
    ) -> Result<(), WorkerError> {
        (self.put_command_record)(universe_id, world_id, record)
    }

    pub fn latest_world_checkpoint(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<Option<WorldCheckpointRef>, WorkerError> {
        (self.latest_world_checkpoint)(universe_id, world_id)
    }
}
