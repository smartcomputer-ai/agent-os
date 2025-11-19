use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("cargo not found: {0}")]
    CargoNotFound(String),
    #[error("build process failed: {0}")]
    BuildFailed(String),
    #[error("wasm artifact not found in {0:?}")]
    ArtifactNotFound(PathBuf),
}
