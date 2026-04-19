use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aos_node::{
    BackendError, LocalStatePaths, PersistError, SecretBindingRecord, SecretVersionRecord,
    UniverseId,
};
use object_store::ObjectStore;
use object_store::ObjectStoreExt;
use object_store::PutPayload;
use object_store::path::Path as ObjectPath;

use crate::blobstore::{BlobStoreConfig, build_object_store, object_store_backend_err, run_async};

#[derive(Debug, thiserror::Error)]
pub enum VaultStoreError {
    #[error(transparent)]
    Persist(#[from] PersistError),
    #[error(transparent)]
    Cbor(#[from] serde_cbor::Error),
    #[error(transparent)]
    LogFirst(#[from] BackendError),
}

#[derive(Debug, Default)]
pub struct EmbeddedVaultStore {
    bindings: BTreeMap<(UniverseId, String), SecretBindingRecord>,
    versions: BTreeMap<(UniverseId, String, u64), SecretVersionRecord>,
}

impl EmbeddedVaultStore {
    pub fn list_bindings(&self, universe_id: UniverseId) -> Vec<SecretBindingRecord> {
        self.bindings
            .iter()
            .filter(|((universe, _), _)| *universe == universe_id)
            .map(|(_, record)| record.clone())
            .collect()
    }

    pub fn get_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Option<SecretBindingRecord> {
        self.bindings
            .get(&(universe_id, binding_id.to_owned()))
            .cloned()
    }

    pub fn put_binding(&mut self, universe_id: UniverseId, record: SecretBindingRecord) {
        self.bindings
            .insert((universe_id, record.binding_id.clone()), record);
    }

    pub fn delete_binding(
        &mut self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Option<SecretBindingRecord> {
        let removed = self.bindings.remove(&(universe_id, binding_id.to_owned()));
        self.versions.retain(|(universe, binding, _), _| {
            !(*universe == universe_id && binding == binding_id)
        });
        removed
    }

    pub fn list_versions(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Vec<SecretVersionRecord> {
        self.versions
            .iter()
            .filter(|((universe, binding, _), _)| *universe == universe_id && binding == binding_id)
            .map(|(_, record)| record.clone())
            .collect()
    }

    pub fn get_version(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Option<SecretVersionRecord> {
        self.versions
            .get(&(universe_id, binding_id.to_owned(), version))
            .cloned()
    }

    pub fn put_version(&mut self, universe_id: UniverseId, record: SecretVersionRecord) {
        self.versions.insert(
            (universe_id, record.binding_id.clone(), record.version),
            record,
        );
    }
}

pub struct ObjectStoreVaultStore {
    config: BlobStoreConfig,
    bucket: String,
    store: Arc<dyn ObjectStore>,
}

impl std::fmt::Debug for ObjectStoreVaultStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectStoreVaultStore")
            .field("bucket", &self.bucket)
            .field("prefix", &self.config.prefix)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct LocalVaultStore {
    root: PathBuf,
}

impl LocalVaultStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, VaultStoreError> {
        let root = root.into();
        fs::create_dir_all(&root)
            .map_err(|err| PersistError::backend(format!("create local vault root: {err}")))?;
        Ok(Self { root })
    }

    pub fn for_paths(paths: &LocalStatePaths) -> Result<Self, VaultStoreError> {
        Self::new(paths.vault_root())
    }

    pub fn list_bindings(
        &self,
        universe_id: UniverseId,
    ) -> Result<Vec<SecretBindingRecord>, VaultStoreError> {
        read_cbor_records(self.binding_dir(universe_id))
    }

    pub fn get_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<Option<SecretBindingRecord>, VaultStoreError> {
        read_optional_cbor(self.binding_path(universe_id, binding_id))
    }

    pub fn put_binding(
        &self,
        universe_id: UniverseId,
        record: &SecretBindingRecord,
    ) -> Result<(), VaultStoreError> {
        write_cbor_atomic(self.binding_path(universe_id, &record.binding_id), record)
    }

    pub fn delete_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<Option<SecretBindingRecord>, VaultStoreError> {
        let path = self.binding_path(universe_id, binding_id);
        let record = read_optional_cbor(&path)?;
        if record.is_none() {
            return Ok(None);
        }
        remove_file_if_exists(&path)?;
        remove_dir_if_exists(self.version_dir(universe_id, binding_id))?;
        Ok(record)
    }

    pub fn list_versions(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<Vec<SecretVersionRecord>, VaultStoreError> {
        read_cbor_records(self.version_dir(universe_id, binding_id))
    }

    pub fn get_version(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<Option<SecretVersionRecord>, VaultStoreError> {
        read_optional_cbor(self.version_path(universe_id, binding_id, version))
    }

    pub fn put_version(
        &self,
        universe_id: UniverseId,
        record: &SecretVersionRecord,
    ) -> Result<(), VaultStoreError> {
        write_cbor_atomic(
            self.version_path(universe_id, &record.binding_id, record.version),
            record,
        )
    }

    fn universe_dir(&self, universe_id: UniverseId) -> PathBuf {
        self.root.join("universes").join(universe_id.to_string())
    }

    fn binding_dir(&self, universe_id: UniverseId) -> PathBuf {
        self.universe_dir(universe_id).join("bindings")
    }

    fn binding_path(&self, universe_id: UniverseId, binding_id: &str) -> PathBuf {
        self.binding_dir(universe_id)
            .join(format!("{}.cbor", encoded_binding_id(binding_id)))
    }

    fn version_dir(&self, universe_id: UniverseId, binding_id: &str) -> PathBuf {
        self.universe_dir(universe_id)
            .join("versions")
            .join(encoded_binding_id(binding_id))
    }

    fn version_path(&self, universe_id: UniverseId, binding_id: &str, version: u64) -> PathBuf {
        self.version_dir(universe_id, binding_id)
            .join(format!("{version:020}.cbor"))
    }
}

impl ObjectStoreVaultStore {
    pub fn new(config: BlobStoreConfig) -> Result<Self, VaultStoreError> {
        let bucket = config
            .bucket
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                PersistError::validation(
                    "AOS_BLOBSTORE_BUCKET must be set for object-store-backed hosted vault",
                )
            })?;
        let store = build_object_store(&config, &bucket)?;
        Ok(Self {
            config,
            bucket,
            store,
        })
    }

    pub fn list_bindings(
        &self,
        universe_id: UniverseId,
    ) -> Result<Vec<SecretBindingRecord>, VaultStoreError> {
        let mut bindings: Vec<SecretBindingRecord> = Vec::new();
        for key in self.list_object_keys_sync(&binding_prefix(&self.config, universe_id))? {
            if let Some(payload) = self.get_object_sync(&key)? {
                bindings.push(serde_cbor::from_slice(&payload)?);
            }
        }
        bindings.sort_by(|left, right| left.binding_id.cmp(&right.binding_id));
        Ok(bindings)
    }

    pub fn get_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<Option<SecretBindingRecord>, VaultStoreError> {
        let key = binding_key(&self.config, universe_id, binding_id);
        let Some(payload) = self.get_object_sync(&key)? else {
            return Ok(None);
        };
        Ok(Some(serde_cbor::from_slice(&payload)?))
    }

    pub fn put_binding(
        &self,
        universe_id: UniverseId,
        record: &SecretBindingRecord,
    ) -> Result<(), VaultStoreError> {
        self.put_object_sync(
            binding_key(&self.config, universe_id, &record.binding_id),
            serde_cbor::to_vec(record)?,
        )
    }

    pub fn delete_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<Option<SecretBindingRecord>, VaultStoreError> {
        let record = self.get_binding(universe_id, binding_id)?;
        if record.is_none() {
            return Ok(None);
        }
        self.delete_object_sync(&binding_key(&self.config, universe_id, binding_id))?;
        for key in
            self.list_object_keys_sync(&version_prefix(&self.config, universe_id, binding_id))?
        {
            self.delete_object_sync(&key)?;
        }
        Ok(record)
    }

    pub fn list_versions(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<Vec<SecretVersionRecord>, VaultStoreError> {
        let mut versions: Vec<SecretVersionRecord> = Vec::new();
        for key in
            self.list_object_keys_sync(&version_prefix(&self.config, universe_id, binding_id))?
        {
            if let Some(payload) = self.get_object_sync(&key)? {
                versions.push(serde_cbor::from_slice(&payload)?);
            }
        }
        versions.sort_by_key(|record| record.version);
        Ok(versions)
    }

    pub fn get_version(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<Option<SecretVersionRecord>, VaultStoreError> {
        let key = version_key(&self.config, universe_id, binding_id, version);
        let Some(payload) = self.get_object_sync(&key)? else {
            return Ok(None);
        };
        Ok(Some(serde_cbor::from_slice(&payload)?))
    }

    pub fn put_version(
        &self,
        universe_id: UniverseId,
        record: &SecretVersionRecord,
    ) -> Result<(), VaultStoreError> {
        self.put_object_sync(
            version_key(
                &self.config,
                universe_id,
                &record.binding_id,
                record.version,
            ),
            serde_cbor::to_vec(record)?,
        )
    }

    fn put_object_sync(&self, key: String, payload: Vec<u8>) -> Result<(), VaultStoreError> {
        let store = Arc::clone(&self.store);
        let bucket = self.bucket.clone();
        let path = ObjectPath::from(key.clone());
        run_async(format!("put object-store://{bucket}/{key}"), async move {
            store
                .put(&path, PutPayload::from(payload))
                .await
                .map(|_| ())
                .map_err(object_store_backend_err("put object"))
        })
        .map_err(Into::into)
    }

    fn get_object_sync(&self, key: &str) -> Result<Option<Vec<u8>>, VaultStoreError> {
        let store = Arc::clone(&self.store);
        let bucket = self.bucket.clone();
        let path = ObjectPath::from(key);
        run_async(format!("get object-store://{bucket}/{key}"), async move {
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
        .map_err(Into::into)
    }

    fn list_object_keys_sync(&self, prefix: &str) -> Result<Vec<String>, VaultStoreError> {
        let store = Arc::clone(&self.store);
        let bucket = self.bucket.clone();
        let path = ObjectPath::from(prefix);
        run_async(
            format!("list object-store://{bucket}/{prefix}"),
            async move {
                let listing = store
                    .list_with_delimiter(Some(&path))
                    .await
                    .map_err(object_store_backend_err("list objects"))?;
                Ok(listing
                    .objects
                    .into_iter()
                    .map(|item| item.location.to_string())
                    .collect())
            },
        )
        .map_err(Into::into)
    }

    fn delete_object_sync(&self, key: &str) -> Result<(), VaultStoreError> {
        let store = Arc::clone(&self.store);
        let bucket = self.bucket.clone();
        let path = ObjectPath::from(key);
        run_async(
            format!("delete object-store://{bucket}/{key}"),
            async move {
                store
                    .delete(&path)
                    .await
                    .map_err(object_store_backend_err("delete object"))
            },
        )
        .map_err(Into::into)
    }
}

#[derive(Debug)]
pub enum VaultBlobstore {
    Embedded(EmbeddedVaultStore),
    Local(LocalVaultStore),
    ObjectStore(ObjectStoreVaultStore),
}

impl VaultBlobstore {
    pub fn new(config: BlobStoreConfig) -> Result<Self, VaultStoreError> {
        if config
            .bucket
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Ok(Self::ObjectStore(ObjectStoreVaultStore::new(config)?));
        }
        Ok(Self::Embedded(EmbeddedVaultStore::default()))
    }

    pub fn new_persistent(
        config: BlobStoreConfig,
        paths: &LocalStatePaths,
    ) -> Result<Self, VaultStoreError> {
        if config
            .bucket
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Ok(Self::ObjectStore(ObjectStoreVaultStore::new(config)?));
        }
        Ok(Self::Local(LocalVaultStore::for_paths(paths)?))
    }

    pub fn list_bindings(
        &self,
        universe_id: UniverseId,
    ) -> Result<Vec<SecretBindingRecord>, VaultStoreError> {
        match self {
            Self::Embedded(inner) => Ok(inner.list_bindings(universe_id)),
            Self::Local(inner) => inner.list_bindings(universe_id),
            Self::ObjectStore(inner) => inner.list_bindings(universe_id),
        }
    }

    pub fn get_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<Option<SecretBindingRecord>, VaultStoreError> {
        match self {
            Self::Embedded(inner) => Ok(inner.get_binding(universe_id, binding_id)),
            Self::Local(inner) => inner.get_binding(universe_id, binding_id),
            Self::ObjectStore(inner) => inner.get_binding(universe_id, binding_id),
        }
    }

    pub fn put_binding(
        &mut self,
        universe_id: UniverseId,
        record: SecretBindingRecord,
    ) -> Result<(), VaultStoreError> {
        match self {
            Self::Embedded(inner) => {
                inner.put_binding(universe_id, record);
                Ok(())
            }
            Self::Local(inner) => inner.put_binding(universe_id, &record),
            Self::ObjectStore(inner) => inner.put_binding(universe_id, &record),
        }
    }

    pub fn delete_binding(
        &mut self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<Option<SecretBindingRecord>, VaultStoreError> {
        match self {
            Self::Embedded(inner) => Ok(inner.delete_binding(universe_id, binding_id)),
            Self::Local(inner) => inner.delete_binding(universe_id, binding_id),
            Self::ObjectStore(inner) => inner.delete_binding(universe_id, binding_id),
        }
    }

    pub fn list_versions(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<Vec<SecretVersionRecord>, VaultStoreError> {
        match self {
            Self::Embedded(inner) => Ok(inner.list_versions(universe_id, binding_id)),
            Self::Local(inner) => inner.list_versions(universe_id, binding_id),
            Self::ObjectStore(inner) => inner.list_versions(universe_id, binding_id),
        }
    }

    pub fn get_version(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<Option<SecretVersionRecord>, VaultStoreError> {
        match self {
            Self::Embedded(inner) => Ok(inner.get_version(universe_id, binding_id, version)),
            Self::Local(inner) => inner.get_version(universe_id, binding_id, version),
            Self::ObjectStore(inner) => inner.get_version(universe_id, binding_id, version),
        }
    }

    pub fn put_version(
        &mut self,
        universe_id: UniverseId,
        record: SecretVersionRecord,
    ) -> Result<(), VaultStoreError> {
        match self {
            Self::Embedded(inner) => {
                inner.put_version(universe_id, record);
                Ok(())
            }
            Self::Local(inner) => inner.put_version(universe_id, &record),
            Self::ObjectStore(inner) => inner.put_version(universe_id, &record),
        }
    }
}

fn binding_prefix(config: &BlobStoreConfig, universe_id: UniverseId) -> String {
    format!(
        "{}/secrets/{}/bindings",
        config.prefix.trim_end_matches('/'),
        universe_id
    )
}

fn binding_key(config: &BlobStoreConfig, universe_id: UniverseId, binding_id: &str) -> String {
    format!(
        "{}/{}.cbor",
        binding_prefix(config, universe_id),
        encoded_binding_id(binding_id)
    )
}

fn version_prefix(config: &BlobStoreConfig, universe_id: UniverseId, binding_id: &str) -> String {
    format!(
        "{}/secrets/{}/versions/{}",
        config.prefix.trim_end_matches('/'),
        universe_id,
        encoded_binding_id(binding_id)
    )
}

fn version_key(
    config: &BlobStoreConfig,
    universe_id: UniverseId,
    binding_id: &str,
    version: u64,
) -> String {
    format!(
        "{}/{}.cbor",
        version_prefix(config, universe_id, binding_id),
        version
    )
}

fn encoded_binding_id(binding_id: &str) -> String {
    hex::encode(binding_id.as_bytes())
}

fn read_cbor_records<T>(dir: PathBuf) -> Result<Vec<T>, VaultStoreError>
where
    T: serde::de::DeserializeOwned,
{
    let mut records = Vec::new();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(records),
        Err(err) => {
            return Err(PersistError::backend(format!(
                "list local vault dir {}: {err}",
                dir.display()
            ))
            .into());
        }
    };
    for entry in entries {
        let entry = entry.map_err(|err| {
            PersistError::backend(format!("read local vault dir {}: {err}", dir.display()))
        })?;
        if !entry
            .file_type()
            .map_err(|err| {
                PersistError::backend(format!(
                    "read local vault entry type {}: {err}",
                    entry.path().display()
                ))
            })?
            .is_file()
        {
            continue;
        }
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("cbor") {
            continue;
        }
        let bytes = fs::read(entry.path()).map_err(|err| {
            PersistError::backend(format!(
                "read local vault record {}: {err}",
                entry.path().display()
            ))
        })?;
        records.push(serde_cbor::from_slice(&bytes)?);
    }
    Ok(records)
}

fn read_optional_cbor<T>(path: impl AsRef<Path>) -> Result<Option<T>, VaultStoreError>
where
    T: serde::de::DeserializeOwned,
{
    let path = path.as_ref();
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(PersistError::backend(format!(
                "read local vault record {}: {err}",
                path.display()
            ))
            .into());
        }
    };
    Ok(Some(serde_cbor::from_slice(&bytes)?))
}

fn write_cbor_atomic<T>(path: impl AsRef<Path>, value: &T) -> Result<(), VaultStoreError>
where
    T: serde::Serialize,
{
    let path = path.as_ref();
    let parent = path
        .parent()
        .ok_or_else(|| PersistError::backend("invalid local vault path"))?;
    fs::create_dir_all(parent)
        .map_err(|err| PersistError::backend(format!("create local vault dir: {err}")))?;
    let bytes = serde_cbor::to_vec(value)?;
    let temp_path = path.with_extension(format!("tmp-{}-{}", std::process::id(), unique_suffix()));
    fs::write(&temp_path, bytes)
        .map_err(|err| PersistError::backend(format!("write local vault temp file: {err}")))?;
    fs::rename(&temp_path, path)
        .map_err(|err| PersistError::backend(format!("rename local vault temp file: {err}")))?;
    Ok(())
}

fn remove_file_if_exists(path: impl AsRef<Path>) -> Result<(), VaultStoreError> {
    let path = path.as_ref();
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(PersistError::backend(format!(
            "remove local vault file {}: {err}",
            path.display()
        ))
        .into()),
    }
}

fn remove_dir_if_exists(path: impl AsRef<Path>) -> Result<(), VaultStoreError> {
    let path = path.as_ref();
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(PersistError::backend(format!(
            "remove local vault dir {}: {err}",
            path.display()
        ))
        .into()),
    }
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
