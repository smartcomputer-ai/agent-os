use std::collections::BTreeMap;
use std::sync::Arc;

use aos_node::{PersistError, PlaneError, SecretBindingRecord, SecretVersionRecord, UniverseId};
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
    LogFirst(#[from] PlaneError),
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

    pub fn list_bindings(
        &self,
        universe_id: UniverseId,
    ) -> Result<Vec<SecretBindingRecord>, VaultStoreError> {
        match self {
            Self::Embedded(inner) => Ok(inner.list_bindings(universe_id)),
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
