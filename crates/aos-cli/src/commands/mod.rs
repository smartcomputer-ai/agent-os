//! CLI command handlers.

pub mod blob;
pub mod cells;
pub mod defs;
pub mod event;
pub mod gov;
pub mod head;
pub mod info;
pub mod init;
pub mod journal_tail;
pub mod manifest;
pub mod plans;
pub mod pull;
pub mod push;
pub mod replay;
pub mod run;
pub mod snapshot;
pub mod state;
pub mod stop;
pub mod sync;
pub mod trace;
pub mod trace_diagnose;
pub mod trace_find;
pub mod trace_summary;
pub mod ui;
pub mod workspace;
pub mod workspace_sync;

use std::sync::Arc;

use anyhow::{Context, Result};
use aos_host::control::ControlClient;
use aos_host::host::WorldHost;
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

/// Prepare the world for running: load manifest from journal/CAS.
///
/// Returns the store and loaded manifest ready to create a WorldHost.
pub fn prepare_world(
    dirs: &ResolvedDirs,
    _opts: &WorldOpts,
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

    let Some(manifest_hash) = util::latest_manifest_hash_from_journal(&dirs.store_root)? else {
        anyhow::bail!("no manifest found in journal; run `aos push` to seed the world");
    };

    let loaded = aos_kernel::ManifestLoader::load_from_hash(store.as_ref(), manifest_hash)
        .context("load manifest from CAS")?;

    Ok((store, loaded))
}

/// Create a WorldHost from prepared world data.
pub fn create_host(
    store: Arc<FsStore>,
    loaded: LoadedManifest,
    dirs: &ResolvedDirs,
    opts: &WorldOpts,
) -> Result<WorldHost<FsStore>> {
    let host_config = util::host_config_from_opts(opts.http_timeout_ms, opts.http_max_body_bytes);
    let kernel_config = util::make_kernel_config(&dirs.store_root)?;
    WorldHost::from_loaded_manifest(store, loaded, &dirs.store_root, host_config, kernel_config)
        .context("create world host")
}
