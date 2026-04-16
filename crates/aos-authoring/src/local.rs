use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use aos_kernel::KernelConfig;
use aos_node::LocalStatePaths;

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

pub fn local_kernel_config(world_root: &Path) -> Result<KernelConfig> {
    let cache_dir = local_state_paths(world_root).wasmtime_cache_dir();
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;
    Ok(KernelConfig {
        module_cache_dir: Some(cache_dir),
        eager_module_load: true,
        secret_resolver: None,
        allow_placeholder_secrets: false,
        cell_cache_size: aos_kernel::world::DEFAULT_CELL_CACHE_SIZE,
        universe_id: uuid::Uuid::nil(),
    })
}
