//! `aos push` command.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use clap::Args;
use serde_json::json;

use aos_air_types::{AirNode, HashRef};
use aos_host::manifest_loader;
use aos_host::world_io::{WorldBundle, build_patch_document, manifest_node_hash};
use aos_kernel::ManifestLoader;
use aos_kernel::patch_doc::compile_patch_document;
use aos_kernel::secret::{MapSecretResolver, SharedSecretResolver};
use aos_store::{FsStore, Store};

use crate::opts::WorldOpts;
use crate::opts::resolve_dirs;
use crate::output::print_success;
use crate::util::{
    compile_workflow, latest_manifest_hash_from_journal, load_world_env,
    host_config_from_opts, make_kernel_config, resolve_placeholder_modules,
};

use super::create_host;
use super::sync::{load_sync_config, resolve_air_sources};
use super::workspace_sync::{SyncPushOptions, SyncStats, sync_workspace_push};

#[derive(Args, Debug)]
pub struct PushArgs {
    /// Sync config path (defaults to <world>/aos.sync.json)
    #[arg(long)]
    pub map: Option<std::path::PathBuf>,

    /// Local directory to push (overrides map workspaces)
    pub dir: Option<std::path::PathBuf>,

    /// Workspace ref to push (overrides map workspaces)
    pub reference: Option<String>,

    /// Dry-run: emit patch doc or manifest hash and exit
    #[arg(long)]
    pub dry_run: bool,

    /// Remove workspace paths not present locally
    #[arg(long)]
    pub prune: bool,

    /// Annotation message to set on workspace root
    #[arg(long)]
    pub message: Option<String>,
}

pub async fn cmd_push(opts: &WorldOpts, args: &PushArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    load_world_env(&dirs.world)?;
    let (map_path, config) = load_sync_config(&dirs.world, args.map.as_deref())?;
    let map_root = map_path.parent().unwrap_or(&dirs.world);
    let store = FsStore::open(&dirs.store_root).context("open store")?;
    let store = Arc::new(store);

    let workflow_dir = config
        .build
        .as_ref()
        .and_then(|build| build.workflow_dir.as_ref())
        .map(|dir| resolve_map_path(map_root, dir))
        .unwrap_or_else(|| dirs.workflow_dir.clone());
    let target_module = config
        .build
        .as_ref()
        .and_then(|build| build.module.as_deref())
        .or(opts.module.as_deref());

    let air_sources =
        resolve_air_sources(&dirs.world, map_root, &config, &dirs.air_dir, &workflow_dir)?;
    let air_dir = air_sources.air_dir;
    let mut warnings = air_sources.warnings.clone();

    let assets = manifest_loader::load_from_assets_with_imports_and_defs(
        store.clone(),
        &air_dir,
        &air_sources.import_dirs,
    )
    .with_context(|| format!("load AIR assets from {}", air_dir.display()))?
    .ok_or_else(|| anyhow!("no manifest found in {}", air_dir.display()))?;

    let mut loaded = assets.loaded;
    let secrets = assets.secrets;
    let compiled = if workflow_dir.exists() {
        let compiled = compile_workflow(
            &workflow_dir,
            &dirs.store_root,
            store.as_ref(),
            opts.force_build,
        )?;
        if !compiled.cache_hit {
            println!("Compiled workflow from {}", workflow_dir.display());
            println!("Workflow compiled: {}", compiled.hash.as_str());
        }
        Some(compiled.hash)
    } else {
        None
    };

    let patched = resolve_placeholder_modules(
        &mut loaded,
        store.as_ref(),
        &dirs.world,
        &dirs.store_root,
        compiled.as_ref(),
        target_module,
    )?;
    if patched > 0 {
        println!("Resolved {} module(s) with WASM hash", patched);
    }

    refresh_module_refs(&mut loaded, store.as_ref())?;
    let workspace_entries = resolve_workspace_entries(&dirs.world, map_root, &config, args)?;
    if let Some(base_hash) = latest_manifest_hash_from_journal(&dirs.store_root)? {
        let bundle = WorldBundle::from_loaded_assets(loaded, secrets);
        let base_hex = base_hash.to_hex();
        let base_loaded = ManifestLoader::load_from_hash(store.as_ref(), base_hash)
            .context("load base manifest")?;
        let doc = build_patch_document(&bundle, &base_loaded.manifest, &base_hex)?;
        let doc_json = serde_json::to_value(&doc).context("serialize patch doc")?;
        if args.dry_run {
            return print_success(opts, doc_json, None, warnings);
        }

        let compiled = compile_patch_document(store.as_ref(), doc).context("compile patch doc")?;
        let mut kernel_config = make_kernel_config(&dirs.store_root)?;
        if let Some(resolver) = secret_resolver_from_bundle(&bundle)? {
            kernel_config.secret_resolver = Some(resolver);
        }
        let host_config = host_config_from_opts(opts.http_timeout_ms, opts.http_max_body_bytes);
        let mut host = aos_host::host::WorldHost::from_loaded_manifest(
            store.clone(),
            base_loaded,
            &dirs.store_root,
            host_config,
            kernel_config,
        )?;
        let manifest_hash = host
            .kernel_mut()
            .apply_patch_direct(compiled)
            .context("apply patch")?;
        sync_workspaces(
            &mut host,
            store.as_ref(),
            &workspace_entries,
            &SyncPushOptions {
                prune: args.prune,
                message: args.message.as_deref(),
            },
            &mut warnings,
        )?;
        host.kernel_mut()
            .create_snapshot()
            .context("create snapshot")?;

        return print_success(
            opts,
            json!({ "manifest_hash": manifest_hash, "map": map_path.display().to_string() }),
            None,
            warnings,
        );
    }

    let manifest_hash = manifest_node_hash(&loaded.manifest)?;
    if args.dry_run {
        return print_success(
            opts,
            json!({ "manifest_hash": manifest_hash }),
            None,
            warnings,
        );
    }

    let mut host = create_host(store.clone(), loaded, &dirs, opts)?;
    sync_workspaces(
        &mut host,
        store.as_ref(),
        &workspace_entries,
        &SyncPushOptions {
            prune: args.prune,
            message: args.message.as_deref(),
        },
        &mut warnings,
    )?;
    host.kernel_mut()
        .create_snapshot()
        .context("create snapshot")?;

    print_success(
        opts,
        json!({ "manifest_hash": manifest_hash, "map": map_path.display().to_string() }),
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
    args: &PushArgs,
) -> Result<Vec<WorkspaceEntry>> {
    let (dir, reference) = match (&args.dir, &args.reference) {
        (Some(dir), Some(reference)) => (Some(dir), Some(reference)),
        (None, None) => (None, None),
        _ => anyhow::bail!("push requires both <dir> and <ref> when specifying a workspace"),
    };
    if let (Some(dir), Some(reference)) = (dir, reference) {
        let resolved = resolve_cli_path(world_root, dir);
        return Ok(vec![WorkspaceEntry {
            reference: reference.clone(),
            dir: resolved,
            ignore: Vec::new(),
            annotations: BTreeMap::new(),
        }]);
    }
    let mut entries = Vec::new();
    for entry in &config.workspaces {
        entries.push(WorkspaceEntry {
            reference: entry.reference.clone(),
            dir: resolve_map_path(map_root, &entry.dir),
            ignore: entry.ignore.clone(),
            annotations: entry.annotations.clone(),
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
    annotations: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
}

fn sync_workspaces(
    host: &mut aos_host::host::WorldHost<FsStore>,
    store: &FsStore,
    entries: &[WorkspaceEntry],
    opts: &SyncPushOptions<'_>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    for entry in entries {
        let stats = sync_workspace_push(
            host,
            store,
            &entry.reference,
            &entry.dir,
            &entry.ignore,
            &entry.annotations,
            opts,
        )?;
        if should_report_workspace(&stats) {
            warnings.push(format!(
                "workspace '{}' synced (writes {}, removes {}, annotations {})",
                entry.reference, stats.writes, stats.removes, stats.annotations
            ));
        }
    }
    Ok(())
}

fn should_report_workspace(stats: &SyncStats) -> bool {
    stats.writes > 0 || stats.removes > 0 || stats.annotations > 0 || stats.committed
}

fn store_module_hash(store: &FsStore, module: &aos_air_types::DefModule) -> Result<HashRef> {
    let hash = store
        .put_node(&AirNode::Defmodule(module.clone()))
        .context("store module node")?;
    HashRef::new(hash.to_hex()).context("create module hash ref")
}

fn refresh_module_refs(loaded: &mut aos_kernel::LoadedManifest, store: &FsStore) -> Result<()> {
    for module in loaded.modules.values() {
        let hash_ref = store_module_hash(store, module)?;
        for entry in loaded.manifest.modules.iter_mut() {
            if entry.name == module.name {
                entry.hash = hash_ref.clone();
                break;
            }
        }
    }
    Ok(())
}

fn secret_resolver_from_bundle(bundle: &WorldBundle) -> Result<Option<SharedSecretResolver>> {
    let referenced: BTreeSet<&str> = bundle
        .manifest
        .secrets
        .iter()
        .filter_map(|entry| match entry {
            aos_air_types::SecretEntry::Ref(named) => Some(named.name.as_str()),
            aos_air_types::SecretEntry::Decl(_) => None,
        })
        .collect();

    if referenced.is_empty() {
        return Ok(None);
    }

    let defs_by_name: HashMap<&str, &aos_air_types::DefSecret> = bundle
        .secrets
        .iter()
        .map(|secret| (secret.name.as_str(), secret))
        .collect();

    let mut values: HashMap<String, Vec<u8>> = HashMap::new();
    for name in referenced {
        let secret = defs_by_name.get(name).ok_or_else(|| {
            anyhow!("manifest references secret '{name}' but no matching defsecret was loaded")
        })?;
        let binding = secret.binding_id.as_str();
        let var_name = binding.strip_prefix("env:").ok_or_else(|| {
            anyhow!(
                "unsupported secret binding '{binding}' for '{name}' (expected env:VAR_NAME)"
            )
        })?;
        if var_name.is_empty() {
            anyhow::bail!("invalid empty env binding for secret '{name}'");
        }
        let value = std::env::var(var_name).map_err(|_| {
            anyhow!("missing env var '{var_name}' required by secret '{name}' ({binding})")
        })?;
        values.insert(binding.to_string(), value.into_bytes());
    }

    Ok(Some(Arc::new(MapSecretResolver::new(values))))
}
