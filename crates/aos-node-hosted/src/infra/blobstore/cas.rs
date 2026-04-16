use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use aos_cbor::Hash;
use aos_kernel::{MemStore, Store, StoreError, StoreResult};
use aos_node::{BackendError, BlobBackend, FsCas, PersistError, UniverseId};
use object_store::ObjectStore;
use object_store::ObjectStoreExt;
use object_store::PutPayload;
use object_store::path::Path as ObjectPath;
use serde::{Deserialize, Serialize};

use super::{BlobStoreConfig, build_object_store, object_store_backend_err, run_async};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CasRootRecord {
    logical_hash: String,
    size_bytes: u64,
    layout: BlobLayout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum BlobLayout {
    Direct {
        object_key: String,
    },
    Packed {
        pack_key: String,
        offset: u64,
        stored_len: u64,
    },
}

#[derive(Debug, Clone)]
pub struct EmbeddedRemoteCasStore {
    store: Arc<MemStore>,
}

impl EmbeddedRemoteCasStore {
    pub fn new() -> Self {
        Self {
            store: Arc::new(MemStore::new()),
        }
    }

    #[cfg(test)]
    pub fn from_store(store: Arc<MemStore>) -> Self {
        Self { store }
    }

    fn put_cas_blob(&self, bytes: &[u8]) -> Result<Hash, BackendError> {
        Ok(self.store.put_blob(bytes)?)
    }

    fn get_cas_blob(&self, hash: Hash) -> Result<Vec<u8>, BackendError> {
        Ok(self.store.get_blob(hash)?)
    }

    fn has_cas_blob(&self, hash: Hash) -> Result<bool, BackendError> {
        Ok(self.store.has_blob(hash)?)
    }
}

pub struct ObjectStoreRemoteCasStore {
    config: BlobStoreConfig,
    bucket: String,
    store: Arc<dyn ObjectStore>,
}

impl ObjectStoreRemoteCasStore {
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
        })
    }

    #[cfg(test)]
    pub fn from_store(
        config: BlobStoreConfig,
        bucket: impl Into<String>,
        store: Arc<dyn ObjectStore>,
    ) -> Self {
        Self {
            config,
            bucket: bucket.into(),
            store,
        }
    }

    pub fn config(&self) -> &BlobStoreConfig {
        &self.config
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

    fn get_object_range_sync(&self, key: &str, range: Range<u64>) -> Result<Vec<u8>, BackendError> {
        let store = Arc::clone(&self.store);
        let path = ObjectPath::from(key);
        let label = format!(
            "get object-store://{}/{} range {}..{}",
            self.bucket, key, range.start, range.end
        );
        run_async(label, async move {
            store
                .get_range(&path, range)
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(object_store_backend_err("get object range"))
        })
    }

    fn load_cas_root(&self, hash: Hash) -> Result<Option<CasRootRecord>, BackendError> {
        let key = cas_root_key(&self.config, hash);
        self.get_object_sync(&key)?
            .map(|payload| serde_cbor::from_slice(&payload).map_err(BackendError::from))
            .transpose()
    }

    fn store_cas_root(&self, hash: Hash, record: &CasRootRecord) -> Result<(), BackendError> {
        let key = cas_root_key(&self.config, hash);
        self.put_object_sync(key, serde_cbor::to_vec(record)?)
    }

    fn write_direct_blob(&self, hash: Hash, bytes: Vec<u8>) -> Result<(), BackendError> {
        let object_key = direct_blob_key(&self.config, hash);
        self.put_object_sync(object_key.clone(), bytes.clone())?;
        self.store_cas_root(
            hash,
            &CasRootRecord {
                logical_hash: hash.to_hex(),
                size_bytes: bytes.len() as u64,
                layout: BlobLayout::Direct { object_key },
            },
        )
    }

    fn write_packed_blob_group(&self, blobs: Vec<(Hash, Vec<u8>)>) -> Result<(), BackendError> {
        if blobs.is_empty() {
            return Ok(());
        }

        let mut current = Vec::new();
        let mut current_size = 0usize;
        for (hash, bytes) in blobs {
            let next_size = current_size.saturating_add(bytes.len());
            if !current.is_empty() && next_size > self.config.pack_target_bytes {
                self.flush_pack(std::mem::take(&mut current))?;
                current_size = 0;
            }
            current_size = current_size.saturating_add(bytes.len());
            current.push((hash, bytes));
        }
        self.flush_pack(current)
    }

    fn flush_pack(&self, blobs: Vec<(Hash, Vec<u8>)>) -> Result<(), BackendError> {
        if blobs.is_empty() {
            return Ok(());
        }

        let mut pack = Vec::new();
        let mut layouts = Vec::with_capacity(blobs.len());
        for (hash, bytes) in blobs {
            let offset = pack.len() as u64;
            let stored_len = bytes.len() as u64;
            pack.extend_from_slice(&bytes);
            layouts.push((hash, stored_len, offset));
        }

        let pack_hash = Hash::of_bytes(&pack);
        let pack_key = pack_blob_key(&self.config, pack_hash);
        self.put_object_sync(pack_key.clone(), pack)?;
        for (hash, stored_len, offset) in layouts {
            self.store_cas_root(
                hash,
                &CasRootRecord {
                    logical_hash: hash.to_hex(),
                    size_bytes: stored_len,
                    layout: BlobLayout::Packed {
                        pack_key: pack_key.clone(),
                        offset,
                        stored_len,
                    },
                },
            )?;
        }
        Ok(())
    }

    fn put_cas_blob(&self, bytes: &[u8]) -> Result<Hash, BackendError> {
        let hash = Hash::of_bytes(bytes);
        if bytes.len() <= self.config.pack_threshold_bytes {
            self.write_packed_blob_group(vec![(hash, bytes.to_vec())])?;
        } else {
            self.write_direct_blob(hash, bytes.to_vec())?;
        }
        Ok(hash)
    }

    fn get_cas_blob(&self, hash: Hash) -> Result<Vec<u8>, BackendError> {
        let bytes = match self.load_cas_root(hash)? {
            Some(root) => match root.layout {
                BlobLayout::Direct { object_key } => {
                    self.get_object_sync(&object_key)?.ok_or_else(|| {
                        BackendError::Persist(PersistError::not_found(format!(
                            "blob object-store://{}/{}",
                            self.bucket, object_key
                        )))
                    })?
                }
                BlobLayout::Packed {
                    pack_key,
                    offset,
                    stored_len,
                } => self.get_object_range_sync(&pack_key, offset..offset + stored_len)?,
            },
            None => {
                let key = direct_blob_key(&self.config, hash);
                self.get_object_sync(&key)?.ok_or_else(|| {
                    BackendError::Persist(PersistError::not_found(format!(
                        "blob object-store://{}/{key}",
                        self.bucket
                    )))
                })?
            }
        };
        let actual = Hash::of_bytes(&bytes);
        if actual != hash {
            return Err(BackendError::Persist(PersistError::backend(format!(
                "blob hash mismatch after read: expected {}, got {}",
                hash.to_hex(),
                actual.to_hex()
            ))));
        }
        Ok(bytes)
    }

    fn has_cas_blob(&self, hash: Hash) -> Result<bool, BackendError> {
        if self.load_cas_root(hash)?.is_some() {
            return Ok(true);
        }
        let key = direct_blob_key(&self.config, hash);
        Ok(self.get_object_sync(&key)?.is_some())
    }
}

impl std::fmt::Debug for ObjectStoreRemoteCasStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectStoreRemoteCasStore")
            .field("config", &self.config)
            .field("bucket", &self.bucket)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub enum RemoteCasStore {
    Embedded(EmbeddedRemoteCasStore),
    ObjectStore(Arc<ObjectStoreRemoteCasStore>),
}

impl RemoteCasStore {
    pub fn new(config: BlobStoreConfig) -> Result<Self, BackendError> {
        if config
            .bucket
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Ok(Self::ObjectStore(Arc::new(ObjectStoreRemoteCasStore::new(
                config,
            )?)));
        }
        Ok(Self::Embedded(EmbeddedRemoteCasStore::new()))
    }

    pub fn new_embedded(_config: BlobStoreConfig) -> Self {
        Self::Embedded(EmbeddedRemoteCasStore::new())
    }

    pub fn is_external(&self) -> bool {
        matches!(self, Self::ObjectStore(_))
    }

    pub fn put_cas_blob(&self, bytes: &[u8]) -> Result<Hash, BackendError> {
        match self {
            Self::Embedded(inner) => inner.put_cas_blob(bytes),
            Self::ObjectStore(inner) => inner.put_cas_blob(bytes),
        }
    }

    pub fn get_cas_blob(&self, hash: Hash) -> Result<Vec<u8>, BackendError> {
        match self {
            Self::Embedded(inner) => inner.get_cas_blob(hash),
            Self::ObjectStore(inner) => inner.get_cas_blob(hash),
        }
    }

    pub fn has_cas_blob(&self, hash: Hash) -> Result<bool, BackendError> {
        match self {
            Self::Embedded(inner) => inner.has_cas_blob(hash),
            Self::ObjectStore(inner) => inner.has_cas_blob(hash),
        }
    }
}

impl BlobBackend for RemoteCasStore {
    fn put_blob(&self, _universe_id: UniverseId, bytes: &[u8]) -> Result<Hash, BackendError> {
        self.put_cas_blob(bytes)
    }

    fn get_blob(&self, _universe_id: UniverseId, hash: Hash) -> Result<Vec<u8>, BackendError> {
        self.get_cas_blob(hash)
    }

    fn has_blob(&self, _universe_id: UniverseId, hash: Hash) -> Result<bool, BackendError> {
        self.has_cas_blob(hash)
    }
}

#[derive(Debug, Clone)]
pub struct HostedCas {
    local: Arc<FsCas>,
    remote: Arc<RemoteCasStore>,
}

impl HostedCas {
    pub fn new(local: Arc<FsCas>, remote: Arc<RemoteCasStore>) -> Self {
        Self { local, remote }
    }

    pub fn local_cache(&self) -> Arc<FsCas> {
        Arc::clone(&self.local)
    }

    pub fn put_verified(&self, bytes: &[u8]) -> Result<Hash, PersistError> {
        let local_hash = self.local.put_verified(bytes)?;
        let remote_hash = self
            .remote
            .put_cas_blob(bytes)
            .map_err(backend_to_persist_error)?;
        if remote_hash != local_hash {
            return Err(PersistError::backend(format!(
                "hosted CAS write hash mismatch: local {}, remote {}",
                local_hash.to_hex(),
                remote_hash.to_hex()
            )));
        }
        Ok(local_hash)
    }

    pub fn has(&self, hash: Hash) -> bool {
        self.local.has(hash)
    }

    pub fn get(&self, hash: Hash) -> Result<Vec<u8>, PersistError> {
        if self.local.has(hash) {
            return self.local.get(hash);
        }
        let bytes = self
            .remote
            .get_cas_blob(hash)
            .map_err(backend_to_persist_error)?;
        let stored = self.local.put_verified(&bytes)?;
        if stored != hash {
            return Err(PersistError::backend(format!(
                "hosted CAS hydrate hash mismatch: expected {}, got {}",
                hash.to_hex(),
                stored.to_hex()
            )));
        }
        Ok(bytes)
    }

    pub fn all_hashes(&self) -> Result<Vec<Hash>, PersistError> {
        self.local.all_hashes()
    }
}

impl Store for HostedCas {
    fn put_node<T: serde::Serialize>(&self, value: &T) -> StoreResult<Hash> {
        let bytes = aos_cbor::to_canonical_cbor(value)?;
        self.put_blob(&bytes)
    }

    fn get_node<T: serde::de::DeserializeOwned>(&self, hash: Hash) -> StoreResult<T> {
        let bytes = self.get_blob(hash)?;
        serde_cbor::from_slice(&bytes).map_err(StoreError::from)
    }

    fn has_node(&self, hash: Hash) -> StoreResult<bool> {
        self.has_blob(hash)
    }

    fn put_blob(&self, bytes: &[u8]) -> StoreResult<Hash> {
        self.put_verified(bytes)
            .map_err(persist_error_to_store_error)
    }

    fn get_blob(&self, hash: Hash) -> StoreResult<Vec<u8>> {
        self.get(hash).map_err(persist_error_to_store_error)
    }

    fn has_blob(&self, hash: Hash) -> StoreResult<bool> {
        if self.local.has(hash) {
            return Ok(true);
        }
        self.remote
            .has_cas_blob(hash)
            .map_err(backend_to_store_error)
    }
}

fn cas_root_key(config: &BlobStoreConfig, hash: Hash) -> String {
    format!(
        "{}/cas/{}.cbor",
        config.prefix.trim_end_matches('/'),
        hash.to_hex()
    )
}

fn direct_blob_key(config: &BlobStoreConfig, hash: Hash) -> String {
    format!(
        "{}/blobs/{}",
        config.prefix.trim_end_matches('/'),
        hash.to_hex()
    )
}

fn pack_blob_key(config: &BlobStoreConfig, pack_hash: Hash) -> String {
    format!(
        "{}/packs/{}.bin",
        config.prefix.trim_end_matches('/'),
        pack_hash.to_hex()
    )
}

fn backend_to_persist_error(err: BackendError) -> PersistError {
    match err {
        BackendError::Persist(err) => err,
        other => PersistError::backend(other.to_string()),
    }
}

fn backend_to_store_error(err: BackendError) -> StoreError {
    persist_error_to_store_error(backend_to_persist_error(err))
}

fn persist_error_to_store_error(err: PersistError) -> StoreError {
    match err {
        PersistError::NotFound(message) => StoreError::Io {
            path: PathBuf::from(message),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "hosted CAS entry missing"),
        },
        PersistError::Backend(message) => StoreError::Io {
            path: PathBuf::from(".aos/cas"),
            source: std::io::Error::other(message),
        },
        PersistError::Conflict(message) => StoreError::Io {
            path: PathBuf::from(".aos/cas"),
            source: std::io::Error::new(std::io::ErrorKind::AlreadyExists, message),
        },
        PersistError::Validation(message) => StoreError::Io {
            path: PathBuf::from(".aos/cas"),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, message),
        },
        PersistError::Corrupt(err) => StoreError::Io {
            path: PathBuf::from(".aos/cas"),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn object_store_remote_cas_packs_small_blobs_and_restores_by_range() {
        let store = Arc::new(InMemory::new());
        let cas = ObjectStoreRemoteCasStore::from_store(test_config(), "test-bucket", store);
        let blob_a = b"alpha snapshot bytes".to_vec();
        let blob_b = b"beta snapshot bytes".to_vec();

        let hash_a = cas.put_cas_blob(&blob_a).unwrap();
        let hash_b = cas.put_cas_blob(&blob_b).unwrap();

        assert_eq!(cas.get_cas_blob(hash_a).unwrap(), blob_a);
        assert_eq!(cas.get_cas_blob(hash_b).unwrap(), blob_b);
    }

    #[test]
    fn hosted_cas_hydrates_local_cache_from_remote() {
        let local_root = tempfile::tempdir().unwrap();
        let local = Arc::new(FsCas::open_cas_root(local_root.path()).unwrap());
        let remote = Arc::new(RemoteCasStore::Embedded(EmbeddedRemoteCasStore::new()));
        let hosted = HostedCas::new(Arc::clone(&local), Arc::clone(&remote));
        let bytes = b"hello hosted cas";
        let hash = remote.put_cas_blob(bytes).unwrap();

        assert!(!local.has(hash));
        assert_eq!(hosted.get(hash).unwrap(), bytes);
        assert!(local.has(hash));
    }
}
