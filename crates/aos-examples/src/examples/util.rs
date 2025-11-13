use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

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
