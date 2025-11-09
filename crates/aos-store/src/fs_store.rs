use crate::{EntryKind, Store, StoreError, StoreResult, io_error};
use aos_cbor::{Hash, to_canonical_cbor};
use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

/// Filesystem-backed store rooted at `<root>/.store`.
#[derive(Clone)]
pub struct FsStore {
    nodes_dir: PathBuf,
    blobs_dir: PathBuf,
}

impl fmt::Debug for FsStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FsStore")
            .field("nodes_dir", &self.nodes_dir)
            .field("blobs_dir", &self.blobs_dir)
            .finish()
    }
}

impl FsStore {
    pub fn open(root: impl AsRef<Path>) -> StoreResult<Self> {
        let root = root.as_ref();
        let store_root = root.join(".store");
        let nodes_dir = store_root.join("nodes").join("sha256");
        let blobs_dir = store_root.join("blobs").join("sha256");
        fs::create_dir_all(&nodes_dir).map_err(|e| io_error(&nodes_dir, e))?;
        fs::create_dir_all(&blobs_dir).map_err(|e| io_error(&blobs_dir, e))?;
        Ok(Self {
            nodes_dir,
            blobs_dir,
        })
    }

    fn write_once(path: &Path, bytes: &[u8]) -> StoreResult<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        }
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => {
                file.write_all(bytes).map_err(|e| io_error(path, e))?;
                file.sync_all().map_err(|e| io_error(path, e))?;
                Ok(())
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => Ok(()),
            Err(err) => Err(io_error(path, err)),
        }
    }

    fn node_path(&self, hash: &Hash) -> PathBuf {
        self.nodes_dir.join(hex::encode(hash.as_bytes()))
    }

    fn blob_path(&self, hash: &Hash) -> PathBuf {
        self.blobs_dir.join(hex::encode(hash.as_bytes()))
    }

    fn read_entry(&self, kind: EntryKind, hash: Hash) -> StoreResult<Vec<u8>> {
        let path = match kind {
            EntryKind::Node => self.node_path(&hash),
            EntryKind::Blob => self.blob_path(&hash),
        };
        let bytes = fs::read(&path).map_err(|e| io_error(path.clone(), e))?;
        let actual = Hash::of_bytes(&bytes);
        if actual != hash {
            return Err(StoreError::HashMismatch {
                kind,
                expected: hash,
                actual,
            });
        }
        Ok(bytes)
    }
}

impl Store for FsStore {
    fn put_node<T: serde::Serialize>(&self, value: &T) -> StoreResult<Hash> {
        let bytes = to_canonical_cbor(value)?;
        let hash = Hash::of_bytes(&bytes);
        let path = self.node_path(&hash);
        Self::write_once(&path, &bytes)?;
        Ok(hash)
    }

    fn get_node<T: serde::de::DeserializeOwned>(&self, hash: Hash) -> StoreResult<T> {
        let bytes = self.read_entry(EntryKind::Node, hash)?;
        Ok(serde_cbor::from_slice(&bytes)?)
    }

    fn has_node(&self, hash: Hash) -> StoreResult<bool> {
        Ok(self.node_path(&hash).exists())
    }

    fn put_blob(&self, bytes: &[u8]) -> StoreResult<Hash> {
        let hash = Hash::of_bytes(bytes);
        let path = self.blob_path(&hash);
        Self::write_once(&path, bytes)?;
        Ok(hash)
    }

    fn get_blob(&self, hash: Hash) -> StoreResult<Vec<u8>> {
        self.read_entry(EntryKind::Blob, hash)
    }

    fn has_blob(&self, hash: Hash) -> StoreResult<bool> {
        Ok(self.blob_path(&hash).exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Dummy {
        name: String,
        counter: u64,
    }

    #[test]
    fn node_round_trip() {
        let dir = TempDir::new().expect("tmp");
        let store = FsStore::open(dir.path()).expect("open");
        let value = Dummy {
            name: "demo".into(),
            counter: 42,
        };
        let hash = store.put_node(&value).expect("put");
        assert!(store.has_node(hash).expect("has"));
        let loaded: Dummy = store.get_node(hash).expect("get");
        assert_eq!(value, loaded);
    }

    #[test]
    fn blob_round_trip() {
        let dir = TempDir::new().expect("tmp");
        let store = FsStore::open(dir.path()).expect("open");
        let bytes = b"hello world".to_vec();
        let hash = store.put_blob(&bytes).expect("put");
        assert!(store.has_blob(hash).expect("has"));
        let loaded = store.get_blob(hash).expect("get");
        assert_eq!(bytes, loaded);
    }

    #[test]
    fn hash_mismatch_detected() {
        let dir = TempDir::new().expect("tmp");
        let store = FsStore::open(dir.path()).expect("open");
        let hash = store.put_blob(b"original").expect("put");
        let path = store.blob_path(&hash);
        std::fs::write(&path, b"tampered").expect("tamper");
        let err = store.get_blob(hash).expect_err("should fail");
        match err {
            StoreError::HashMismatch { kind, .. } => assert_eq!(kind, EntryKind::Blob),
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
