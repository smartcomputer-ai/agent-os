use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use aos_cbor::Hash;
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_kernel::KernelConfig;
use aos_node::{
    HostedStore, WorldAdminStore, WorldId, WorldLineage, WorldStore, default_world_handle,
    open_hosted_from_manifest_hash, snapshot_hosted_world,
};
use aos_runtime::{WorldConfig, WorldHost};
use aos_sqlite::{LocalStatePaths, SqliteNodeStore};
use uuid::Uuid;

use crate::bundle::{ImportMode, ImportOutcome, WorldBundle, import_bundle};

pub struct LocalRuntimeContext {
    paths: LocalStatePaths,
    sqlite: Arc<SqliteNodeStore>,
    persistence: Arc<dyn WorldStore>,
    store: Arc<HostedStore>,
}

pub struct BootstrappedLocalWorld {
    pub world_id: WorldId,
    pub host: WorldHost<HostedStore>,
    pub store: Arc<HostedStore>,
}

pub fn local_state_paths(world_root: &Path) -> LocalStatePaths {
    LocalStatePaths::from_world_root(world_root)
}

pub fn reset_local_runtime_state(world_root: &Path) -> Result<LocalStatePaths> {
    let paths = local_state_paths(world_root);
    paths
        .reset_runtime_state()
        .with_context(|| format!("reset local runtime state under {}", paths.root().display()))?;
    Ok(paths)
}

pub fn open_local_runtime(world_root: &Path, reset: bool) -> Result<LocalRuntimeContext> {
    let paths = if reset {
        reset_local_runtime_state(world_root)?
    } else {
        local_state_paths(world_root)
    };
    LocalRuntimeContext::open(paths)
}

impl LocalRuntimeContext {
    pub fn open(paths: LocalStatePaths) -> Result<Self> {
        paths.ensure_root().context("create local state root")?;
        let sqlite = Arc::new(SqliteNodeStore::open_with_paths(&paths).context("open sqlite")?);
        let universe = sqlite.local_universe_id();
        let persistence: Arc<dyn WorldStore> = sqlite.clone();
        let store = Arc::new(HostedStore::new(Arc::clone(&persistence), universe));
        Ok(Self {
            paths,
            sqlite,
            persistence,
            store,
        })
    }

    pub fn paths(&self) -> &LocalStatePaths {
        &self.paths
    }

    pub fn sqlite(&self) -> &Arc<SqliteNodeStore> {
        &self.sqlite
    }

    pub fn hosted_store(&self) -> Arc<HostedStore> {
        self.store.clone()
    }

    pub fn bootstrap_world_bundle(
        &self,
        bundle: WorldBundle,
        world_config: WorldConfig,
        adapter_config: EffectAdapterConfig,
        kernel_config: KernelConfig,
    ) -> Result<BootstrappedLocalWorld> {
        let ImportOutcome::Genesis(imported) =
            import_bundle(self.store.as_ref(), &bundle, ImportMode::Genesis)
                .context("store hosted manifest bundle")?
        else {
            unreachable!("genesis import mode must produce a genesis import");
        };
        let manifest_hash =
            Hash::from_hex_str(&imported.manifest_hash).context("parse hosted manifest hash")?;
        let world_id = WorldId::from(Uuid::new_v4());
        let universe = self.sqlite.local_universe_id();
        self.sqlite.world_prepare_manifest_bootstrap(
            universe,
            world_id,
            manifest_hash,
            default_world_handle(world_id),
            None,
            0,
            WorldLineage::Genesis { created_at_ns: 0 },
        )?;
        let mut host = match open_hosted_from_manifest_hash(
            Arc::clone(&self.persistence),
            universe,
            world_id,
            manifest_hash,
            world_config,
            adapter_config,
            kernel_config,
            None,
        ) {
            Ok(host) => host,
            Err(err) => {
                let _ = self
                    .sqlite
                    .world_drop_manifest_bootstrap(universe, world_id);
                return Err(err).context("open hosted world from manifest");
            }
        };
        if let Err(err) = snapshot_hosted_world(&mut host, &self.persistence, universe, world_id) {
            let _ = self
                .sqlite
                .world_drop_manifest_bootstrap(universe, world_id);
            return Err(err).context("snapshot bootstrapped hosted world");
        }
        let store = host.store_arc();
        Ok(BootstrappedLocalWorld {
            world_id,
            host,
            store,
        })
    }
}
