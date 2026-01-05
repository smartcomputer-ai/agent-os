//! `aos world init` command.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use aos_air_types::{AirNode, Manifest, CURRENT_AIR_VERSION};
use clap::Args;

use crate::opts::WorldOpts;

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Path to create world (defaults to --world/AOS_WORLD or current directory)
    pub path: Option<PathBuf>,

    /// Template to use (counter, http, llm-chat)
    #[arg(long)]
    pub template: Option<String>,

    /// Create missing directories (air/, reducer/, modules/, .aos/)
    #[arg(long)]
    pub dirs: bool,

    /// Create missing manifest (air/manifest.air.json)
    #[arg(long)]
    pub manifest: bool,

    /// Overwrite manifest even if it exists
    #[arg(long)]
    pub manifest_force: bool,

    /// Create missing sync file (aos.sync.json)
    #[arg(long)]
    pub sync: bool,

    /// Overwrite sync file even if it exists
    #[arg(long)]
    pub sync_force: bool,
}

pub fn cmd_init(opts: &WorldOpts, args: &InitArgs) -> Result<()> {
    let world_root = resolve_world_root(opts, args)?;
    let air_dir = resolve_opt_path(&world_root, opts.air.as_deref(), "air");
    let reducer_dir = resolve_opt_path(&world_root, opts.reducer.as_deref(), "reducer");
    let store_root = resolve_opt_path(&world_root, opts.store.as_deref(), "");
    let modules_dir = world_root.join("modules");

    let any_scoped = args.dirs
        || args.manifest
        || args.manifest_force
        || args.sync
        || args.sync_force;
    let do_dirs = if any_scoped { args.dirs } else { true };
    let do_manifest = if any_scoped {
        args.manifest || args.manifest_force
    } else {
        true
    };
    let do_sync = if any_scoped {
        args.sync || args.sync_force
    } else {
        true
    };
    let do_modules_dir = args.dirs;

    let world_status = init_dir(&world_root, true)?;
    let air_status = init_dir(&air_dir, do_dirs || do_manifest)?;
    let reducer_root_status = init_dir(&reducer_dir, do_dirs)?;
    let reducer_src_status = init_dir(&reducer_dir.join("src"), do_dirs)?;
    let reducer_status = if do_dirs {
        if matches!(reducer_root_status, InitStatus::Created)
            || matches!(reducer_src_status, InitStatus::Created)
        {
            InitStatus::Created
        } else {
            reducer_root_status
        }
    } else {
        reducer_root_status
    };
    let modules_status = init_dir(&modules_dir, do_modules_dir)?;
    let store_status = init_dir(&store_root.join(".aos"), do_dirs)?;

    let manifest_path = air_dir.join("manifest.air.json");
    let manifest_status = init_file(
        &manifest_path,
        do_manifest,
        args.manifest_force,
        || write_manifest_file(&manifest_path),
    )?;

    let sync_path = world_root.join("aos.sync.json");
    let sync_status = init_file(
        &sync_path,
        do_sync,
        args.sync_force,
        || write_sync_file(&sync_path, &world_root, &air_dir, &reducer_dir),
    )?;

    // TODO: Support --template to scaffold different starter manifests

    println!(
        "World initialized at {} ({})",
        world_root.display(),
        world_status.label()
    );
    println!(
        "  AIR assets: {} ({})",
        air_dir.display(),
        air_status.label()
    );
    println!(
        "  Reducer:    {} ({})",
        reducer_dir.display(),
        reducer_status.label()
    );
    println!(
        "  Modules:    {} ({})",
        modules_dir.display(),
        modules_status.label()
    );
    println!(
        "  Store:      {} ({})",
        store_root.join(".aos").display(),
        store_status.label()
    );
    println!(
        "  Manifest:   {} ({})",
        manifest_path.display(),
        manifest_status.label()
    );
    println!(
        "  Sync:       {} ({})",
        sync_path.display(),
        sync_status.label()
    );

    if args.template.is_some() {
        println!("\nNote: --template is not yet implemented; created minimal manifest.");
    }

    Ok(())
}

fn resolve_world_root(opts: &WorldOpts, args: &InitArgs) -> Result<PathBuf> {
    if let Some(path) = &args.path {
        return Ok(path.to_path_buf());
    }
    if let Some(world) = &opts.world {
        return Ok(world.to_path_buf());
    }
    std::env::current_dir().context("get current directory")
}

fn resolve_opt_path(root: &Path, override_path: Option<&Path>, fallback: &str) -> PathBuf {
    match override_path {
        Some(path) if path.is_relative() => root.join(path),
        Some(path) => path.to_path_buf(),
        None if fallback.is_empty() => root.to_path_buf(),
        None => root.join(fallback),
    }
}

fn write_manifest_file(manifest_path: &Path) -> Result<()> {
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent).context("create air dir")?;
    }
    let manifest = Manifest {
        air_version: CURRENT_AIR_VERSION.to_string(),
        schemas: Vec::new(),
        modules: Vec::new(),
        plans: Vec::new(),
        effects: Vec::new(),
        caps: Vec::new(),
        policies: Vec::new(),
        secrets: Vec::new(),
        defaults: None,
        module_bindings: Default::default(),
        routing: None,
        triggers: Vec::new(),
    };
    let node = AirNode::Manifest(manifest);
    let json = serde_json::to_string_pretty(&node).context("serialize manifest")?;
    fs::write(manifest_path, json).context("write manifest.air.json")?;
    Ok(())
}

fn write_sync_file(
    sync_path: &Path,
    world_root: &Path,
    air_dir: &Path,
    reducer_dir: &Path,
) -> Result<()> {
    let air_value = map_path_value(world_root, air_dir, "air");
    let reducer_value = map_path_value(world_root, reducer_dir, "reducer");
    let sync = serde_json::json!({
        "version": 1,
        "air": { "dir": air_value },
        "build": { "reducer_dir": reducer_value },
        "modules": { "pull": false },
        "workspaces": [
            {
                "ref": "reducer",
                "dir": reducer_value,
                "ignore": ["target/", ".git/", ".aos/"]
            }
        ]
    });
    fs::write(
        sync_path,
        serde_json::to_string_pretty(&sync).context("serialize sync config")?,
    )
    .context("write aos.sync.json")?;
    Ok(())
}

fn map_path_value(root: &Path, path: &Path, default_rel: &str) -> String {
    let default_path = root.join(default_rel);
    if path == default_path {
        return default_rel.to_string();
    }
    if let Ok(rel) = path.strip_prefix(root) {
        if !rel.as_os_str().is_empty() {
            return rel.to_string_lossy().to_string();
        }
    }
    path.to_string_lossy().to_string()
}

#[derive(Clone, Copy)]
enum InitStatus {
    Created,
    Exists,
    Overwritten,
    Missing,
}

impl InitStatus {
    fn label(self) -> &'static str {
        match self {
            InitStatus::Created => "created",
            InitStatus::Exists => "skipped",
            InitStatus::Overwritten => "overwritten",
            InitStatus::Missing => "missing",
        }
    }
}

fn init_dir(path: &Path, enabled: bool) -> Result<InitStatus> {
    let exists = path.exists();
    if enabled {
        if exists {
            if !path.is_dir() {
                anyhow::bail!("path is not a directory: {}", path.display());
            }
            return Ok(InitStatus::Exists);
        }
        fs::create_dir_all(path)?;
        Ok(InitStatus::Created)
    } else if exists {
        if !path.is_dir() {
            anyhow::bail!("path is not a directory: {}", path.display());
        }
        Ok(InitStatus::Exists)
    } else {
        Ok(InitStatus::Missing)
    }
}

fn init_file<F>(path: &Path, enabled: bool, force: bool, write: F) -> Result<InitStatus>
where
    F: FnOnce() -> Result<()>,
{
    let exists = path.exists();
    if enabled {
        if exists {
            if !path.is_file() {
                anyhow::bail!("path is not a file: {}", path.display());
            }
            if force {
                write()?;
                Ok(InitStatus::Overwritten)
            } else {
                Ok(InitStatus::Exists)
            }
        } else {
            write()?;
            Ok(InitStatus::Created)
        }
    } else if exists {
        if !path.is_file() {
            anyhow::bail!("path is not a file: {}", path.display());
        }
        Ok(InitStatus::Exists)
    } else {
        Ok(InitStatus::Missing)
    }
}
