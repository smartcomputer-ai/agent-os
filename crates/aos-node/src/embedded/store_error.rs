use thiserror::Error;

use crate::PersistError;

#[derive(Debug, Error)]
pub enum LocalStoreError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Persist(#[from] PersistError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Cbor(#[from] serde_cbor::Error),
    #[error("backend error: {0}")]
    Backend(String),
}
