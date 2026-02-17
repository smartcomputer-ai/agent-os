use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

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
    #[serde(default)]
    pub imports: Vec<AirImport>,
}

#[derive(Debug, Deserialize)]
pub struct AirImport {
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub cargo: Option<CargoAirImport>,
    #[allow(dead_code)]
    #[serde(default)]
    pub lock: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CargoAirImport {
    pub package: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub air_dir: Option<PathBuf>,
    #[serde(default)]
    pub manifest_path: Option<PathBuf>,
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
    let bytes =
        std::fs::read(&path).with_context(|| format!("read sync config {}", path.display()))?;
    let config: SyncConfig = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse sync config {}", path.display()))?;
    if config.version != 1 {
        anyhow::bail!("unsupported sync config version {}", config.version);
    }
    Ok((path, config))
}

#[derive(Debug, Clone)]
pub struct ResolvedAirSources {
    pub air_dir: PathBuf,
    pub import_dirs: Vec<PathBuf>,
}

pub fn resolve_air_sources(
    world_root: &Path,
    map_root: &Path,
    config: &SyncConfig,
    default_air_dir: &Path,
    default_reducer_dir: &Path,
) -> Result<ResolvedAirSources> {
    let air_dir = config
        .air
        .as_ref()
        .and_then(|air| air.dir.as_ref())
        .map(|dir| resolve_map_path(map_root, dir))
        .unwrap_or_else(|| default_air_dir.to_path_buf());

    let mut metadata_cache: HashMap<PathBuf, CargoMetadata> = HashMap::new();
    let mut import_dirs = Vec::new();
    if let Some(air) = &config.air {
        for import in &air.imports {
            let root = resolve_air_import_root(
                world_root,
                map_root,
                default_reducer_dir,
                import,
                &mut metadata_cache,
            )?;
            import_dirs.push(root);
        }
    }

    Ok(ResolvedAirSources {
        air_dir,
        import_dirs,
    })
}

fn resolve_air_import_root(
    world_root: &Path,
    map_root: &Path,
    default_reducer_dir: &Path,
    import: &AirImport,
    metadata_cache: &mut HashMap<PathBuf, CargoMetadata>,
) -> Result<PathBuf> {
    match (&import.path, &import.cargo) {
        (Some(path), None) => Ok(resolve_map_path(map_root, path)),
        (None, Some(cargo)) => resolve_cargo_import(
            world_root,
            map_root,
            default_reducer_dir,
            cargo,
            metadata_cache,
        ),
        (Some(_), Some(_)) => {
            anyhow::bail!("air.imports entry must set exactly one of 'path' or 'cargo'")
        }
        (None, None) => anyhow::bail!("air.imports entry must set one of 'path' or 'cargo'"),
    }
}

fn resolve_cargo_import(
    world_root: &Path,
    map_root: &Path,
    default_reducer_dir: &Path,
    import: &CargoAirImport,
    metadata_cache: &mut HashMap<PathBuf, CargoMetadata>,
) -> Result<PathBuf> {
    let manifest_path = match import.manifest_path.as_ref() {
        Some(path) => resolve_map_path(map_root, path),
        None => default_metadata_manifest(world_root, default_reducer_dir)?,
    };

    let metadata = if let Some(existing) = metadata_cache.get(&manifest_path) {
        existing
    } else {
        let loaded = load_cargo_metadata(&manifest_path)?;
        metadata_cache.insert(manifest_path.clone(), loaded);
        metadata_cache
            .get(&manifest_path)
            .expect("metadata just inserted")
    };

    let mut candidates: Vec<&CargoMetadataPackage> = metadata
        .packages
        .iter()
        .filter(|pkg| pkg.name == import.package)
        .collect();

    if let Some(version) = import.version.as_ref() {
        let normalized = normalize_version(version);
        candidates.retain(|pkg| pkg.version == normalized);
    }
    if let Some(source) = import.source.as_ref() {
        candidates.retain(|pkg| pkg.source.as_deref() == Some(source.as_str()));
    }

    if candidates.is_empty() {
        anyhow::bail!(
            "air.imports cargo package '{}' not found via {}",
            import.package,
            manifest_path.display()
        );
    }
    if candidates.len() > 1 {
        let mut choices = candidates
            .iter()
            .map(|pkg| format!("{}{}", pkg.version, pkg.source.as_deref().unwrap_or("")))
            .collect::<Vec<_>>();
        choices.sort();
        anyhow::bail!(
            "air.imports cargo package '{}' is ambiguous (candidates: {}); set cargo.version and/or cargo.source",
            import.package,
            choices.join(", ")
        );
    }

    let package = candidates[0];
    let package_root = PathBuf::from(&package.manifest_path)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "invalid cargo metadata manifest path '{}'",
                package.manifest_path
            )
        })?;
    let air_dir = import
        .air_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("air"));
    Ok(package_root.join(air_dir))
}

fn default_metadata_manifest(world_root: &Path, default_reducer_dir: &Path) -> Result<PathBuf> {
    let reducer_manifest = default_reducer_dir.join("Cargo.toml");
    if reducer_manifest.exists() {
        return Ok(reducer_manifest);
    }
    let world_manifest = world_root.join("Cargo.toml");
    if world_manifest.exists() {
        return Ok(world_manifest);
    }
    anyhow::bail!(
        "cargo air import requires Cargo.toml; checked {} and {}",
        reducer_manifest.display(),
        world_manifest.display()
    )
}

fn load_cargo_metadata(manifest_path: &Path) -> Result<CargoMetadata> {
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(manifest_path)
        .output()
        .with_context(|| format!("run cargo metadata for {}", manifest_path.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "cargo metadata failed for {}: {}",
            manifest_path.display(),
            stderr.trim()
        );
    }
    let metadata: CargoMetadata =
        serde_json::from_slice(&output.stdout).context("parse cargo metadata json")?;
    Ok(metadata)
}

fn normalize_version(version: &str) -> String {
    version.trim().trim_start_matches('=').to_string()
}

fn resolve_map_path(map_root: &Path, path: &Path) -> PathBuf {
    if path.is_relative() {
        map_root.join(path)
    } else {
        path.to_path_buf()
    }
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoMetadataPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataPackage {
    name: String,
    version: String,
    source: Option<String>,
    manifest_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_config() -> SyncConfig {
        SyncConfig {
            version: 1,
            air: None,
            build: None,
            modules: None,
            workspaces: Vec::new(),
        }
    }

    #[test]
    fn resolve_air_sources_path_import_is_map_relative() {
        let world_root = PathBuf::from("/tmp/world");
        let map_root = world_root.clone();
        let reducer_dir = world_root.join("reducer");
        let default_air = world_root.join("air-default");

        let mut config = empty_config();
        config.air = Some(AirSync {
            dir: Some(PathBuf::from("air")),
            imports: vec![AirImport {
                path: Some(PathBuf::from("../sdk/air")),
                cargo: None,
                lock: None,
            }],
        });

        let resolved =
            resolve_air_sources(&world_root, &map_root, &config, &default_air, &reducer_dir)
                .expect("resolve");
        assert_eq!(resolved.air_dir, world_root.join("air"));
        assert_eq!(resolved.import_dirs, vec![world_root.join("../sdk/air")]);
    }

    #[test]
    fn resolve_air_sources_rejects_invalid_import_shape() {
        let world_root = PathBuf::from("/tmp/world");
        let map_root = world_root.clone();
        let reducer_dir = world_root.join("reducer");
        let default_air = world_root.join("air-default");

        let mut config = empty_config();
        config.air = Some(AirSync {
            dir: Some(PathBuf::from("air")),
            imports: vec![AirImport {
                path: Some(PathBuf::from("../sdk/air")),
                cargo: Some(CargoAirImport {
                    package: "aos-agent-sdk".into(),
                    version: None,
                    source: None,
                    air_dir: None,
                    manifest_path: None,
                }),
                lock: None,
            }],
        });

        let err = resolve_air_sources(&world_root, &map_root, &config, &default_air, &reducer_dir)
            .expect_err("expected error");
        assert!(err.to_string().contains("exactly one"));
    }

    #[test]
    fn resolve_air_sources_cargo_import_finds_workspace_package() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let world_root = workspace_root.clone();
        let map_root = workspace_root.clone();
        let reducer_dir = workspace_root.join("apps/demiurge/reducer");
        let default_air = workspace_root.join("air");

        let mut config = empty_config();
        config.air = Some(AirSync {
            dir: Some(PathBuf::from("apps/demiurge/air")),
            imports: vec![AirImport {
                path: None,
                cargo: Some(CargoAirImport {
                    package: "aos-agent-sdk".into(),
                    version: None,
                    source: None,
                    air_dir: Some(PathBuf::from("air/exports/session-contracts")),
                    manifest_path: Some(PathBuf::from("Cargo.toml")),
                }),
                lock: None,
            }],
        });

        let resolved =
            resolve_air_sources(&world_root, &map_root, &config, &default_air, &reducer_dir)
                .expect("resolve");
        let actual = std::fs::canonicalize(&resolved.import_dirs[0]).expect("canonical actual");
        let expected = std::fs::canonicalize(
            workspace_root.join("crates/aos-agent-sdk/air/exports/session-contracts"),
        )
        .expect("canonical expected");
        assert_eq!(actual, expected);
    }
}
