//! Global CLI options and world resolution.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use clap::ValueEnum;

/// Global options for CLI commands.
///
/// These options apply to all commands and can be set via env vars.
#[derive(Args, Debug, Clone)]
pub struct WorldOpts {
    /// World directory (env: AOS_WORLD)
    #[arg(short = 'w', long, global = true, env = "AOS_WORLD")]
    pub world: Option<PathBuf>,

    /// Mode selection: auto prefers daemon when available
    #[arg(long, value_enum, default_value_t = Mode::Auto, global = true, env = "AOS_MODE")]
    pub mode: Mode,

    /// Control socket override (env: AOS_CONTROL)
    #[arg(long, global = true, env = "AOS_CONTROL")]
    pub control: Option<PathBuf>,

    /// JSON output envelope
    #[arg(long, global = true)]
    pub json: bool,

    /// Pretty-print JSON output (implies --json)
    #[arg(long, global = true)]
    pub pretty: bool,

    /// Suppress notices (e.g., batch fallback)
    #[arg(long, global = true)]
    pub quiet: bool,

    /// Client-side control timeout in milliseconds (env: AOS_TIMEOUT_MS)
    #[arg(long, global = true, env = "AOS_TIMEOUT_MS")]
    pub timeout_ms: Option<u64>,

    /// Drop provenance metadata in JSON output
    #[arg(long, global = true)]
    pub no_meta: bool,

    /// AIR assets directory (env: AOS_AIR, default: <world>/air)
    #[arg(long, global = true, env = "AOS_AIR")]
    pub air: Option<PathBuf>,

    /// Reducer crate directory (env: AOS_REDUCER, default: <world>/reducer)
    #[arg(long = "reducer-dir", global = true, env = "AOS_REDUCER")]
    pub reducer: Option<PathBuf>,

    /// Store/journal directory (env: AOS_STORE, default: <world>)
    #[arg(long, global = true, env = "AOS_STORE")]
    pub store: Option<PathBuf>,

    /// Module name to patch with compiled WASM
    #[arg(long, global = true)]
    pub module: Option<String>,

    /// Force reducer recompilation
    #[arg(long, global = true)]
    pub force_build: bool,

    /// Override HTTP adapter timeout (milliseconds) (env: AOS_HTTP_TIMEOUT_MS)
    #[arg(long, global = true, env = "AOS_HTTP_TIMEOUT_MS", hide = true)]
    pub http_timeout_ms: Option<u64>,

    /// Override HTTP adapter max response body size (bytes) (env: AOS_HTTP_MAX_BODY_BYTES)
    #[arg(long, global = true, env = "AOS_HTTP_MAX_BODY_BYTES", hide = true)]
    pub http_max_body_bytes: Option<usize>,
}

/// Execution mode for CLI reads/writes.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Daemon,
    Batch,
}

/// Resolved directory paths for a world.
#[derive(Debug, Clone)]
pub struct ResolvedDirs {
    /// The world root directory.
    pub world: PathBuf,
    /// AIR assets directory.
    pub air_dir: PathBuf,
    /// Reducer crate directory.
    pub reducer_dir: PathBuf,
    /// Store root directory (contains .aos/).
    pub store_root: PathBuf,
    /// Control socket path.
    pub control_socket: PathBuf,
}

/// Resolve the world directory from options.
///
/// Priority:
/// 1. `--world` / `-w` flag
/// 2. `AOS_WORLD` env var (handled by Clap)
/// 3. Walk up from CWD to find a world marker (aos.sync.json, air/, .aos/)
/// 4. Error
pub fn resolve_world(opts: &WorldOpts) -> Result<PathBuf> {
    // 1 & 2: Explicit flag or env var (Clap handles env with `env = "..."`)
    if let Some(w) = &opts.world {
        return Ok(w.clone());
    }

    // 3: Walk up from CWD to find a marker
    let cwd = std::env::current_dir().context("get current directory")?;
    if let Some(found) = find_world_root(&cwd) {
        return Ok(found);
    };

    // 4: Error
    anyhow::bail!(
        "No world specified. Pass --world <DIR>, set AOS_WORLD env var, \
         or run from a directory containing aos.sync.json, air/, or .aos/"
    );
}

/// Resolve all world directories from options.
///
/// Relative paths are resolved relative to the world directory.
pub fn resolve_dirs(opts: &WorldOpts) -> Result<ResolvedDirs> {
    let world = resolve_world(opts)?;

    let air_dir = opts
        .air
        .clone()
        .map(|p| if p.is_relative() { world.join(p) } else { p })
        .unwrap_or_else(|| world.join("air"));

    let reducer_dir = opts
        .reducer
        .clone()
        .map(|p| if p.is_relative() { world.join(p) } else { p })
        .unwrap_or_else(|| world.join("reducer"));

    let store_root = opts
        .store
        .clone()
        .map(|p| if p.is_relative() { world.join(p) } else { p })
        .unwrap_or_else(|| world.clone());
    let control_socket = opts
        .control
        .clone()
        .map(|p| {
            if p.is_relative() {
                store_root.join(p)
            } else {
                p
            }
        })
        .unwrap_or_else(|| store_root.join(".aos/control.sock"));

    Ok(ResolvedDirs {
        world,
        air_dir,
        reducer_dir,
        store_root,
        control_socket,
    })
}

/// Walk upward from a starting directory looking for a world marker.
fn find_world_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start.to_path_buf());
    while let Some(dir) = current {
        if dir.join("aos.sync.json").exists() || dir.join("air").exists() || dir.join(".aos").exists() {
            return Some(dir);
        }
        current = dir.parent().map(|p| p.to_path_buf());
    }
    None
}
