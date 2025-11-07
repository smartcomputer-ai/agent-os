use crate::{EntryKind, Store, StoreError, StoreResult};
use aos_cbor::{to_canonical_cbor, Hash};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    collections::HashMap,
    io,
    io::ErrorKind,
    path::PathBuf,
    sync::{Arc, RwLock},
};

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

    fn load_bytes(map: &RwLock<HashMap<Hash, Vec<u8>>>, kind: EntryKind, hash: Hash) -> StoreResult<Vec<u8>> {
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
            return Err(StoreError::HashMismatch { kind: EntryKind::Node, expected: hash, actual });
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
            return Err(StoreError::HashMismatch { kind: EntryKind::Blob, expected: hash, actual });
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
