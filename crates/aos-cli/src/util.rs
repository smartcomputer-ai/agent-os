//! CLI utility functions for reducer compilation and kernel configuration.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use aos_air_types::HashRef;
use aos_host::util::{is_placeholder_hash, patch_modules};
use aos_kernel::{KernelConfig, LoadedManifest};
use aos_store::{FsStore, Store};
use aos_wasm_build::{BuildRequest, Builder};
use camino::Utf8PathBuf;

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
