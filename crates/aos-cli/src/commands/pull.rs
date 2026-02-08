//! `aos pull` command.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use clap::Args;
use serde_json::json;

use aos_cbor::Hash;
use aos_host::util::is_placeholder_hash;
use aos_host::world_io::{
    WorldBundle, WriteOptions, manifest_node_bytes, write_air_layout_with_options,
};
use aos_kernel::ManifestLoader;
use aos_store::{FsStore, Store};

use crate::opts::WorldOpts;
use crate::opts::resolve_dirs;
use crate::output::print_success;
use crate::util::latest_manifest_hash_from_journal;

use super::create_host;
use super::sync::load_sync_config;
use super::workspace_sync::{SyncPullOptions, SyncStats, sync_workspace_pull};

#[derive(Args, Debug)]
pub struct PullArgs {
    /// Sync config path (defaults to <world>/aos.sync.json)
    #[arg(long)]
    pub map: Option<PathBuf>,

    /// Workspace ref to pull (overrides map workspaces)
    pub reference: Option<String>,

    /// Local directory to pull into (overrides map workspaces)
    pub dir: Option<PathBuf>,

    /// Export modules (writes modules/ directory)
    #[arg(long)]
    pub modules: bool,

    /// Keep wasm hashes in AIR JSON (default: strip)
    #[arg(long)]
    pub keep_wasm_hashes: bool,

    /// Remove local paths not present in workspace
    #[arg(long)]
    pub prune: bool,

    /// Dry-run: show what would change without writing
    #[arg(long)]
    pub dry_run: bool,
}

pub async fn cmd_pull(opts: &WorldOpts, args: &PullArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let (map_path, config) = load_sync_config(&dirs.world, args.map.as_deref())?;
    let map_root = map_path.parent().unwrap_or(&dirs.world);
    let store = Arc::new(FsStore::open(&dirs.store_root).context("open store")?);

    let Some(manifest_hash) = latest_manifest_hash_from_journal(&dirs.store_root)? else {
        anyhow::bail!("no manifest found in journal; run `aos push` first");
    };

    let loaded = ManifestLoader::load_from_hash(store.as_ref(), manifest_hash)
        .context("load manifest from CAS")?;

    let air_dir = config
        .air
        .as_ref()
        .and_then(|air| air.dir.as_ref())
        .map(|dir| resolve_map_path(map_root, dir))
        .unwrap_or_else(|| dirs.air_dir.clone());
    let mut warnings = Vec::new();
    if !args.dry_run
        && (args.modules
            || config
                .modules
                .as_ref()
                .and_then(|m| m.pull)
                .unwrap_or(false))
    {
        let modules_dir = config
            .modules
            .as_ref()
            .and_then(|m| m.dir.as_ref())
            .map(|dir| resolve_map_path(map_root, dir))
            .unwrap_or_else(|| dirs.world.join("modules"));
        export_modules(store.as_ref(), &loaded, &modules_dir, &mut warnings)?;
    } else if args.dry_run
        && (args.modules
            || config
                .modules
                .as_ref()
                .and_then(|m| m.pull)
                .unwrap_or(false))
    {
        warnings.push("dry-run: skipped module export".into());
    }

    let workspace_entries = resolve_workspace_entries(&dirs.world, map_root, &config, args)?;

    let bundle = WorldBundle::from_loaded(loaded);

    if !args.dry_run {
        fs::create_dir_all(&air_dir).context("create air dir")?;
        fs::create_dir_all(dirs.world.join(".aos")).context("ensure .aos dir")?;

        let manifest_bytes = manifest_node_bytes(&bundle.manifest)?;
        write_air_layout_with_options(
            &bundle,
            &manifest_bytes,
            &dirs.world,
            WriteOptions {
                include_sys: false,
                defs_bundle: false,
                strip_wasm_hashes: !args.keep_wasm_hashes,
                write_manifest_cbor: false,
                air_dir: Some(air_dir),
            },
        )?;
    } else {
        warnings.push("dry-run: skipped AIR export".into());
    }

    if !workspace_entries.is_empty() {
        let loaded_for_host = ManifestLoader::load_from_hash(store.as_ref(), manifest_hash)
            .context("load manifest for workspace sync")?;
        let mut host = create_host(store.clone(), loaded_for_host, &dirs, opts)?;
        sync_workspaces(
            &mut host,
            &workspace_entries,
            &SyncPullOptions {
                prune: args.prune,
                dry_run: args.dry_run,
            },
            &mut warnings,
        )?;
    }

    print_success(
        opts,
        json!({
            "manifest_hash": manifest_hash.to_hex(),
            "map": map_path.display().to_string(),
        }),
        None,
        warnings,
    )
}

fn resolve_map_path(map_root: &Path, path: &Path) -> PathBuf {
    if path.is_relative() {
        map_root.join(path)
    } else {
        path.to_path_buf()
    }
}

fn resolve_workspace_entries(
    world_root: &Path,
    map_root: &Path,
    config: &super::sync::SyncConfig,
    args: &PullArgs,
) -> Result<Vec<WorkspaceEntry>> {
    let (reference, dir) = match (&args.reference, &args.dir) {
        (Some(reference), Some(dir)) => (Some(reference), Some(dir)),
        (None, None) => (None, None),
        _ => anyhow::bail!("pull requires both <ref> and <dir> when specifying a workspace"),
    };
    if let (Some(reference), Some(dir)) = (reference, dir) {
        let resolved = resolve_cli_path(world_root, dir);
        return Ok(vec![WorkspaceEntry {
            reference: reference.clone(),
            dir: resolved,
            ignore: Vec::new(),
        }]);
    }
    let mut entries = Vec::new();
    for entry in &config.workspaces {
        entries.push(WorkspaceEntry {
            reference: entry.reference.clone(),
            dir: resolve_map_path(map_root, &entry.dir),
            ignore: entry.ignore.clone(),
        });
    }
    Ok(entries)
}

fn resolve_cli_path(world_root: &Path, path: &Path) -> PathBuf {
    if path.is_relative() {
        world_root.join(path)
    } else {
        path.to_path_buf()
    }
}

struct WorkspaceEntry {
    reference: String,
    dir: PathBuf,
    ignore: Vec<String>,
}

fn sync_workspaces(
    host: &mut aos_host::host::WorldHost<FsStore>,
    entries: &[WorkspaceEntry],
    opts: &SyncPullOptions,
    warnings: &mut Vec<String>,
) -> Result<()> {
    for entry in entries {
        let stats = sync_workspace_pull(host, &entry.reference, &entry.dir, &entry.ignore, opts)?;
        if should_report_workspace(&stats) {
            warnings.push(format!(
                "workspace '{}' synced (writes {}, removes {})",
                entry.reference, stats.writes, stats.removes
            ));
        }
    }
    Ok(())
}

fn should_report_workspace(stats: &SyncStats) -> bool {
    stats.writes > 0 || stats.removes > 0 || stats.annotations > 0 || stats.committed
}

fn export_modules(
    store: &FsStore,
    loaded: &aos_kernel::LoadedManifest,
    modules_dir: &Path,
    warnings: &mut Vec<String>,
) -> Result<()> {
    if loaded.modules.is_empty() {
        warnings.push("module export requested but manifest has no modules".into());
        return Ok(());
    }
    fs::create_dir_all(modules_dir).context("create modules dir")?;

    for module in loaded.modules.values() {
        if module.name.as_str().starts_with("sys/") {
            continue;
        }
        if is_placeholder_hash(module) {
            warnings.push(format!(
                "module '{}' has placeholder wasm_hash; skipping export",
                module.name
            ));
            continue;
        }
        let hash = Hash::from_hex_str(module.wasm_hash.as_str())
            .map_err(|e| anyhow!("invalid wasm hash for '{}': {e}", module.name))?;
        let bytes = store
            .get_blob(hash)
            .with_context(|| format!("load wasm blob for {}", module.name))?;
        let path = modules_dir.join(format!("{}-{}.wasm", module.name, hash.to_hex()));
        fs::write(&path, bytes).with_context(|| format!("write module {}", path.display()))?;
    }
    Ok(())
}
