mod blobstore;
mod control;
mod harness;
mod paths;
mod runtime;
mod secrets;
mod sqlite;
mod store_error;
mod workspace;

pub use blobstore::{FsBlobBackend, FsCas, LocalBlobBackend, LocalBlobStoreConfig};
pub use control::LocalControl;
pub use harness::{EmbeddedHarnessError, EmbeddedWorldHarness};
pub use paths::LocalStatePaths;
pub use runtime::{LocalRuntime, LocalRuntimeError};
pub use sqlite::{LocalSqliteBackend, LocalSqliteConfig};
pub use store_error::LocalStoreError;

pub fn local_universe_id() -> crate::UniverseId {
    crate::UniverseId::nil()
}
