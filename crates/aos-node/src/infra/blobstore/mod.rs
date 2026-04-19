mod cas;
mod fs_cas;

use std::collections::BTreeMap;
use std::sync::Arc;

use aos_cbor::HASH_PREFIX;
use aos_node::{
    BackendError, CheckpointBackend, CommandRecord, LocalStatePaths, PersistError, UniverseId,
    WorldCheckpointRecord, WorldId, WorldInventoryBackend,
};
use futures::TryStreamExt;
use object_store::ObjectStore;
use object_store::ObjectStoreExt;
use object_store::PutPayload;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectPath;

pub use cas::{HostedCas, RemoteCasStore};
pub use fs_cas::FsCas;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobStoreConfig {
    pub bucket: Option<String>,
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub prefix: String,
    pub force_path_style: bool,
    pub pack_threshold_bytes: usize,
    pub pack_target_bytes: usize,
    pub retained_checkpoints_per_partition: usize,
}

impl Default for BlobStoreConfig {
    fn default() -> Self {
        Self {
            bucket: env_or_legacy("AOS_BLOBSTORE_BUCKET", "AOS_S3_BUCKET"),
            endpoint: env_or_legacy("AOS_BLOBSTORE_ENDPOINT", "AOS_S3_ENDPOINT"),
            region: env_or_legacy("AOS_BLOBSTORE_REGION", "AOS_S3_REGION"),
            prefix: env_or_legacy("AOS_BLOBSTORE_PREFIX", "AOS_S3_PREFIX")
                .unwrap_or_else(|| "aos".to_owned()),
            force_path_style: env_or_legacy(
                "AOS_BLOBSTORE_FORCE_PATH_STYLE",
                "AOS_S3_FORCE_PATH_STYLE",
            )
            .and_then(|value| value.parse::<bool>().ok())
            .unwrap_or(true),
            pack_threshold_bytes: env_or_legacy(
                "AOS_BLOBSTORE_PACK_THRESHOLD_BYTES",
                "AOS_S3_PACK_THRESHOLD_BYTES",
            )
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(64 * 1024),
            pack_target_bytes: env_or_legacy(
                "AOS_BLOBSTORE_PACK_TARGET_BYTES",
                "AOS_S3_PACK_TARGET_BYTES",
            )
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(512 * 1024),
            retained_checkpoints_per_partition: env_or_legacy(
                "AOS_BLOBSTORE_RETAINED_CHECKPOINTS",
                "AOS_S3_RETAINED_CHECKPOINTS",
            )
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(3),
        }
    }
}

#[derive(Debug)]
pub struct EmbeddedBlobMetaStore {
    config: BlobStoreConfig,
    latest_checkpoints: BTreeMap<WorldId, WorldCheckpointRecord>,
    command_records: BTreeMap<(WorldId, String), CommandRecord>,
}

impl EmbeddedBlobMetaStore {
    pub fn new(config: BlobStoreConfig) -> Self {
        Self {
            config,
            latest_checkpoints: BTreeMap::new(),
            command_records: BTreeMap::new(),
        }
    }

    pub fn config(&self) -> &BlobStoreConfig {
        &self.config
    }

    pub fn prime_latest_checkpoints(&mut self) -> Result<(), BackendError> {
        Ok(())
    }

    pub fn put_command_record(
        &mut self,
        world_id: WorldId,
        record: CommandRecord,
    ) -> Result<(), BackendError> {
        self.command_records
            .insert((world_id, record.command_id.clone()), record);
        Ok(())
    }

    pub fn get_command_record(&self, world_id: WorldId, command_id: &str) -> Option<CommandRecord> {
        self.command_records
            .get(&(world_id, command_id.to_owned()))
            .cloned()
    }
}

pub struct ObjectStoreBlobMetaStore {
    config: BlobStoreConfig,
    bucket: String,
    store: Arc<dyn ObjectStore>,
    latest_checkpoints: BTreeMap<WorldId, WorldCheckpointRecord>,
    command_records: BTreeMap<(WorldId, String), CommandRecord>,
}

impl ObjectStoreBlobMetaStore {
    pub fn new(config: BlobStoreConfig) -> Result<Self, BackendError> {
        let bucket = config
            .bucket
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                BackendError::Persist(PersistError::validation(
                    "AOS_BLOBSTORE_BUCKET must be set for object-store-backed blob backends",
                ))
            })?;
        let store = build_object_store(&config, &bucket)?;
        Ok(Self {
            config,
            bucket,
            store,
            latest_checkpoints: BTreeMap::new(),
            command_records: BTreeMap::new(),
        })
    }

    #[cfg(test)]
    fn from_store(
        config: BlobStoreConfig,
        bucket: impl Into<String>,
        store: Arc<dyn ObjectStore>,
    ) -> Self {
        Self {
            config,
            bucket: bucket.into(),
            store,
            latest_checkpoints: BTreeMap::new(),
            command_records: BTreeMap::new(),
        }
    }

    pub fn config(&self) -> &BlobStoreConfig {
        &self.config
    }

    pub fn prime_latest_checkpoints(&mut self) -> Result<(), BackendError> {
        let prefix = world_checkpoint_root_prefix(&self.config);
        for key in self.list_object_keys_recursive_sync(&prefix)? {
            if !key.ends_with("/latest.cbor") {
                continue;
            }
            let Some(payload) = self.get_object_sync(&key)? else {
                continue;
            };
            let checkpoint: WorldCheckpointRecord = serde_cbor::from_slice(&payload)?;
            self.latest_checkpoints
                .insert(checkpoint.world_id, checkpoint);
        }
        Ok(())
    }

    pub fn put_command_record(
        &mut self,
        world_id: WorldId,
        record: CommandRecord,
    ) -> Result<(), BackendError> {
        let key = command_record_key(&self.config, world_id, &record.command_id);
        let payload = serde_cbor::to_vec(&record)?;
        self.put_object_sync(key, payload)?;
        self.command_records
            .insert((world_id, record.command_id.clone()), record);
        Ok(())
    }

    pub fn get_command_record(
        &mut self,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<Option<CommandRecord>, BackendError> {
        if let Some(record) = self.command_records.get(&(world_id, command_id.to_owned())) {
            return Ok(Some(record.clone()));
        }
        let key = command_record_key(&self.config, world_id, command_id);
        let Some(payload) = self.get_object_sync(&key)? else {
            return Ok(None);
        };
        let record: CommandRecord = serde_cbor::from_slice(&payload)?;
        self.command_records
            .insert((world_id, command_id.to_owned()), record.clone());
        Ok(Some(record))
    }

    fn put_object_sync(&self, key: String, payload: Vec<u8>) -> Result<(), BackendError> {
        let store = Arc::clone(&self.store);
        let path = ObjectPath::from(key.clone());
        run_async(
            format!("put object-store://{}/{key}", self.bucket),
            async move {
                store
                    .put(&path, PutPayload::from(payload))
                    .await
                    .map(|_| ())
                    .map_err(object_store_backend_err("put object"))
            },
        )
    }

    fn get_object_sync(&self, key: &str) -> Result<Option<Vec<u8>>, BackendError> {
        let store = Arc::clone(&self.store);
        let path = ObjectPath::from(key);
        let label = format!("get object-store://{}/{}", self.bucket, key);
        run_async(label, async move {
            match store.get(&path).await {
                Ok(result) => {
                    let bytes = result
                        .bytes()
                        .await
                        .map_err(object_store_backend_err("read object-store object body"))?;
                    Ok(Some(bytes.to_vec()))
                }
                Err(object_store::Error::NotFound { .. }) => Ok(None),
                Err(err) => Err(object_store_backend_err("get object")(err)),
            }
        })
    }

    fn list_object_keys_sync(&self, prefix: &str) -> Result<Vec<String>, BackendError> {
        let store = Arc::clone(&self.store);
        let path = ObjectPath::from(prefix);
        let label = format!("list object-store://{}/{}", self.bucket, prefix);
        run_async(label, async move {
            let listing = store
                .list_with_delimiter(Some(&path))
                .await
                .map_err(object_store_backend_err("list objects"))?;
            Ok(listing
                .objects
                .into_iter()
                .map(|item| item.location.to_string())
                .collect())
        })
    }

    fn list_object_keys_recursive_sync(&self, prefix: &str) -> Result<Vec<String>, BackendError> {
        let store = Arc::clone(&self.store);
        let path = ObjectPath::from(prefix);
        let label = format!("list object-store://{}/{} recursively", self.bucket, prefix);
        run_async(label, async move {
            let mut stream = store.list(Some(&path));
            let mut keys = Vec::new();
            while let Some(meta) = stream
                .try_next()
                .await
                .map_err(object_store_backend_err("list objects recursively"))?
            {
                keys.push(meta.location.to_string());
            }
            Ok(keys)
        })
    }

    fn delete_object_sync(&self, key: &str) -> Result<(), BackendError> {
        let store = Arc::clone(&self.store);
        let path = ObjectPath::from(key);
        let label = format!("delete object-store://{}/{}", self.bucket, key);
        run_async(label, async move {
            store
                .delete(&path)
                .await
                .map_err(object_store_backend_err("delete object"))
        })
    }

    fn enforce_checkpoint_retention(&self, world_id: WorldId) -> Result<(), BackendError> {
        let retain = self.config.retained_checkpoints_per_partition.max(1);
        let prefix = world_checkpoint_manifest_prefix(&self.config, world_id);
        let mut keys = self.list_object_keys_sync(&prefix)?;
        keys.sort();
        if keys.len() <= retain {
            return Ok(());
        }
        let delete_count = keys.len() - retain;
        for key in keys.into_iter().take(delete_count) {
            self.delete_object_sync(&key)?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for ObjectStoreBlobMetaStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectStoreBlobMetaStore")
            .field("config", &self.config)
            .field("bucket", &self.bucket)
            .field("latest_checkpoints", &self.latest_checkpoints.len())
            .finish()
    }
}

#[derive(Debug)]
pub enum HostedBlobMetaStore {
    Embedded(EmbeddedBlobMetaStore),
    ObjectStore(ObjectStoreBlobMetaStore),
}

impl HostedBlobMetaStore {
    pub fn new(config: BlobStoreConfig) -> Result<Self, BackendError> {
        if config
            .bucket
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Ok(Self::ObjectStore(ObjectStoreBlobMetaStore::new(config)?));
        }
        Ok(Self::Embedded(EmbeddedBlobMetaStore::new(config)))
    }

    pub fn new_embedded(config: BlobStoreConfig) -> Self {
        Self::Embedded(EmbeddedBlobMetaStore::new(config))
    }

    pub fn config(&self) -> &BlobStoreConfig {
        match self {
            Self::Embedded(inner) => inner.config(),
            Self::ObjectStore(inner) => inner.config(),
        }
    }

    pub fn prime_latest_checkpoints(&mut self) -> Result<(), BackendError> {
        match self {
            Self::Embedded(inner) => inner.prime_latest_checkpoints(),
            Self::ObjectStore(inner) => inner.prime_latest_checkpoints(),
        }
    }

    pub fn put_command_record(
        &mut self,
        world_id: WorldId,
        record: CommandRecord,
    ) -> Result<(), BackendError> {
        match self {
            Self::Embedded(inner) => inner.put_command_record(world_id, record),
            Self::ObjectStore(inner) => inner.put_command_record(world_id, record),
        }
    }

    pub fn get_command_record(
        &mut self,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<Option<CommandRecord>, BackendError> {
        match self {
            Self::Embedded(inner) => Ok(inner.get_command_record(world_id, command_id)),
            Self::ObjectStore(inner) => inner.get_command_record(world_id, command_id),
        }
    }
}

impl CheckpointBackend for EmbeddedBlobMetaStore {
    fn commit_world_checkpoint(
        &mut self,
        checkpoint: WorldCheckpointRecord,
    ) -> Result<(), BackendError> {
        self.latest_checkpoints
            .insert(checkpoint.world_id, checkpoint);
        Ok(())
    }

    fn latest_world_checkpoint(
        &self,
        world_id: WorldId,
    ) -> Result<Option<WorldCheckpointRecord>, BackendError> {
        Ok(self.latest_checkpoints.get(&world_id).cloned())
    }

    fn list_world_checkpoints(&self) -> Result<Vec<WorldCheckpointRecord>, BackendError> {
        Ok(self.latest_checkpoints.values().cloned().collect())
    }
}

impl CheckpointBackend for ObjectStoreBlobMetaStore {
    fn commit_world_checkpoint(
        &mut self,
        checkpoint: WorldCheckpointRecord,
    ) -> Result<(), BackendError> {
        let latest_key = world_checkpoint_key(&self.config, checkpoint.world_id);
        let manifest_key = world_checkpoint_manifest_key(
            &self.config,
            checkpoint.world_id,
            checkpoint.checkpointed_at_ns,
        );
        let payload = serde_cbor::to_vec(&checkpoint)?;
        self.put_object_sync(manifest_key, payload.clone())?;
        self.put_object_sync(latest_key, payload)?;
        self.latest_checkpoints
            .insert(checkpoint.world_id, checkpoint.clone());
        self.enforce_checkpoint_retention(checkpoint.world_id)?;
        Ok(())
    }

    fn latest_world_checkpoint(
        &self,
        world_id: WorldId,
    ) -> Result<Option<WorldCheckpointRecord>, BackendError> {
        Ok(self.latest_checkpoints.get(&world_id).cloned())
    }

    fn list_world_checkpoints(&self) -> Result<Vec<WorldCheckpointRecord>, BackendError> {
        Ok(self.latest_checkpoints.values().cloned().collect())
    }
}

impl CheckpointBackend for HostedBlobMetaStore {
    fn commit_world_checkpoint(
        &mut self,
        checkpoint: WorldCheckpointRecord,
    ) -> Result<(), BackendError> {
        match self {
            Self::Embedded(inner) => inner.commit_world_checkpoint(checkpoint),
            Self::ObjectStore(inner) => inner.commit_world_checkpoint(checkpoint),
        }
    }

    fn latest_world_checkpoint(
        &self,
        world_id: WorldId,
    ) -> Result<Option<WorldCheckpointRecord>, BackendError> {
        match self {
            Self::Embedded(inner) => inner.latest_world_checkpoint(world_id),
            Self::ObjectStore(inner) => inner.latest_world_checkpoint(world_id),
        }
    }

    fn list_world_checkpoints(&self) -> Result<Vec<WorldCheckpointRecord>, BackendError> {
        match self {
            Self::Embedded(inner) => inner.list_world_checkpoints(),
            Self::ObjectStore(inner) => inner.list_world_checkpoints(),
        }
    }
}

impl WorldInventoryBackend for EmbeddedBlobMetaStore {
    fn list_worlds(&self) -> Result<Vec<WorldId>, BackendError> {
        let mut worlds = self.latest_checkpoints.keys().copied().collect::<Vec<_>>();
        worlds.sort_unstable();
        Ok(worlds)
    }
}

impl WorldInventoryBackend for ObjectStoreBlobMetaStore {
    fn list_worlds(&self) -> Result<Vec<WorldId>, BackendError> {
        let mut worlds = self.latest_checkpoints.keys().copied().collect::<Vec<_>>();
        worlds.sort_unstable();
        Ok(worlds)
    }
}

impl WorldInventoryBackend for HostedBlobMetaStore {
    fn list_worlds(&self) -> Result<Vec<WorldId>, BackendError> {
        match self {
            Self::Embedded(inner) => inner.list_worlds(),
            Self::ObjectStore(inner) => inner.list_worlds(),
        }
    }
}

pub(crate) fn build_object_store(
    config: &BlobStoreConfig,
    bucket: &str,
) -> Result<Arc<dyn ObjectStore>, BackendError> {
    let region = config
        .region
        .clone()
        .unwrap_or_else(|| "us-east-1".to_owned());
    let endpoint = config.endpoint.clone();
    let force_path_style = config.force_path_style;

    let mut builder = AmazonS3Builder::from_env()
        .with_bucket_name(bucket)
        .with_region(region)
        .with_virtual_hosted_style_request(!force_path_style);

    if let Some(endpoint) = endpoint {
        if endpoint.starts_with("http://") {
            builder = builder.with_allow_http(true);
        }
        builder = builder.with_endpoint(endpoint);
    }

    let store = builder.build().map_err(|err| {
        BackendError::Persist(PersistError::backend(format!(
            "build object store client: {err}"
        )))
    })?;
    Ok(Arc::new(store))
}

pub(crate) fn run_async<T, F>(label: impl Into<String>, future: F) -> Result<T, BackendError>
where
    T: Send + 'static,
    F: std::future::Future<Output = Result<T, BackendError>> + Send + 'static,
{
    let label = label.into();
    let join_label = label.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| {
                BackendError::Persist(PersistError::backend(format!(
                    "create tokio runtime for {label}: {err}"
                )))
            })?;
        runtime.block_on(future)
    })
    .join()
    .map_err(|_| {
        BackendError::Persist(PersistError::backend(format!(
            "join async worker for {join_label}"
        )))
    })?
}

fn world_checkpoint_root_prefix(config: &BlobStoreConfig) -> String {
    format!("{}/checkpoints/worlds", config.prefix.trim_end_matches('/'))
}

fn world_checkpoint_prefix(config: &BlobStoreConfig, world_id: WorldId) -> String {
    format!(
        "{}/checkpoints/worlds/{world_id}",
        config.prefix.trim_end_matches('/')
    )
}

fn world_checkpoint_key(config: &BlobStoreConfig, world_id: WorldId) -> String {
    format!("{}/latest.cbor", world_checkpoint_prefix(config, world_id))
}

fn world_checkpoint_manifest_prefix(config: &BlobStoreConfig, world_id: WorldId) -> String {
    format!("{}/manifests", world_checkpoint_prefix(config, world_id))
}

fn world_checkpoint_manifest_key(
    config: &BlobStoreConfig,
    world_id: WorldId,
    checkpointed_at_ns: u64,
) -> String {
    format!(
        "{}/manifests/{checkpointed_at_ns:020}.cbor",
        world_checkpoint_prefix(config, world_id)
    )
}

fn command_record_key(config: &BlobStoreConfig, world_id: WorldId, command_id: &str) -> String {
    format!(
        "{}/commands/{world_id}/{command_id}.cbor",
        config.prefix.trim_end_matches('/'),
    )
}

pub fn scoped_blobstore_config(base: &BlobStoreConfig, universe_id: UniverseId) -> BlobStoreConfig {
    let mut scoped = base.clone();
    scoped.prefix = format!(
        "{}/universes/{}",
        base.prefix.trim_end_matches('/'),
        universe_id
    );
    scoped
}

pub(crate) fn open_hosted_cas_for_universe(
    paths: &LocalStatePaths,
    blobstore_config: &BlobStoreConfig,
    universe_id: UniverseId,
) -> Result<Arc<HostedCas>, BackendError> {
    let domain_paths = paths.for_universe(universe_id);
    domain_paths
        .ensure_root()
        .map_err(|err| BackendError::Persist(PersistError::backend(err.to_string())))?;
    std::fs::create_dir_all(domain_paths.cache_root()).map_err(|err| {
        BackendError::Persist(PersistError::backend(format!(
            "create node domain cache dir: {err}"
        )))
    })?;
    let local_cas = Arc::new(FsCas::open_with_paths(&domain_paths)?);
    let remote = Arc::new(RemoteCasStore::new(scoped_blobstore_config(
        blobstore_config,
        universe_id,
    ))?);
    Ok(Arc::new(HostedCas::new(local_cas, remote)))
}

#[allow(dead_code)]
fn parse_hash_ref(value: &str) -> Result<aos_cbor::Hash, BackendError> {
    let normalized = if value.starts_with(HASH_PREFIX) {
        value.to_owned()
    } else {
        format!("{HASH_PREFIX}{value}")
    };
    aos_cbor::Hash::from_hex_str(&normalized)
        .map_err(|_| BackendError::InvalidHashRef(value.to_owned()))
}

fn env_or_legacy(primary: &str, legacy: &str) -> Option<String> {
    std::env::var(primary)
        .ok()
        .or_else(|| std::env::var(legacy).ok())
}

pub(crate) fn object_store_backend_err(
    label: impl Into<String>,
) -> impl FnOnce(object_store::Error) -> BackendError {
    let label = label.into();
    move |err| BackendError::Persist(PersistError::backend(format!("{label}: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_node::UniverseId;
    use object_store::memory::InMemory;

    fn test_config() -> BlobStoreConfig {
        BlobStoreConfig {
            bucket: Some("test-bucket".into()),
            endpoint: None,
            region: Some("us-east-1".into()),
            prefix: "test".into(),
            force_path_style: true,
            pack_threshold_bytes: 128,
            pack_target_bytes: 512,
            retained_checkpoints_per_partition: 2,
        }
    }

    fn checkpoint_for(
        created_at_ns: u64,
        universe_id: UniverseId,
        world_id: WorldId,
        snapshot_ref: String,
        world_seq: u64,
    ) -> WorldCheckpointRecord {
        aos_node::WorldCheckpointRef {
            universe_id,
            world_id,
            world_epoch: 1,
            checkpointed_at_ns: created_at_ns,
            world_seq,
            baseline: aos_node::PromotableBaselineRef {
                snapshot_ref,
                snapshot_manifest_ref: None,
                manifest_hash: "manifest".into(),
                universe_id: UniverseId::nil(),
                height: world_seq,
                logical_time_ns: created_at_ns,
                receipt_horizon_height: world_seq,
            },
            journal_cursor: Some(aos_node::WorldJournalCursor::Kafka {
                journal_topic: "aos-journal".into(),
                partition: 0,
                journal_offset: world_seq,
            }),
        }
    }

    #[test]
    fn object_store_blob_meta_store_prunes_old_checkpoint_manifests() {
        let store = Arc::new(InMemory::new());
        let mut backend = ObjectStoreBlobMetaStore::from_store(test_config(), "test-bucket", store);
        let universe_id = UniverseId::from(uuid::Uuid::new_v4());
        let world_id = WorldId::from(uuid::Uuid::new_v4());

        for ts in 1..=4 {
            backend
                .commit_world_checkpoint(checkpoint_for(
                    ts,
                    universe_id,
                    world_id,
                    format!("snapshot-{ts}"),
                    ts,
                ))
                .unwrap();
        }

        let manifests = backend
            .list_object_keys_sync(&world_checkpoint_manifest_prefix(&backend.config, world_id))
            .unwrap();
        assert_eq!(manifests.len(), 2);
        assert!(
            manifests
                .iter()
                .any(|item| item.ends_with("00000000000000000003.cbor"))
        );
        assert!(
            manifests
                .iter()
                .any(|item| item.ends_with("00000000000000000004.cbor"))
        );

        let latest_world = backend.latest_world_checkpoint(world_id).unwrap().unwrap();
        assert_eq!(latest_world.world_id, world_id);
        assert_eq!(latest_world.checkpointed_at_ns, 4);
        assert_eq!(
            latest_world
                .journal_cursor
                .as_ref()
                .expect("world checkpoint cursor"),
            &aos_node::WorldJournalCursor::Kafka {
                journal_topic: "aos-journal".into(),
                partition: 0,
                journal_offset: 4,
            }
        );
    }
}
