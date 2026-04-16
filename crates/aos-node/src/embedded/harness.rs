use std::path::Path;
use std::sync::Arc;

use aos_cbor::Hash;
use aos_kernel::{LoadedManifest, Store, StoreError, store_loaded_manifest};

use crate::api::ControlError;
use crate::{CreateWorldRequest, CreateWorldSource, FsCas, LocalControl, LocalStatePaths, WorldId};

#[derive(Debug, thiserror::Error)]
pub enum EmbeddedHarnessError {
    #[error(transparent)]
    Control(#[from] ControlError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("harness backend error: {0}")]
    Backend(String),
}

#[derive(Clone)]
pub struct EmbeddedWorldHarness {
    paths: LocalStatePaths,
    control: Arc<LocalControl>,
}

impl EmbeddedWorldHarness {
    pub fn open(state_root: &Path) -> Result<Self, EmbeddedHarnessError> {
        let paths = LocalStatePaths::new(state_root.to_path_buf());
        paths
            .ensure_root()
            .map_err(|err| EmbeddedHarnessError::Backend(err.to_string()))?;
        let control = LocalControl::open_batch(paths.root())?;
        Ok(Self { paths, control })
    }

    pub fn reopen(&self) -> Result<Self, EmbeddedHarnessError> {
        Self::open(self.paths.root())
    }

    pub fn paths(&self) -> &LocalStatePaths {
        &self.paths
    }

    pub fn control(&self) -> &Arc<LocalControl> {
        &self.control
    }

    pub fn into_control(self) -> Arc<LocalControl> {
        self.control
    }

    pub fn install_loaded_manifest<S: Store + ?Sized>(
        &self,
        source: &S,
        loaded: &LoadedManifest,
    ) -> Result<Hash, EmbeddedHarnessError> {
        let cas = FsCas::open_with_paths(&self.paths)
            .map_err(|err| EmbeddedHarnessError::Backend(err.to_string()))?;
        for module in loaded.modules.values() {
            let hash = Hash::from_hex_str(module.wasm_hash.as_str())
                .map_err(|err| EmbeddedHarnessError::Backend(err.to_string()))?;
            let bytes = source.get_blob(hash)?;
            let stored = cas.put_blob(&bytes)?;
            if stored != hash {
                return Err(EmbeddedHarnessError::Backend(
                    "copied wasm blob hash mismatch".into(),
                ));
            }
        }
        store_loaded_manifest(&cas, loaded)
            .map_err(|err| EmbeddedHarnessError::Backend(err.to_string()))
    }

    pub fn create_world_from_loaded_manifest<S: Store + ?Sized>(
        &self,
        source: &S,
        loaded: &LoadedManifest,
        world_id: WorldId,
        created_at_ns: u64,
    ) -> Result<crate::WorldCreateResult, EmbeddedHarnessError> {
        let manifest_hash = self.install_loaded_manifest(source, loaded)?;
        Ok(self.control.create_world(CreateWorldRequest {
            world_id: Some(world_id),
            universe_id: crate::UniverseId::nil(),
            created_at_ns,
            source: CreateWorldSource::Manifest {
                manifest_hash: manifest_hash.to_hex(),
            },
        })?)
    }
}
