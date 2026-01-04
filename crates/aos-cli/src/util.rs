//! CLI utility functions for reducer compilation and kernel configuration.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_host::config::HostConfig;
use aos_host::util::is_placeholder_hash;
use aos_kernel::{KernelConfig, LoadedManifest};
use aos_store::{FsStore, Store};
use aos_wasm_build::{BuildRequest, Builder};
use camino::Utf8PathBuf;
use dotenvy;
use jsonschema::JSONSchema;
use serde_json::Value;
use walkdir::WalkDir;

/// Compile a reducer crate to WASM and store the blob.
pub fn compile_reducer(
    reducer_dir: &Path,
    store_root: &Path,
    store: &FsStore,
    force_build: bool,
) -> Result<HashRef> {
    let cache_dir = store_root.join(".aos/cache/modules");
    fs::create_dir_all(&cache_dir).context("create module cache directory")?;

    let utf_path = Utf8PathBuf::from_path_buf(reducer_dir.to_path_buf())
        .map_err(|p| anyhow!("reducer path is not UTF-8: {}", p.display()))?;

    let mut request = BuildRequest::new(utf_path);
    request.cache_dir = Some(cache_dir);
    request.use_cache = !force_build;
    request.config.release = false;

    let artifact = Builder::compile(request).context("compile reducer")?;
    let hash = store
        .put_blob(&artifact.wasm_bytes)
        .context("store wasm blob")?;
    HashRef::new(hash.to_hex()).context("create hash ref")
}

/// Resolve placeholder module hashes in a loaded manifest.
///
/// Resolution order:
/// 1) `modules/` directory in the world root (content-addressed wasm files)
/// 2) Known system modules from workspace build artifacts
/// 3) Compiled reducer hash (if provided) when exactly one non-sys placeholder remains
///
/// If `specific_module` is provided, that module is patched with the compiled hash
/// (and must currently be a placeholder).
pub fn resolve_placeholder_modules(
    loaded: &mut LoadedManifest,
    store: &FsStore,
    world_root: &Path,
    compiled_hash: Option<&HashRef>,
    specific_module: Option<&str>,
) -> Result<usize> {
    let mut patched = 0usize;

    if let Some(target) = specific_module {
        let Some(hash) = compiled_hash else {
            anyhow::bail!("--module requires a compiled reducer; no reducer/ found");
        };
        let mut found = false;
        for (name, module) in loaded.modules.iter_mut() {
            if name.as_str() == target {
                found = true;
                if !is_placeholder_hash(module) {
                    anyhow::bail!("module '{target}' already has a wasm_hash; remove it to patch");
                }
                module.wasm_hash = hash.clone();
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
        if let Some(hash) = resolve_from_world_modules(store, world_root, name.as_str())? {
            module.wasm_hash = hash;
            patched += 1;
            continue;
        }
        if let Some(spec) = sys_module_spec(name.as_str()) {
            match resolve_sys_module(store, world_root, spec)? {
                Some(hash) => {
                    module.wasm_hash = hash;
                    patched += 1;
                }
                None => {
                    unresolved_sys.push(name.to_string());
                }
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
                    module.wasm_hash = hash.clone();
                    patched += 1;
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
            msg.push_str("  - or provide a reducer/ to compile local modules\n");
        }
        anyhow::bail!(msg);
    }

    Ok(patched)
}

/// Create a kernel configuration for CLI usage.
///
/// Unlike the examples, CLI allows placeholder secrets and doesn't inject demo keys.
pub fn make_kernel_config(store_root: &Path) -> Result<KernelConfig> {
    let cache_dir = store_root.join(".aos/cache/wasmtime");
    fs::create_dir_all(&cache_dir).context("create wasmtime cache directory")?;
    Ok(KernelConfig {
        module_cache_dir: Some(cache_dir),
        eager_module_load: true,
        secret_resolver: None,
        allow_placeholder_secrets: true,
    })
}

/// Load .env from the world directory without overriding existing environment variables.
pub fn load_world_env(world_path: &Path) -> Result<()> {
    let env_path = world_path.join(".env");
    if env_path.exists() {
        for item in dotenvy::from_path_iter(&env_path).context("load .env")? {
            let (key, val) = item?;
            if std::env::var_os(&key).is_none() {
                unsafe {
                    std::env::set_var(&key, &val);
                }
            }
        }
    }
    Ok(())
}

pub fn host_config_from_opts(
    http_timeout_ms: Option<u64>,
    http_max_body_bytes: Option<usize>,
) -> HostConfig {
    // HostConfig::from_env already applies process env (including .env loaded earlier).
    let mut cfg = HostConfig::from_env();
    if let Some(ms) = http_timeout_ms {
        cfg.http.timeout = Duration::from_millis(ms);
    }
    if let Some(bytes) = http_max_body_bytes {
        cfg.http.max_body_size = bytes;
    }
    cfg
}

fn resolve_from_world_modules(
    store: &FsStore,
    world_root: &Path,
    module_name: &str,
) -> Result<Option<HashRef>> {
    let modules_dir = world_root.join("modules");
    if !modules_dir.exists() {
        return Ok(None);
    }

    let prefix = format!("{module_name}-");
    let mut matches: Vec<PathBuf> = Vec::new();

    for entry in WalkDir::new(&modules_dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("wasm") {
            continue;
        }
        let rel = path.strip_prefix(&modules_dir).unwrap_or(path);
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
    let rel = path.strip_prefix(&modules_dir).unwrap_or(path);
    let rel_str = rel.to_string_lossy();
    let rel_norm = rel_str.replace('\\', "/");
    let hash_str = rel_norm
        .strip_suffix(".wasm")
        .and_then(|s| s.strip_prefix(&prefix))
        .ok_or_else(|| {
            anyhow!("wasm filename does not match '{module_name}-<hash>.wasm' under modules/")
        })?;

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
        name: "sys/Workspace@1",
        bin: "workspace",
    },
    SysModuleSpec {
        name: "sys/CapEnforceWorkspace@1",
        bin: "cap_enforce_workspace",
    },
];

fn resolve_sys_module(
    store: &FsStore,
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
            persist_world_module(world_root, spec.name, hash_ref.as_str(), &bytes)?;
            return Ok(Some(hash_ref));
        }
    }

    Ok(None)
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

fn persist_world_module(
    world_root: &Path,
    module_name: &str,
    hash: &str,
    bytes: &[u8],
) -> Result<()> {
    let modules_dir = world_root.join("modules");
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

/// Validate a patch JSON document against patch.schema.json (and common schema refs).
pub fn validate_patch_json(doc: &Value) -> Result<()> {
    let patch_schema: Value =
        serde_json::from_str(aos_air_types::schemas::PATCH).context("load patch schema")?;
    let common_schema: Value =
        serde_json::from_str(aos_air_types::schemas::COMMON).context("load common schema")?;

    let mut opts = JSONSchema::options();
    opts.with_document("common.schema.json".to_string(), common_schema.clone());
    opts.with_document(
        "https://aos.dev/air/v1/common.schema.json".to_string(),
        common_schema,
    );
    // Leak the schema to give it 'static lifetime for jsonschema API.
    let leaked: &'static Value = Box::leak(Box::new(patch_schema));
    let compiled = opts.compile(leaked).context("compile patch schema")?;

    if let Err(errors) = compiled.validate(doc) {
        let msgs: Vec<String> = errors
            .map(|e| format!("{}: {}", e.instance_path, e))
            .collect();
        anyhow::bail!("patch schema validation failed: {}", msgs.join("; "));
    }
    Ok(())
}
