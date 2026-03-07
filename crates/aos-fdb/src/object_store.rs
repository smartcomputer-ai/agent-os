use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::protocol::PersistError;

pub trait BlobObjectStore: Send + Sync {
    fn put_if_absent(&self, object_key: &str, bytes: &[u8]) -> Result<(), PersistError>;
    fn get(&self, object_key: &str) -> Result<Vec<u8>, PersistError>;
    fn exists(&self, object_key: &str) -> Result<bool, PersistError>;
}

pub type DynBlobObjectStore = Arc<dyn BlobObjectStore>;

#[derive(Debug, Clone)]
pub struct FilesystemObjectStore {
    root: PathBuf,
}

impl FilesystemObjectStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, PersistError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root).map_err(|err| {
            PersistError::backend(format!(
                "create object store root {}: {err}",
                root.display()
            ))
        })?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn object_path(&self, object_key: &str) -> PathBuf {
        self.root.join(object_key)
    }
}

pub fn filesystem_object_store(root: impl AsRef<Path>) -> Result<DynBlobObjectStore, PersistError> {
    Ok(Arc::new(FilesystemObjectStore::open(root)?))
}

impl BlobObjectStore for FilesystemObjectStore {
    fn put_if_absent(&self, object_key: &str, bytes: &[u8]) -> Result<(), PersistError> {
        let path = self.object_path(object_key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                PersistError::backend(format!("create object parent {}: {err}", parent.display()))
            })?;
        }
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(bytes).map_err(|err| {
                    PersistError::backend(format!("write object {}: {err}", path.display()))
                })?;
                file.sync_all().map_err(|err| {
                    PersistError::backend(format!("sync object {}: {err}", path.display()))
                })?;
                Ok(())
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => Ok(()),
            Err(err) => Err(PersistError::backend(format!(
                "open object {}: {err}",
                path.display()
            ))),
        }
    }

    fn get(&self, object_key: &str) -> Result<Vec<u8>, PersistError> {
        let path = self.object_path(object_key);
        fs::read(&path).map_err(|err| match err.kind() {
            ErrorKind::NotFound => PersistError::not_found(format!("object body {object_key}")),
            _ => PersistError::backend(format!("read object {}: {err}", path.display())),
        })
    }

    fn exists(&self, object_key: &str) -> Result<bool, PersistError> {
        let path = self.object_path(object_key);
        match fs::metadata(&path) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
            Err(err) => Err(PersistError::backend(format!(
                "stat object {}: {err}",
                path.display()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_object_store_put_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = FilesystemObjectStore::open(dir.path()).unwrap();

        store.put_if_absent("cas/u/hash", b"first").unwrap();
        store.put_if_absent("cas/u/hash", b"second").unwrap();

        assert_eq!(store.get("cas/u/hash").unwrap(), b"first");
        assert!(store.exists("cas/u/hash").unwrap());
    }

    #[test]
    fn filesystem_object_store_missing_get_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = FilesystemObjectStore::open(dir.path()).unwrap();

        let err = store.get("missing/key").unwrap_err();
        assert!(matches!(err, PersistError::NotFound(_)));
    }
}
