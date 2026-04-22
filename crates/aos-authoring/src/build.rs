//! Shared authoring/build helpers used by node-facing CLIs and probes.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use aos_air_types::{AirNode, HashRef};
use aos_cbor::Hash;
use aos_kernel::Store;
use aos_kernel::{LoadedManifest, MemStore};
use aos_node::{FsCas, LocalStatePaths};
use aos_wasm_build::{BuildRequest, Builder};
use camino::Utf8PathBuf;
use walkdir::WalkDir;

use crate::bundle::WorldBundle;
use crate::local::local_state_paths;
use crate::manifest_loader;
use crate::sync::ResolvedAirImport;
use crate::sync::{load_sync_config, resolve_air_sources};
use crate::util::{is_placeholder_hash, set_module_wasm_hash};

pub struct CompiledWorkflow {
    pub hash: HashRef,
    pub cache_hit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkflowBuildProfile {
    Debug,
    Release,
}

impl WorkflowBuildProfile {
    fn is_release(self) -> bool {
        matches!(self, Self::Release)
    }
}

fn compile_workflow_with_cache_override(
    workflow_dir: &Path,
    cache_dir: Option<&Path>,
    store: &impl Store,
    force_build: bool,
    build_profile: WorkflowBuildProfile,
) -> Result<CompiledWorkflow> {
    if let Some(cache_dir) = cache_dir {
        fs::create_dir_all(cache_dir).context("create module cache directory")?;
    }

    let utf_path = Utf8PathBuf::from_path_buf(workflow_dir.to_path_buf())
        .map_err(|p| anyhow!("workflow path is not UTF-8: {}", p.display()))?;

    let mut request = BuildRequest::new(utf_path);
    request.cache_dir = cache_dir.map(Path::to_path_buf);
    request.use_cache = !force_build;
    request.config.release = build_profile.is_release();

    let artifact = Builder::compile(request).context("compile workflow")?;
    let hash = store
        .put_blob(&artifact.wasm_bytes)
        .context("store wasm blob")?;
    let hash_ref = HashRef::new(hash.to_hex()).context("create hash ref")?;
    let cache_hit = artifact.build_log.as_deref() == Some("cache hit");
    Ok(CompiledWorkflow {
        hash: hash_ref,
        cache_hit,
    })
}

/// Compile a workflow crate to WASM and store the blob.
pub fn compile_workflow(
    workflow_dir: &Path,
    paths: &LocalStatePaths,
    store: &impl Store,
    force_build: bool,
) -> Result<CompiledWorkflow> {
    let cache_dir = paths.module_cache_dir();
    compile_workflow_with_cache_override(
        workflow_dir,
        Some(&cache_dir),
        store,
        force_build,
        WorkflowBuildProfile::Debug,
    )
}

pub fn materialize_imported_cargo_modules(
    imports: &[ResolvedAirImport],
    world_root: &Path,
    cache_root: &Path,
    store: &impl Store,
    build_profile: WorkflowBuildProfile,
) -> Result<usize> {
    let mut refreshed = 0usize;
    for import in imports {
        let Some(manifest_path) = import.cargo_manifest_path.as_ref() else {
            continue;
        };
        for module_name in &import.cargo_module_names {
            let Some(bin_name) = module_bin_name(module_name) else {
                continue;
            };
            let bytes = build_cargo_wasm_bin(
                manifest_path,
                import.cargo_package.as_deref(),
                bin_name.as_str(),
                cache_root,
                build_profile,
            )?;
            let hash = Hash::of_bytes(&bytes).to_hex();
            let stored = store
                .put_blob(&bytes)
                .with_context(|| format!("store imported wasm blob for {module_name}"))?;
            let hash_ref =
                HashRef::new(stored.to_hex()).context("create imported module hash ref")?;
            if hash_ref.as_str() != hash {
                anyhow::bail!(
                    "imported wasm hash mismatch for '{}': computed {hash}, stored {}",
                    module_name,
                    hash_ref.as_str()
                );
            }
            remove_stale_module_files(&world_root.join("modules"), module_name, hash_ref.as_str())?;
            persist_module_file(
                &world_root.join("modules"),
                module_name,
                hash_ref.as_str(),
                &bytes,
            )?;
            refreshed += 1;
        }
    }
    Ok(refreshed)
}

/// Build a world bundle from a local authored-world root.
///
/// This resolves sync imports, materializes imported cargo modules, optionally
/// compiles the local workflow crate, patches placeholder hashes, refreshes
/// manifest module refs, and returns the opened local store alongside the
/// assembled bundle.
pub fn build_bundle_from_local_world(
    world_root: &Path,
    force_build: bool,
) -> Result<(FsCas, WorldBundle, Vec<String>)> {
    build_bundle_from_local_world_with_profile(
        world_root,
        force_build,
        WorkflowBuildProfile::Release,
    )
}

pub fn build_bundle_from_local_world_with_profile(
    world_root: &Path,
    force_build: bool,
    build_profile: WorkflowBuildProfile,
) -> Result<(FsCas, WorldBundle, Vec<String>)> {
    let paths = local_state_paths(world_root);
    paths.ensure_root().context("create local state root")?;
    let store = FsCas::open_with_paths(&paths).context("open local CAS")?;
    let (bundle, warnings) = build_bundle_from_local_world_with_store(
        world_root,
        &paths,
        &store,
        force_build,
        build_profile,
    )?;
    Ok((store, bundle, warnings))
}

pub fn build_bundle_from_local_world_ephemeral(
    world_root: &Path,
    force_build: bool,
) -> Result<(MemStore, WorldBundle, Vec<String>)> {
    build_bundle_from_local_world_ephemeral_with_profile(
        world_root,
        force_build,
        WorkflowBuildProfile::Release,
    )
}

pub fn build_bundle_from_local_world_ephemeral_with_profile(
    world_root: &Path,
    force_build: bool,
    build_profile: WorkflowBuildProfile,
) -> Result<(MemStore, WorldBundle, Vec<String>)> {
    let paths = local_state_paths(world_root);
    let store = MemStore::new();
    let (bundle, warnings) = build_bundle_from_local_world_with_store(
        world_root,
        &paths,
        &store,
        force_build,
        build_profile,
    )?;
    Ok((store, bundle, warnings))
}

fn build_bundle_from_local_world_with_store<S: Store + Clone + 'static>(
    world_root: &Path,
    paths: &LocalStatePaths,
    store: &S,
    force_build: bool,
    build_profile: WorkflowBuildProfile,
) -> Result<(WorldBundle, Vec<String>)> {
    let air_dir = world_root.join("air");
    let workflow_dir = world_root.join("workflow");
    let (map_path, config) = load_sync_config(world_root, None)?;
    let map_root = map_path.parent().unwrap_or(world_root);
    let air_sources = resolve_air_sources(world_root, map_root, &config, &air_dir, &workflow_dir)?;
    let assets = manifest_loader::load_from_assets_with_imports_and_defs(
        std::sync::Arc::new(store.clone()),
        &air_sources.air_dir,
        &air_sources.import_dirs,
    )
    .with_context(|| format!("load AIR assets from {}", air_sources.air_dir.display()))?
    .ok_or_else(|| anyhow!("no manifest found in {}", air_sources.air_dir.display()))?;

    let mut loaded = assets.loaded;
    let secrets = assets.secrets;
    materialize_imported_cargo_modules(
        &air_sources.imports,
        world_root,
        &paths.cache_root(),
        store,
        build_profile,
    )?;
    let compiled = if workflow_dir.exists() {
        Some(compile_workflow_with_cache_override(
            &workflow_dir,
            Some(&paths.module_cache_dir()),
            store,
            force_build,
            build_profile,
        )?)
    } else {
        None
    };
    resolve_placeholder_modules(
        &mut loaded,
        store,
        world_root,
        paths,
        compiled.as_ref().map(|value| &value.hash),
        None,
    )?;
    refresh_module_refs(&mut loaded, store)?;
    Ok((
        WorldBundle::from_loaded_assets(loaded, secrets),
        air_sources.warnings,
    ))
}

/// Build a loaded manifest directly from authored AIR plus an optional workflow crate,
/// without requiring a full local-world root or sync config.
///
/// This is the narrow authoring path for workflow-focused harness tests. The caller
/// provides a scratch root for local build/cache state; the authored inputs remain in place.
pub fn build_loaded_manifest_from_authored_paths(
    air_dir: &Path,
    workflow_dir: Option<&Path>,
    import_roots: &[PathBuf],
    scratch_root: &Path,
    force_build: bool,
    build_profile: WorkflowBuildProfile,
) -> Result<(MemStore, LoadedManifest)> {
    let paths = local_state_paths(scratch_root);
    paths.ensure_root().context("create local state root")?;
    let store = MemStore::new();
    let assets = manifest_loader::load_from_assets_with_imports_and_defs(
        std::sync::Arc::new(store.clone()),
        air_dir,
        import_roots,
    )
    .with_context(|| format!("load AIR assets from {}", air_dir.display()))?
    .ok_or_else(|| anyhow!("no manifest found in {}", air_dir.display()))?;

    let mut loaded = assets.loaded;
    let compiled = workflow_dir
        .filter(|path| path.exists())
        .map(|path| {
            compile_workflow_with_cache_override(path, None, &store, force_build, build_profile)
        })
        .transpose()?;
    let module_root = workflow_dir
        .map(|path| path.parent().unwrap_or(path))
        .unwrap_or_else(|| air_dir.parent().unwrap_or(air_dir));
    resolve_placeholder_modules(
        &mut loaded,
        &store,
        module_root,
        &paths,
        compiled.as_ref().map(|value| &value.hash),
        None,
    )?;
    refresh_module_refs(&mut loaded, &store)?;
    Ok((store, loaded))
}

/// Resolve placeholder module hashes in a loaded manifest.
///
/// Resolution order:
/// 1) Known system modules from workspace build artifacts (fallback: sys-module cache)
/// 2) `modules/` directory in the world root (content-addressed wasm files)
/// 3) Compiled workflow hash (if provided) when exactly one non-sys placeholder remains
///
/// If `specific_module` is provided, that module is patched with the compiled hash
/// (and must currently be a placeholder).
pub fn resolve_placeholder_modules(
    loaded: &mut LoadedManifest,
    store: &impl Store,
    world_root: &Path,
    paths: &LocalStatePaths,
    compiled_hash: Option<&HashRef>,
    specific_module: Option<&str>,
) -> Result<usize> {
    let mut patched = 0usize;

    if let Some(target) = specific_module {
        let Some(hash) = compiled_hash else {
            anyhow::bail!("--module requires a compiled workflow; no workflow/ found");
        };
        let mut found = false;
        for (name, module) in loaded.modules.iter_mut() {
            if name.as_str() == target {
                found = true;
                if !is_placeholder_hash(module) {
                    anyhow::bail!("module '{target}' already has a wasm_hash; remove it to patch");
                }
                if !set_module_wasm_hash(module, hash.clone()) {
                    anyhow::bail!("module '{target}' is not a wasm module");
                }
                patched += 1;
            }
        }
        if !found {
            anyhow::bail!("module '{target}' not found in manifest");
        }
    }

    let mut unresolved_non_sys: Vec<String> = Vec::new();
    let mut unresolved_sys: Vec<String> = Vec::new();

    for (name, module) in loaded.modules.iter_mut() {
        if !is_placeholder_hash(module) {
            continue;
        }
        if let Some(spec) = sys_module_spec(name.as_str()) {
            match resolve_sys_module(store, paths, world_root, spec)? {
                Some(hash) => {
                    if set_module_wasm_hash(module, hash) {
                        patched += 1;
                    }
                }
                None => {
                    unresolved_sys.push(name.to_string());
                }
            }
            continue;
        }
        if let Some(hash) = resolve_from_world_modules(store, world_root, name.as_str())? {
            if set_module_wasm_hash(module, hash) {
                patched += 1;
            }
            continue;
        }
        unresolved_non_sys.push(name.to_string());
    }

    if !unresolved_non_sys.is_empty() {
        if let Some(hash) = compiled_hash {
            if unresolved_non_sys.len() == 1 {
                let target = unresolved_non_sys.remove(0);
                if let Some(module) = loaded.modules.get_mut(target.as_str()) {
                    if set_module_wasm_hash(module, hash.clone()) {
                        patched += 1;
                    }
                }
            }
        }
    }

    let mut still_missing: Vec<String> = Vec::new();
    still_missing.extend(unresolved_non_sys);
    still_missing.extend(unresolved_sys);

    if !still_missing.is_empty() {
        let mut msg = String::from("unresolved module wasm hashes:\n");
        for name in &still_missing {
            msg.push_str(&format!("  - {name}\n"));
        }
        msg.push_str("\nResolution hints:\n");
        msg.push_str(
            "  - add content-addressed wasm to <world>/modules/<name>@<ver>-<hash>.wasm\n",
        );
        msg.push_str("  - build system modules with `cargo build -p aos-sys --target wasm32-unknown-unknown`\n");
        if compiled_hash.is_none() {
            msg.push_str("  - or provide a workflow/ to compile local modules\n");
        }
        anyhow::bail!(msg);
    }

    Ok(patched)
}

fn refresh_module_refs(loaded: &mut LoadedManifest, store: &impl Store) -> Result<()> {
    for module in loaded.modules.values() {
        let hash = store
            .put_node(&AirNode::Defmodule(module.clone()))
            .context("store module node")?;
        let hash_ref = HashRef::new(hash.to_hex()).context("create module hash ref")?;
        for entry in &mut loaded.manifest.modules {
            if entry.name == module.name {
                entry.hash = hash_ref.clone();
                break;
            }
        }
    }
    Ok(())
}

/// Create a kernel configuration for CLI usage.
///
/// CLI doesn't inject demo keys; secrets are resolved from env during host boot when available.
fn resolve_from_world_modules(
    store: &impl Store,
    world_root: &Path,
    module_name: &str,
) -> Result<Option<HashRef>> {
    let modules_dir = world_root.join("modules");
    resolve_from_modules_dir(store, &modules_dir, module_name)
}

fn resolve_from_sys_cache(
    store: &impl Store,
    paths: &LocalStatePaths,
    module_name: &str,
) -> Result<Option<HashRef>> {
    let modules_dir = sys_cache_dir(paths);
    resolve_from_modules_dir(store, &modules_dir, module_name)
}

fn resolve_from_modules_dir(
    store: &impl Store,
    modules_dir: &Path,
    module_name: &str,
) -> Result<Option<HashRef>> {
    if !modules_dir.exists() {
        return Ok(None);
    }

    let prefix = format!("{module_name}-");
    let mut matches: Vec<PathBuf> = Vec::new();

    for entry in WalkDir::new(modules_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("wasm") {
            continue;
        }
        let rel = path.strip_prefix(modules_dir).unwrap_or(path);
        let rel_str = rel.to_string_lossy();
        let rel_norm = rel_str.replace('\\', "/");
        if !rel_norm.starts_with(&prefix) {
            continue;
        }
        if rel_norm.contains('/') {
            // Only match exact module path + filename (no extra nested segments).
            if rel_norm.matches('/').count() > module_name.matches('/').count() {
                continue;
            }
        }
        matches.push(path.to_path_buf());
    }

    if matches.is_empty() {
        return Ok(None);
    }
    if matches.len() > 1 {
        let listed = matches
            .iter()
            .map(|p| format!("{}", p.display()))
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!("multiple wasm files found for module '{module_name}': {listed}");
    }

    let path = &matches[0];
    let rel = path.strip_prefix(modules_dir).unwrap_or(path);
    let rel_str = rel.to_string_lossy();
    let rel_norm = rel_str.replace('\\', "/");
    let hash_str = rel_norm
        .strip_suffix(".wasm")
        .and_then(|s| s.strip_prefix(&prefix))
        .ok_or_else(|| anyhow!("wasm filename does not match '{module_name}-<hash>.wasm'"))?;

    let expected = normalize_hash_str(hash_str)
        .ok_or_else(|| anyhow!("invalid hash in wasm filename '{rel_norm}'"))?;
    let bytes = fs::read(path).with_context(|| format!("read wasm file {}", path.display()))?;
    let actual = Hash::of_bytes(&bytes).to_hex();
    if expected != actual {
        anyhow::bail!(
            "wasm hash mismatch for module '{module_name}': filename has {expected}, computed {actual}"
        );
    }
    let stored = store.put_blob(&bytes).context("store wasm blob")?;
    HashRef::new(stored.to_hex())
        .map(Some)
        .context("create hash ref")
}

struct SysModuleSpec {
    name: &'static str,
    bin: &'static str,
}

fn sys_module_spec(name: &str) -> Option<&'static SysModuleSpec> {
    SYS_MODULES.iter().find(|spec| spec.name == name)
}

const SYS_MODULES: &[SysModuleSpec] = &[
    SysModuleSpec {
        name: "sys/workspace_wasm@1",
        bin: "workspace",
    },
    SysModuleSpec {
        name: "sys/http_publish_wasm@1",
        bin: "http_publish",
    },
];

fn resolve_sys_module(
    store: &impl Store,
    paths: &LocalStatePaths,
    world_root: &Path,
    spec: &SysModuleSpec,
) -> Result<Option<HashRef>> {
    let target_dir = resolve_target_dir();
    let profiles = ["debug", "release"];
    for profile in profiles {
        let path = target_dir
            .join("wasm32-unknown-unknown")
            .join(profile)
            .join(format!("{}.wasm", spec.bin));
        if path.exists() {
            let bytes =
                fs::read(&path).with_context(|| format!("read system wasm {}", path.display()))?;
            let hash = Hash::of_bytes(&bytes).to_hex();
            let stored = store.put_blob(&bytes).context("store system wasm blob")?;
            let hash_ref = HashRef::new(stored.to_hex()).context("create hash ref")?;
            if hash_ref.as_str() != hash {
                anyhow::bail!(
                    "system wasm hash mismatch for '{}': computed {hash}, stored {}",
                    spec.name,
                    hash_ref.as_str()
                );
            }
            persist_module_file(&sys_cache_dir(paths), spec.name, hash_ref.as_str(), &bytes)?;
            if should_copy_sys_modules() {
                persist_module_file(
                    &world_root.join("modules"),
                    spec.name,
                    hash_ref.as_str(),
                    &bytes,
                )?;
            }
            return Ok(Some(hash_ref));
        }
    }

    resolve_from_sys_cache(store, paths, spec.name)
}

pub fn resolve_sys_module_wasm_hash(
    store: &impl Store,
    paths: &LocalStatePaths,
    world_root: &Path,
    module_name: &str,
) -> Result<HashRef> {
    let Some(spec) = sys_module_spec(module_name) else {
        anyhow::bail!("unknown system module '{module_name}'");
    };
    if let Some(hash) = resolve_sys_module(store, paths, world_root, spec)? {
        return Ok(hash);
    }
    anyhow::bail!(
        "system wasm for '{module_name}' not found; build with `cargo build -p aos-sys --target wasm32-unknown-unknown`"
    );
}

fn persist_module_file(
    modules_dir: &Path,
    module_name: &str,
    hash: &str,
    bytes: &[u8],
) -> Result<()> {
    let path = modules_dir.join(format!("{module_name}-{hash}.wasm"));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create modules dir {}", parent.display()))?;
    }
    if path.exists() {
        let existing =
            fs::read(&path).with_context(|| format!("read existing module {}", path.display()))?;
        let existing_hash = Hash::of_bytes(&existing).to_hex();
        if existing_hash != hash {
            anyhow::bail!(
                "module file hash mismatch at {} (expected {hash}, found {existing_hash})",
                path.display()
            );
        }
        return Ok(());
    }
    fs::write(&path, bytes).with_context(|| format!("write module {}", path.display()))?;
    Ok(())
}

fn remove_stale_module_files(modules_dir: &Path, module_name: &str, keep_hash: &str) -> Result<()> {
    let prefix = format!("{module_name}-");
    if !modules_dir.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(modules_dir) {
        let entry = entry.context("walk modules dir")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(modules_dir).unwrap_or(path);
        let rel_norm = rel.to_string_lossy().replace('\\', "/");
        if !rel_norm.starts_with(&prefix) || !rel_norm.ends_with(".wasm") {
            continue;
        }
        let Some(found_hash) = rel_norm
            .strip_suffix(".wasm")
            .and_then(|value| value.strip_prefix(&prefix))
        else {
            continue;
        };
        let normalized =
            normalize_hash_str(found_hash).unwrap_or_else(|| format!("sha256:{found_hash}"));
        if normalized == keep_hash {
            continue;
        }
        fs::remove_file(path).with_context(|| format!("remove stale module {}", path.display()))?;
    }
    Ok(())
}

fn sys_cache_dir(paths: &LocalStatePaths) -> PathBuf {
    paths.cache_root().join("sys-modules")
}

fn should_copy_sys_modules() -> bool {
    match std::env::var("AOS_SYS_MODULES_COPY") {
        Ok(val) => {
            let val = val.to_lowercase();
            !(val.is_empty() || val == "0" || val == "false" || val == "no")
        }
        Err(_) => false,
    }
}

fn resolve_target_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let mut path = PathBuf::from(dir);
        if path.is_relative() {
            path = workspace_root().join(path);
        }
        return path;
    }
    workspace_root().join("target")
}

fn build_cargo_wasm_bin(
    manifest_path: &Path,
    package_name: Option<&str>,
    bin_name: &str,
    cache_root: &Path,
    build_profile: WorkflowBuildProfile,
) -> Result<Vec<u8>> {
    let target_dir = imported_cargo_target_dir(manifest_path, package_name, bin_name, cache_root);
    fs::create_dir_all(&target_dir)
        .with_context(|| format!("create imported cargo target dir {}", target_dir.display()))?;
    let mut command = std::process::Command::new("cargo");
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(manifest_path);
    if let Some(package_name) = package_name.filter(|value| !value.trim().is_empty()) {
        command.arg("-p").arg(package_name);
    }
    if build_profile.is_release() {
        command.arg("--release");
    }
    let status = command
        .arg("--bin")
        .arg(bin_name)
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .arg("--target-dir")
        .arg(&target_dir)
        .status()
        .with_context(|| format!("run cargo build for {}", manifest_path.display()))?;
    if !status.success() {
        anyhow::bail!(
            "cargo build failed for {} --bin {}",
            manifest_path.display(),
            bin_name
        );
    }
    let path = target_dir
        .join("wasm32-unknown-unknown")
        .join(if build_profile.is_release() {
            "release"
        } else {
            "debug"
        })
        .join(format!("{bin_name}.wasm"));
    fs::read(&path).with_context(|| format!("read built wasm {}", path.display()))
}

fn imported_cargo_target_dir(
    manifest_path: &Path,
    package_name: Option<&str>,
    bin_name: &str,
    cache_root: &Path,
) -> PathBuf {
    let key = format!(
        "{}::{}::{bin_name}",
        manifest_path.display(),
        package_name.unwrap_or_default()
    );
    let digest = Hash::of_bytes(key.as_bytes()).to_hex();
    let digest = digest.strip_prefix("sha256:").unwrap_or(digest.as_str());
    cache_root.join("imported-cargo").join(digest)
}

fn module_bin_name(module_name: &str) -> Option<String> {
    let tail = module_name.rsplit('/').next()?;
    let name = tail.split('@').next()?;
    if name.is_empty() {
        return None;
    }
    Some(camel_to_snake(name))
}

fn camel_to_snake(input: &str) -> String {
    let mut out = String::new();
    let mut prev_is_lower_or_digit = false;
    let mut prev_is_upper = false;
    let chars: Vec<char> = input.chars().collect();
    for (idx, ch) in chars.iter().copied().enumerate() {
        let next_is_lower = chars
            .get(idx + 1)
            .map(|next| next.is_ascii_lowercase())
            .unwrap_or(false);
        if ch.is_ascii_uppercase() {
            if idx > 0 && (prev_is_lower_or_digit || (prev_is_upper && next_is_lower)) {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            prev_is_lower_or_digit = false;
            prev_is_upper = true;
        } else {
            out.push(ch);
            prev_is_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
            prev_is_upper = false;
        }
    }
    out
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn normalize_hash_str(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.starts_with("sha256:") {
        Hash::from_hex_str(trimmed).ok()?;
        return Some(trimmed.to_string());
    }
    if trimmed.len() == 64 && hex::decode(trimmed).is_ok() {
        return Some(format!("sha256:{trimmed}"));
    }
    None
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;
    use crate::manifest_loader::ZERO_HASH_SENTINEL;

    fn fixture_root(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../aos-smoke/fixtures")
            .join(name)
    }

    #[test]
    fn build_loaded_manifest_from_authored_paths_supports_workflow_fixtures_without_world_root()
    -> Result<()> {
        let fixture = fixture_root("01-hello-timer");
        let scratch = tempdir()?;
        let (_store, loaded) = build_loaded_manifest_from_authored_paths(
            &fixture.join("air"),
            Some(&fixture.join("workflow")),
            &[],
            scratch.path(),
            false,
            WorkflowBuildProfile::Debug,
        )?;

        assert!(loaded.modules.contains_key("demo/TimerSM_wasm@1"));
        let module = loaded.modules.get("demo/TimerSM_wasm@1").unwrap();
        assert_ne!(
            crate::util::wasm_module_hash(module)
                .expect("workflow wasm hash")
                .as_str(),
            ZERO_HASH_SENTINEL
        );
        assert!(loaded.schemas.contains_key("demo/TimerEvent@1"));
        Ok(())
    }

    #[test]
    fn ephemeral_bundle_build_keeps_local_cas_out_of_world_root() -> Result<()> {
        let fixture = fixture_root("01-hello-timer");
        let temp = tempdir()?;
        for entry in WalkDir::new(&fixture) {
            let entry = entry?;
            let relative = entry
                .path()
                .strip_prefix(&fixture)
                .expect("fixture-relative path");
            let target = temp.path().join(relative);
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&target)?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(entry.path(), &target)?;
            }
        }
        std::fs::write(
            temp.path().join("aos.sync.json"),
            serde_json::to_vec(&serde_json::json!({ "version": 1 }))?,
        )?;
        let aos_state = temp.path().join(".aos");
        if aos_state.exists() {
            std::fs::remove_dir_all(&aos_state)?;
        }
        let crates_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .canonicalize()?;
        let cargo_toml = temp.path().join("workflow/Cargo.toml");
        let cargo_text = std::fs::read_to_string(&cargo_toml)?;
        let cargo_text = cargo_text.replace("../../../../", &format!("{}/", crates_root.display()));
        std::fs::write(cargo_toml, cargo_text)?;

        let (_store, bundle, _warnings) = build_bundle_from_local_world_ephemeral_with_profile(
            temp.path(),
            false,
            WorkflowBuildProfile::Debug,
        )?;

        assert!(
            bundle
                .manifest
                .modules
                .iter()
                .any(|module| !module.hash.as_str().is_empty())
        );
        assert!(!temp.path().join(".aos/cas").exists());
        assert!(temp.path().join(".aos/cache/modules").exists());
        Ok(())
    }
}
