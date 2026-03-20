use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CliConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, ProfileConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileConfig {
    #[serde(default)]
    pub kind: ProfileKind,
    pub api: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_env: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub universe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    Local,
    #[default]
    Remote,
}

impl ProfileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }
}

impl FromStr for ProfileKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "local" => Ok(Self::Local),
            "remote" => Ok(Self::Remote),
            _ => Err(anyhow!(
                "invalid profile kind '{value}'; expected local or remote"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub path: PathBuf,
}

impl ConfigPaths {
    pub fn resolve(explicit: Option<&Path>) -> Result<Self> {
        let path = if let Some(path) = explicit {
            path.to_path_buf()
        } else if let Ok(path) = std::env::var("AOS_CONFIG") {
            PathBuf::from(path)
        } else if let Ok(path) = std::env::var("AOS_FDB_CONFIG") {
            PathBuf::from(path)
        } else if let Some((preferred, legacy)) = default_config_paths() {
            if preferred.exists() || !legacy.exists() {
                preferred
            } else {
                legacy
            }
        } else {
            return Err(anyhow!(
                "could not resolve config path; set --config, AOS_CONFIG, AOS_FDB_CONFIG, XDG_CONFIG_HOME, or HOME"
            ));
        };
        Ok(Self { path })
    }
}

fn default_config_paths() -> Option<(PathBuf, PathBuf)> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let root = PathBuf::from(xdg).join("aos");
        return Some((root.join("cli.json"), root.join("fdb.json")));
    }
    if let Ok(home) = std::env::var("HOME") {
        let root = PathBuf::from(home).join(".config/aos");
        return Some((root.join("cli.json"), root.join("fdb.json")));
    }
    None
}

pub fn load_config(paths: &ConfigPaths) -> Result<CliConfig> {
    if !paths.path.exists() {
        return Ok(CliConfig::default());
    }
    let bytes =
        fs::read(&paths.path).with_context(|| format!("read config {}", paths.path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse config {}", paths.path.display()))
}

pub fn save_config(paths: &ConfigPaths, config: &CliConfig) -> Result<()> {
    if let Some(parent) = paths.path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create config directory {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(config).context("encode config json")?;
    fs::write(&paths.path, bytes).with_context(|| format!("write config {}", paths.path.display()))
}

pub fn redact_profile(profile: &ProfileConfig) -> serde_json::Value {
    serde_json::json!({
        "kind": profile.kind.as_str(),
        "api": profile.api,
        "token": profile.token.as_ref().map(|_| "***"),
        "token_env": profile.token_env,
        "headers": profile.headers,
        "universe": profile.universe,
        "world": profile.world,
    })
}

#[cfg(test)]
mod tests {
    use super::{ProfileConfig, ProfileKind};

    #[test]
    fn legacy_profile_without_kind_defaults_to_remote() {
        let profile: ProfileConfig = serde_json::from_str(
            r#"{
                "api": "http://127.0.0.1:9080",
                "universe": "local"
            }"#,
        )
        .expect("decode legacy profile");
        assert_eq!(profile.kind, ProfileKind::Remote);
    }

    #[test]
    fn profile_kind_serializes_as_snake_case() {
        let profile = ProfileConfig {
            kind: ProfileKind::Local,
            api: "http://127.0.0.1:9080".into(),
            token: None,
            token_env: None,
            headers: Default::default(),
            universe: Some("local".into()),
            world: None,
        };
        let json = serde_json::to_value(&profile).expect("encode profile");
        assert_eq!(
            json.get("kind").and_then(serde_json::Value::as_str),
            Some("local")
        );
    }
}
