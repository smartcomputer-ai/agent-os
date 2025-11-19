use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use aos_kernel::KernelConfig;
use aos_wasm_build::{BuildRequest, Builder};
use camino::Utf8PathBuf;
use log::debug;
use once_cell::sync::OnceCell;

pub fn reset_journal(example_root: &Path) -> Result<()> {
    let journal_dir = example_root.join(".aos").join("journal");
    if journal_dir.exists() {
        fs::remove_dir_all(&journal_dir)
            .with_context(|| format!("remove {}", journal_dir.display()))?;
    }
    Ok(())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(crate::workspace_root())
}

static FORCE_BUILD: OnceCell<bool> = OnceCell::new();

pub(crate) fn set_force_build(flag: bool) {
    let _ = FORCE_BUILD.set(flag);
}

fn force_build() -> bool {
    FORCE_BUILD.get().copied().unwrap_or(false)
}

pub fn compile_reducer(crate_rel: &str) -> Result<Vec<u8>> {
    let source_path = workspace_root().join(crate_rel);
    let utf_path = Utf8PathBuf::from_path_buf(source_path.clone())
        .map_err(|_| anyhow!("path is not utf-8: {}", source_path.display()))?;
    let example_root = source_path
        .parent()
        .ok_or_else(|| anyhow!("reducer path missing parent: {}", source_path.display()))?;
    let cache_dir = example_root.join(".aos").join("cache").join("modules");
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;
    let mut request = BuildRequest::new(utf_path);
    request.config.release = false;
    request.cache_dir = Some(cache_dir);
    if force_build() {
        debug!("forcing rebuild for {crate_rel}");
        request.use_cache = false;
    }
    let artifact = Builder::compile(request).context("compile reducer via aos-wasm-build")?;
    debug!(
        "build result: {} ({} bytes)",
        hex::encode(&artifact.wasm_hash.0),
        artifact.wasm_bytes.len()
    );
    Ok(artifact.wasm_bytes)
}

pub fn kernel_config(example_root: &Path) -> Result<KernelConfig> {
    let cache_dir = example_root.join(".aos").join("cache").join("wasmtime");
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;
    Ok(KernelConfig {
        module_cache_dir: Some(cache_dir),
        eager_module_load: true,
    })
}
