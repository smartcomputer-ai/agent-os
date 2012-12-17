//! CLI command handlers.

pub mod event;
pub mod defs;
pub mod gov;
pub mod head;
pub mod info;
pub mod init;
pub mod manifest;
pub mod blob;
pub mod replay;
pub mod run;
pub mod stop;
pub mod snapshot;
pub mod state;

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_host::control::ControlClient;
use aos_host::host::WorldHost;
use aos_host::manifest_loader;
use aos_host::util::has_placeholder_modules;
use aos_kernel::LoadedManifest;
use aos_store::FsStore;

use crate::opts::{Mode, ResolvedDirs, WorldOpts};
use crate::util;

/// Try to connect to a running daemon via control socket.
pub async fn try_control_client(dirs: &ResolvedDirs) -> Option<ControlClient> {
    let socket_path = &dirs.control_socket;
    if socket_path.exists() {
        ControlClient::connect(&socket_path).await.ok()
    } else {
        None
    }
}

/// Decide whether to attempt control based on mode selection.
pub fn should_use_control(opts: &WorldOpts) -> bool {
    matches!(opts.mode, Mode::Auto | Mode::Daemon)
}

/// Prepare the world for running: compile reducer, load manifest, patch modules.
///
/// Returns the store and loaded manifest ready to create a WorldHost.
pub fn prepare_world(
    dirs: &ResolvedDirs,
    opts: &WorldOpts,
) -> Result<(Arc<FsStore>, LoadedManifest)> {
    // Validate world directory
    if !dirs.world.exists() {
        anyhow::bail!("world directory '{}' not found", dirs.world.display());
    }
    if !dirs.world.is_dir() {
        anyhow::bail!("'{}' is not a directory", dirs.world.display());
    }

    // Load world-specific .env
    util::load_world_env(&dirs.world)?;

    // Open store
    let store = Arc::new(FsStore::open(&dirs.store_root).context("open store")?);

    // Compile reducer if present
    let wasm_hash = if dirs.reducer_dir.exists() {
        println!("Compiling reducer from {}...", dirs.reducer_dir.display());
        let hash = util::compile_reducer(
            &dirs.reducer_dir,
            &dirs.store_root,
            &store,
            opts.force_build,
        )?;
        println!("Reducer compiled: {}", hash.as_str());
        Some(hash)
    } else {
        None
    };

    // Load manifest from AIR assets
    let mut loaded = manifest_loader::load_from_assets(store.clone(), &dirs.air_dir)
        .context("load manifest from assets")?
        .ok_or_else(|| anyhow!("no manifest found in {}", dirs.air_dir.display()))?;

    // Patch module hashes
    if let Some(hash) = &wasm_hash {
        let patched = util::patch_module_hashes(&mut loaded, hash, opts.module.as_deref())?;
        if patched > 0 {
            println!("Patched {} module(s) with WASM hash", patched);
        }
    } else if has_placeholder_modules(&loaded) {
        anyhow::bail!(
            "manifest has modules with placeholder hashes but no reducer/ found; \
             use --reducer to specify reducer crate"
        );
    }

    Ok((store, loaded))
}

/// Create a WorldHost from prepared world data.
pub fn create_host(
    store: Arc<FsStore>,
    loaded: LoadedManifest,
    dirs: &ResolvedDirs,
    opts: &WorldOpts,
) -> Result<WorldHost<FsStore>> {
    let host_config = util::host_config_from_env_and_overrides(
        opts.http_timeout_ms,
        opts.http_max_body_bytes,
        opts.no_llm,
    );
    let kernel_config = util::make_kernel_config(&dirs.store_root)?;
    WorldHost::from_loaded_manifest(store, loaded, &dirs.store_root, host_config, kernel_config)
        .context("create world host")
}
