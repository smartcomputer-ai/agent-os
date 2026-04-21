use aos_cbor::{Hash, to_canonical_cbor};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    io,
    io::ErrorKind,
    path::PathBuf,
    sync::{Arc, RwLock},
};

pub type StoreResult<T> = Result<T, StoreError>;
pub type DynStore = Arc<dyn Store>;

/// Trait implemented by all content-addressed stores.
pub trait Store: Send + Sync {
    fn put_node<T: Serialize>(&self, value: &T) -> StoreResult<Hash>;
    fn get_node<T: DeserializeOwned>(&self, hash: Hash) -> StoreResult<T>;
    fn has_node(&self, hash: Hash) -> StoreResult<bool>;

    fn put_blob(&self, bytes: &[u8]) -> StoreResult<Hash>;
    fn get_blob(&self, hash: Hash) -> StoreResult<Vec<u8>>;
    fn has_blob(&self, hash: Hash) -> StoreResult<bool>;
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("I/O error at {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("CBOR serialization error: {0}")]
    Cbor(#[from] serde_cbor::Error),
    #[error("hash mismatch for {kind:?}: expected {expected}, got {actual}")]
    HashMismatch {
        kind: EntryKind,
        expected: Hash,
        actual: Hash,
    },
    #[error("invalid hash string '{value}': {source}")]
    InvalidHashString {
        value: String,
        #[source]
        source: aos_cbor::HashParseError,
    },
    #[error("node '{name}' is not a {expected}")]
    NodeKindMismatch {
        name: String,
        expected: &'static str,
    },
    #[error("secret {alias}@{version} has invalid version (must be >=1) (context: {context})")]
    InvalidSecretVersion {
        alias: String,
        version: u64,
        context: String,
    },
    #[error("manifest declares duplicate secret {alias}@{version}")]
    DuplicateSecret { alias: String, version: u64 },
    #[error("secret {alias}@{version} missing binding_id")]
    SecretMissingBinding { alias: String, version: u64 },
    #[error("secret {alias}@{version} not declared (context: {context})")]
    UnknownSecret {
        alias: String,
        version: u64,
        context: String,
    },
    #[error("secret name '{name}' is invalid: {reason}")]
    InvalidSecretName { name: String, reason: String },
    #[error("reserved sys/* name '{name}' is not allowed for {kind}")]
    ReservedSysName { kind: &'static str, name: String },
    #[error("manifest declares unsupported air_version '{found}' (supported: {supported})")]
    UnsupportedAirVersion { found: String, supported: String },
    #[error("manifest must declare air_version (supported: {supported})")]
    MissingAirVersion { supported: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Node,
    Blob,
}

pub(crate) fn io_error(path: impl Into<PathBuf>, err: io::Error) -> StoreError {
    StoreError::Io {
        path: path.into(),
        source: err,
    }
}

#[derive(Clone, Default)]
pub struct MemStore {
    nodes: Arc<RwLock<HashMap<Hash, Vec<u8>>>>,
    blobs: Arc<RwLock<HashMap<Hash, Vec<u8>>>>,
}

impl std::fmt::Debug for MemStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemStore")
            .field("nodes", &self.nodes.read().unwrap().len())
            .field("blobs", &self.blobs.read().unwrap().len())
            .finish()
    }
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn missing(kind: EntryKind, hash: Hash) -> StoreError {
        let path = PathBuf::from(format!("memory://{:?}/{}", kind, hash.to_hex()));
        StoreError::Io {
            path,
            source: io::Error::new(ErrorKind::NotFound, "entry not found"),
        }
    }

    fn load_bytes(
        map: &RwLock<HashMap<Hash, Vec<u8>>>,
        kind: EntryKind,
        hash: Hash,
    ) -> StoreResult<Vec<u8>> {
        let guard = map.read().unwrap();
        guard
            .get(&hash)
            .cloned()
            .ok_or_else(|| Self::missing(kind, hash))
    }

    fn insert_if_absent(map: &RwLock<HashMap<Hash, Vec<u8>>>, hash: Hash, bytes: Vec<u8>) {
        let mut guard = map.write().unwrap();
        guard.entry(hash).or_insert(bytes);
    }
}

impl Store for MemStore {
    fn put_node<T: Serialize>(&self, value: &T) -> StoreResult<Hash> {
        let bytes = to_canonical_cbor(value)?;
        let hash = Hash::of_bytes(&bytes);
        Self::insert_if_absent(&self.nodes, hash, bytes);
        Ok(hash)
    }

    fn get_node<T: DeserializeOwned>(&self, hash: Hash) -> StoreResult<T> {
        let bytes = Self::load_bytes(&self.nodes, EntryKind::Node, hash)?;
        let actual = Hash::of_bytes(&bytes);
        if actual != hash {
            return Err(StoreError::HashMismatch {
                kind: EntryKind::Node,
                expected: hash,
                actual,
            });
        }
        Ok(serde_cbor::from_slice(&bytes)?)
    }

    fn has_node(&self, hash: Hash) -> StoreResult<bool> {
        Ok(self.nodes.read().unwrap().contains_key(&hash))
    }

    fn put_blob(&self, bytes: &[u8]) -> StoreResult<Hash> {
        let hash = Hash::of_bytes(bytes);
        Self::insert_if_absent(&self.blobs, hash, bytes.to_vec());
        Ok(hash)
    }

    fn get_blob(&self, hash: Hash) -> StoreResult<Vec<u8>> {
        let bytes = Self::load_bytes(&self.blobs, EntryKind::Blob, hash)?;
        let actual = Hash::of_bytes(&bytes);
        if actual != hash {
            return Err(StoreError::HashMismatch {
                kind: EntryKind::Blob,
                expected: hash,
                actual,
            });
        }
        Ok(bytes)
    }

    fn has_blob(&self, hash: Hash) -> StoreResult<bool> {
        Ok(self.blobs.read().unwrap().contains_key(&hash))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Dummy {
        name: String,
        counter: u64,
    }

    #[test]
    fn node_round_trip() {
        let store = MemStore::new();
        let value = Dummy {
            name: "demo".into(),
            counter: 7,
        };
        let hash = store.put_node(&value).expect("put");
        assert!(store.has_node(hash).expect("has"));
        let loaded: Dummy = store.get_node(hash).expect("get");
        assert_eq!(value, loaded);
    }

    #[test]
    fn blob_round_trip() {
        let store = MemStore::new();
        let bytes = b"hola".to_vec();
        let hash = store.put_blob(&bytes).expect("put");
        assert!(store.has_blob(hash).expect("has"));
        let loaded = store.get_blob(hash).expect("get");
        assert_eq!(bytes, loaded);
    }
}
