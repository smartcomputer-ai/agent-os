//! CLI utility functions for reducer compilation and kernel configuration.

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use aos_air_types::HashRef;
use aos_host::config::HostConfig;
use aos_host::util::{is_placeholder_hash, patch_modules};
use aos_kernel::{KernelConfig, LoadedManifest};
use aos_store::{FsStore, Store};
use aos_wasm_build::{BuildRequest, Builder};
use camino::Utf8PathBuf;
use dotenvy;
use jsonschema::JSONSchema;
use serde_json::Value;

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

/// Patch module hashes in a loaded manifest.
///
/// If `specific_module` is provided, only that module is patched.
/// Otherwise, all modules with placeholder hashes are patched.
pub fn patch_module_hashes(
    loaded: &mut LoadedManifest,
    wasm_hash: &HashRef,
    specific_module: Option<&str>,
) -> Result<usize> {
    let patched = match specific_module {
        Some(target) => patch_modules(loaded, wasm_hash, |name, _| name == target),
        None => patch_modules(loaded, wasm_hash, |_, m| is_placeholder_hash(m)),
    };

    if let Some(target) = specific_module {
        if patched == 0 {
            anyhow::bail!("module '{}' not found in manifest", target);
        }
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

pub fn host_config_from_env_and_overrides(
    http_timeout_ms: Option<u64>,
    http_max_body_bytes: Option<usize>,
) -> HostConfig {
    let mut cfg = HostConfig::from_env();

    if let Some(ms) = env_u64("AOS_HTTP_TIMEOUT_MS").or(http_timeout_ms) {
        cfg.http.timeout = Duration::from_millis(ms);
    }
    if let Some(bytes) = env_usize("AOS_HTTP_MAX_BODY_BYTES").or(http_max_body_bytes) {
        cfg.http.max_body_size = bytes;
    }

    let disable_llm_env = env_bool("AOS_DISABLE_LLM").unwrap_or(false);
    if disable_llm_env {
        cfg.llm = None;
    }

    cfg
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok().and_then(|v| v.parse().ok())
}

fn env_usize(key: &str) -> Option<usize> {
    std::env::var(key).ok().and_then(|v| v.parse().ok())
}

fn env_bool(key: &str) -> Option<bool> {
    std::env::var(key)
        .ok()
        .and_then(|v| match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
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
