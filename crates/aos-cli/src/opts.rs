//! Global CLI options and world resolution.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;

/// Global options for world commands.
///
/// These options apply to all `aos world` subcommands and can be set via env vars.
#[derive(Args, Debug, Clone)]
pub struct WorldOpts {
    /// World directory (env: AOS_WORLD)
    #[arg(short = 'w', long, global = true, env = "AOS_WORLD")]
    pub world: Option<PathBuf>,

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

    /// Override HTTP adapter timeout (milliseconds)
    #[arg(long, global = true)]
    pub http_timeout_ms: Option<u64>,

    /// Override HTTP adapter max response body size (bytes)
    #[arg(long, global = true)]
    pub http_max_body_bytes: Option<usize>,

    /// Disable LLM adapter
    #[arg(long, global = true)]
    pub no_llm: bool,
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
}

impl ResolvedDirs {
    /// Path to the control socket.
    pub fn control_socket(&self) -> PathBuf {
        self.store_root.join(".aos/control.sock")
    }
}

/// Resolve the world directory from options.
///
/// Priority:
/// 1. `--world` / `-w` flag
/// 2. `AOS_WORLD` env var (handled by Clap)
/// 3. CWD if it looks like a world (contains air/, .aos/, or manifest.air.json)
/// 4. Error
pub fn resolve_world(opts: &WorldOpts) -> Result<PathBuf> {
    // 1 & 2: Explicit flag or env var (Clap handles env with `env = "..."`)
    if let Some(w) = &opts.world {
        return Ok(w.clone());
    }

    // 3: CWD detection
    let cwd = std::env::current_dir().context("get current directory")?;
    if cwd.join("air").exists()
        || cwd.join(".aos").exists()
        || cwd.join("manifest.air.json").exists()
    {
        return Ok(cwd);
    }

    // 4: Error
    anyhow::bail!(
        "No world specified. Pass --world <DIR>, set AOS_WORLD env var, \
         or run from a directory containing air/, .aos/, or manifest.air.json"
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

    Ok(ResolvedDirs {
        world,
        air_dir,
        reducer_dir,
        store_root,
    })
}
