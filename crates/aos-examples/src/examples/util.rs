use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use aos_wasm_build::{BuildRequest, Builder};
use camino::Utf8PathBuf;
use once_cell::sync::OnceCell;

pub fn reset_journal(example_root: &Path) -> Result<()> {
    let journal_dir = example_root.join(".aos").join("journal");
    if journal_dir.exists() {
        fs::remove_dir_all(&journal_dir)
            .with_context(|| format!("remove {}", journal_dir.display()))?;
    }
    Ok(())
}

pub fn ensure_wasm_artifact(
    crate_manifest_rel: &str,
    artifact_rel: &str,
    crate_label: &str,
) -> Result<Vec<u8>> {
    let wasm_path = workspace_root().join(artifact_rel);
    if !wasm_path.exists() {
        build_wasm(crate_manifest_rel, crate_label)?;
    }
    fs::read(&wasm_path).with_context(|| format!("read {}", wasm_path.display()))
}

fn build_wasm(crate_manifest_rel: &str, label: &str) -> Result<()> {
    println!("   compiling {label} reducer (wasm32-unknown-unknown)…");
    let manifest_path = workspace_root().join(crate_manifest_rel);
    let status = Command::new("cargo")
        .args(["build", "--target", "wasm32-unknown-unknown"])
        .arg("--manifest-path")
        .arg(&manifest_path)
        .status()
        .with_context(|| format!("spawn cargo build for {}", manifest_path.display()))?;
    if !status.success() {
        return Err(anyhow!("cargo build failed for {label} reducer"));
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
    let mut request = BuildRequest::new(utf_path);
    request.config.release = false;
    if force_build() {
        println!("   forcing rebuild for {crate_rel}");
        request.use_cache = false;
    }
    let artifact = Builder::compile(request).context("compile reducer via aos-wasm-build")?;
    println!(
        "   build result: {} ({} bytes)",
        hex::encode(&artifact.wasm_hash.0),
        artifact.wasm_bytes.len()
    );
    Ok(artifact.wasm_bytes)
}
