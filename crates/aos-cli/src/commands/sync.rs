use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SyncConfig {
    pub version: u32,
    #[serde(default)]
    pub air: Option<AirSync>,
    #[serde(default)]
    pub build: Option<BuildSync>,
    #[serde(default)]
    pub modules: Option<ModulesSync>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceSync>,
}

#[derive(Debug, Deserialize)]
pub struct AirSync {
    pub dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub struct BuildSync {
    pub reducer_dir: Option<PathBuf>,
    pub module: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ModulesSync {
    pub pull: Option<bool>,
    pub dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct WorkspaceSync {
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(rename = "dir", alias = "local_dir")]
    pub dir: PathBuf,
    #[serde(default)]
    pub ignore: Vec<String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
}

pub fn load_sync_config(world_root: &Path, map: Option<&Path>) -> Result<(PathBuf, SyncConfig)> {
    let path = match map {
        Some(path) if path.is_relative() => world_root.join(path),
        Some(path) => path.to_path_buf(),
        None => world_root.join("aos.sync.json"),
    };
    let bytes = std::fs::read(&path)
        .with_context(|| format!("read sync config {}", path.display()))?;
    let config: SyncConfig = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse sync config {}", path.display()))?;
    if config.version != 1 {
        anyhow::bail!("unsupported sync config version {}", config.version);
    }
    Ok((path, config))
}
