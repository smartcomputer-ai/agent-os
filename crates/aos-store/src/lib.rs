//! Content-addressed storage abstractions plus filesystem and in-memory backends.

mod fs_store;
pub mod manifest;
mod mem_store;

pub use fs_store::FsStore;
pub use manifest::{Catalog, CatalogEntry, load_manifest_from_bytes, load_manifest_from_path};
pub use mem_store::MemStore;

use aos_cbor::Hash;
use serde::{Serialize, de::DeserializeOwned};
use std::{io, path::PathBuf, sync::Arc};

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
    #[error("plan validation failed for '{name}': {source}")]
    PlanValidation {
        name: String,
        #[source]
        source: aos_air_types::validate::ValidationError,
    },
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
