use anyhow::{Context, Result};
use std::process::Command;

pub fn resolve_cargo() -> Result<std::path::PathBuf> {
    which::which("cargo").context("cargo executable not found")
}

pub fn spawn_command(mut cmd: Command) -> Result<std::process::Output> {
    let program = format!("{:?}", cmd);
    let output = cmd
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    Ok(output)
}
