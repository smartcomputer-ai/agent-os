use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use aos_air_types::AirNode;
use aos_cbor::Hash;
use dotenvy::from_path_iter;
use serde::Deserialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::generated::write_generated_air_from_cargo_export;
use crate::manifest_loader::AirSource;

const ZERO_HASH_SENTINEL: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Deserialize)]
pub struct WorldConfig {
    pub version: u32,
    #[serde(default)]
    pub air: Option<WorldAirConfig>,
    #[serde(default)]
    pub build: Option<BuildSync>,
    #[serde(default)]
    pub secrets: Option<SecretsSync>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceSync>,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            version: 1,
            air: None,
            build: None,
            secrets: None,
            workspaces: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct WorldAirConfig {
    #[serde(default)]
    pub dir: Option<PathBuf>,
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BuildSync {
    pub module_dir: Option<PathBuf>,
    pub module: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SecretsSync {
    #[serde(default)]
    pub sources: Vec<SecretSourceSync>,
    #[serde(default)]
    pub bindings: Vec<SecretBindingSync>,
}

#[derive(Debug, Deserialize)]
pub struct SecretSourceSync {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub struct SecretBindingSync {
    pub binding: String,
    pub from: SecretBindingSourceRef,
}

#[derive(Debug, Deserialize)]
pub struct SecretBindingSourceRef {
    pub source: String,
    pub key: String,
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

#[derive(Debug, Clone)]
pub struct ResolvedAirPackage {
    pub root: PathBuf,
    pub source: AirSource,
    pub package_name: String,
    pub version: String,
    pub source_id: Option<String>,
    pub manifest_path: PathBuf,
    pub air_dir: PathBuf,
    pub defs_hash: String,
    pub module_names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedAirSources {
    pub air_dir: PathBuf,
    pub module_dir: PathBuf,
    pub import_dirs: Vec<PathBuf>,
    pub sources: Vec<AirSource>,
    pub packages: Vec<ResolvedAirPackage>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSecretValue {
    pub binding: String,
    pub source: String,
    pub key: String,
    pub plaintext: Vec<u8>,
}

pub fn load_world_config(
    world_root: &Path,
    config_path: Option<&Path>,
) -> Result<(Option<PathBuf>, WorldConfig)> {
    let path = match config_path {
        Some(path) if path.is_relative() => world_root.join(path),
        Some(path) => path.to_path_buf(),
        None => world_root.join("aos.world.json"),
    };
    if !path.exists() && config_path.is_none() {
        return Ok((None, WorldConfig::default()));
    }
    let bytes =
        std::fs::read(&path).with_context(|| format!("read world config {}", path.display()))?;
    let config: WorldConfig = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse world config {}", path.display()))?;
    if config.version != 1 {
        anyhow::bail!("unsupported world config version {}", config.version);
    }
    Ok((Some(path), config))
}

pub fn resolve_world_air_sources(
    world_root: &Path,
    config_path: Option<&Path>,
    config: &WorldConfig,
    default_air_dir: &Path,
    default_module_dir: &Path,
) -> Result<ResolvedAirSources> {
    let config_root = config_path.and_then(Path::parent).unwrap_or(world_root);
    let air_dir = config
        .air
        .as_ref()
        .and_then(|air| air.dir.as_ref())
        .map(|dir| resolve_map_path(config_root, dir))
        .unwrap_or_else(|| default_air_dir.to_path_buf());
    let module_dir = config
        .build
        .as_ref()
        .and_then(|build| build.module_dir.as_ref())
        .map(|dir| resolve_map_path(config_root, dir))
        .unwrap_or_else(|| default_module_dir.to_path_buf());

    let mut sources = Vec::new();
    let mut import_dirs = Vec::new();
    let mut packages = Vec::new();
    let mut warnings = Vec::new();

    let local_export = local_export_bin_manifest(&module_dir);
    if let Some(manifest_path) = local_export {
        sources.push(AirSource::generated_rust_package(
            world_root,
            manifest_path,
            None,
            Some(crate::generated::DEFAULT_AIR_EXPORT_BIN.to_string()),
            true,
        ));
        if air_dir.exists() {
            sources.push(AirSource::Directory {
                path: air_dir.clone(),
                allow_manifest: false,
                include_root: false,
            });
        }
    } else if air_dir.exists() {
        sources.push(AirSource::local_directory(air_dir.clone()));
    }

    for package in discover_cargo_air_packages(world_root, &module_dir)? {
        warnings.push(format!(
            "discovered AIR package {} {} from {} (defs {})",
            package.package_name,
            package.version,
            package.root.display(),
            package.defs_hash
        ));
        sources.push(package.source.clone());
        import_dirs.push(package.root.clone());
        packages.push(package);
    }

    Ok(ResolvedAirSources {
        air_dir,
        module_dir,
        import_dirs,
        sources,
        packages,
        warnings,
    })
}

pub fn load_all_world_secret_values(
    world_root: &Path,
    config_path: Option<&Path>,
) -> Result<(Option<PathBuf>, WorldConfig, Vec<ResolvedSecretValue>)> {
    let (resolved_config_path, config) = load_world_config(world_root, config_path)?;
    let config_root = resolved_config_path
        .as_deref()
        .and_then(Path::parent)
        .unwrap_or(world_root);
    let values = resolve_world_secret_values(config_root, &config, None)?;
    Ok((resolved_config_path, config, values))
}

pub fn load_required_secret_value_map(
    world_root: &Path,
    config_path: Option<&Path>,
    required_bindings: &BTreeSet<String>,
) -> Result<HashMap<String, Vec<u8>>> {
    if required_bindings.is_empty() {
        return Ok(HashMap::new());
    }
    let resolved = match config_path {
        Some(path) => {
            let (resolved_config_path, config) = load_world_config(world_root, Some(path))?;
            let config_root = resolved_config_path
                .as_deref()
                .and_then(Path::parent)
                .unwrap_or(world_root);
            resolve_world_secret_values(config_root, &config, Some(required_bindings))?
        }
        None => {
            let default_map = world_root.join("aos.world.json");
            if default_map.exists() {
                let (config_path, config) = load_world_config(world_root, None)?;
                let config_root = config_path
                    .as_deref()
                    .and_then(Path::parent)
                    .unwrap_or(world_root);
                resolve_world_secret_values(config_root, &config, Some(required_bindings))?
            } else {
                Vec::new()
            }
        }
    };
    let mut values: HashMap<String, Vec<u8>> = resolved
        .into_iter()
        .map(|entry| (entry.binding, entry.plaintext))
        .collect();

    for binding in required_bindings {
        if values.contains_key(binding) {
            continue;
        }
        let Some(var_name) = binding.strip_prefix("env:") else {
            anyhow::bail!(
                "missing world config secret binding '{}' and no legacy env:VAR_NAME fallback applies",
                binding
            );
        };
        if var_name.is_empty() {
            anyhow::bail!("invalid empty env binding '{}'", binding);
        }
        let value = std::env::var(var_name).map_err(|_| {
            anyhow::anyhow!(
                "missing env var '{var_name}' required by secret binding '{}'",
                binding
            )
        })?;
        values.insert(binding.clone(), value.into_bytes());
    }
    Ok(values)
}

pub fn load_available_secret_value_map(
    world_root: &Path,
    config_path: Option<&Path>,
    required_bindings: &BTreeSet<String>,
) -> Result<HashMap<String, Vec<u8>>> {
    if required_bindings.is_empty() {
        return Ok(HashMap::new());
    }
    let resolved = match config_path {
        Some(path) => {
            let (resolved_config_path, config) = load_world_config(world_root, Some(path))?;
            let config_root = resolved_config_path
                .as_deref()
                .and_then(Path::parent)
                .unwrap_or(world_root);
            resolve_world_secret_values_allow_missing(
                config_root,
                &config,
                Some(required_bindings),
            )?
        }
        None => {
            let default_map = world_root.join("aos.world.json");
            if default_map.exists() {
                let (config_path, config) = load_world_config(world_root, None)?;
                let config_root = config_path
                    .as_deref()
                    .and_then(Path::parent)
                    .unwrap_or(world_root);
                resolve_world_secret_values_allow_missing(
                    config_root,
                    &config,
                    Some(required_bindings),
                )?
            } else {
                Vec::new()
            }
        }
    };
    let mut values: HashMap<String, Vec<u8>> = resolved
        .into_iter()
        .map(|entry| (entry.binding, entry.plaintext))
        .collect();

    for binding in required_bindings {
        if values.contains_key(binding) {
            continue;
        }
        let Some(var_name) = binding.strip_prefix("env:") else {
            continue;
        };
        if var_name.is_empty() {
            anyhow::bail!("invalid empty env binding '{}'", binding);
        }
        if let Ok(value) = std::env::var(var_name) {
            values.insert(binding.clone(), value.into_bytes());
        }
    }
    Ok(values)
}

fn resolve_world_secret_values(
    map_root: &Path,
    config: &WorldConfig,
    required_bindings: Option<&BTreeSet<String>>,
) -> Result<Vec<ResolvedSecretValue>> {
    resolve_secret_values_from_config(map_root, config.secrets.as_ref(), required_bindings)
}

fn resolve_world_secret_values_allow_missing(
    map_root: &Path,
    config: &WorldConfig,
    required_bindings: Option<&BTreeSet<String>>,
) -> Result<Vec<ResolvedSecretValue>> {
    resolve_secret_values_from_config_allow_missing(
        map_root,
        config.secrets.as_ref(),
        required_bindings,
    )
}

fn resolve_secret_values_from_config(
    map_root: &Path,
    secrets: Option<&SecretsSync>,
    required_bindings: Option<&BTreeSet<String>>,
) -> Result<Vec<ResolvedSecretValue>> {
    let Some(secrets) = secrets else {
        return Ok(Vec::new());
    };
    if secrets.bindings.is_empty() {
        return Ok(Vec::new());
    }

    let mut sources = HashMap::new();
    for source in &secrets.sources {
        let name = source.name.trim();
        if name.is_empty() {
            anyhow::bail!("world config secret source name must be non-empty");
        }
        if sources
            .insert(name.to_string(), load_secret_source(map_root, source)?)
            .is_some()
        {
            anyhow::bail!("duplicate world config secret source '{}'", name);
        }
    }

    let mut seen_bindings = HashSet::new();
    let mut values = Vec::new();
    for binding in &secrets.bindings {
        let binding_id = binding.binding.trim();
        if binding_id.is_empty() {
            anyhow::bail!("world config secret binding id must be non-empty");
        }
        if !seen_bindings.insert(binding_id.to_string()) {
            anyhow::bail!("duplicate world config secret binding '{}'", binding_id);
        }
        if let Some(required) = required_bindings {
            if !required.contains(binding_id) {
                continue;
            }
        }
        let source = sources.get(binding.from.source.as_str()).ok_or_else(|| {
            anyhow::anyhow!(
                "world config secret binding '{}' references unknown source '{}'",
                binding_id,
                binding.from.source
            )
        })?;
        let plaintext = source
            .resolve(binding.from.key.as_str())
            .with_context(|| format!("resolve world config secret binding '{}'", binding_id))?;
        values.push(ResolvedSecretValue {
            binding: binding_id.to_string(),
            source: binding.from.source.clone(),
            key: binding.from.key.clone(),
            plaintext,
        });
    }

    Ok(values)
}

fn resolve_secret_values_from_config_allow_missing(
    map_root: &Path,
    secrets: Option<&SecretsSync>,
    required_bindings: Option<&BTreeSet<String>>,
) -> Result<Vec<ResolvedSecretValue>> {
    let Some(secrets) = secrets else {
        return Ok(Vec::new());
    };
    if secrets.bindings.is_empty() {
        return Ok(Vec::new());
    }

    let mut sources = HashMap::new();
    for source in &secrets.sources {
        let name = source.name.trim();
        if name.is_empty() {
            anyhow::bail!("world config secret source name must be non-empty");
        }
        if sources
            .insert(
                name.to_string(),
                load_secret_source_allow_missing(map_root, source)?,
            )
            .is_some()
        {
            anyhow::bail!("duplicate world config secret source '{}'", name);
        }
    }

    let mut seen_bindings = HashSet::new();
    let mut values = Vec::new();
    for binding in &secrets.bindings {
        let binding_id = binding.binding.trim();
        if binding_id.is_empty() {
            anyhow::bail!("world config secret binding id must be non-empty");
        }
        if !seen_bindings.insert(binding_id.to_string()) {
            anyhow::bail!("duplicate world config secret binding '{}'", binding_id);
        }
        if let Some(required) = required_bindings {
            if !required.contains(binding_id) {
                continue;
            }
        }
        let source = sources.get(binding.from.source.as_str()).ok_or_else(|| {
            anyhow::anyhow!(
                "world config secret binding '{}' references unknown source '{}'",
                binding_id,
                binding.from.source
            )
        })?;
        let Some(plaintext) = source
            .maybe_resolve(binding.from.key.as_str())
            .with_context(|| format!("resolve world config secret binding '{}'", binding_id))?
        else {
            continue;
        };
        values.push(ResolvedSecretValue {
            binding: binding_id.to_string(),
            source: binding.from.source.clone(),
            key: binding.from.key.clone(),
            plaintext,
        });
    }

    Ok(values)
}

enum LoadedSecretSource {
    Dotenv {
        path: PathBuf,
        values: HashMap<String, Vec<u8>>,
    },
    Env,
}

impl LoadedSecretSource {
    fn resolve(&self, key: &str) -> Result<Vec<u8>> {
        let trimmed = key.trim();
        if trimmed.is_empty() {
            anyhow::bail!("secret source key must be non-empty");
        }
        match self {
            LoadedSecretSource::Dotenv { path, values } => values
                .get(trimmed)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing key '{}' in {}", trimmed, path.display())),
            LoadedSecretSource::Env => std::env::var(trimmed)
                .map(|value| value.into_bytes())
                .map_err(|_| anyhow::anyhow!("missing env var '{}'", trimmed)),
        }
    }

    fn maybe_resolve(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let trimmed = key.trim();
        if trimmed.is_empty() {
            anyhow::bail!("secret source key must be non-empty");
        }
        match self {
            LoadedSecretSource::Dotenv { values, .. } => Ok(std::env::var(trimmed)
                .ok()
                .map(|value| value.into_bytes())
                .or_else(|| values.get(trimmed).cloned())),
            LoadedSecretSource::Env => {
                Ok(std::env::var(trimmed).ok().map(|value| value.into_bytes()))
            }
        }
    }
}

fn load_secret_source(map_root: &Path, source: &SecretSourceSync) -> Result<LoadedSecretSource> {
    let kind = source.kind.trim();
    if kind.is_empty() {
        anyhow::bail!("world config secret source '{}' must set kind", source.name);
    }
    match kind {
        "dotenv" => {
            let path = source
                .path
                .as_ref()
                .map(|path| resolve_map_path(map_root, path))
                .unwrap_or_else(|| map_root.join(".env"));
            let mut values = HashMap::new();
            for item in from_path_iter(&path)
                .with_context(|| format!("load dotenv secret source {}", path.display()))?
            {
                let (key, value) = item?;
                values.insert(key, value.into_bytes());
            }
            Ok(LoadedSecretSource::Dotenv { path, values })
        }
        "env" => Ok(LoadedSecretSource::Env),
        other => anyhow::bail!(
            "unsupported world config secret source kind '{}' for source '{}'",
            other,
            source.name
        ),
    }
}

fn load_secret_source_allow_missing(
    map_root: &Path,
    source: &SecretSourceSync,
) -> Result<LoadedSecretSource> {
    let kind = source.kind.trim();
    if kind.is_empty() {
        anyhow::bail!("world config secret source '{}' must set kind", source.name);
    }
    match kind {
        "dotenv" => {
            let path = source
                .path
                .as_ref()
                .map(|path| resolve_map_path(map_root, path))
                .unwrap_or_else(|| map_root.join(".env"));
            let mut values = HashMap::new();
            if path.exists() {
                for item in from_path_iter(&path)
                    .with_context(|| format!("load dotenv secret source {}", path.display()))?
                {
                    let (key, value) = item?;
                    values.insert(key, value.into_bytes());
                }
            }
            Ok(LoadedSecretSource::Dotenv { path, values })
        }
        "env" => Ok(LoadedSecretSource::Env),
        other => anyhow::bail!(
            "unsupported world config secret source kind '{}' for source '{}'",
            other,
            source.name
        ),
    }
}

fn discover_cargo_air_packages(
    world_root: &Path,
    module_dir: &Path,
) -> Result<Vec<ResolvedAirPackage>> {
    let manifest_path = match default_metadata_manifest(world_root, module_dir) {
        Ok(path) => path,
        Err(_) => return Ok(Vec::new()),
    };
    let metadata = load_cargo_metadata(&manifest_path)?;
    let direct_dependency_ids = direct_dependency_package_ids(&metadata)?;
    let mut packages = Vec::new();

    for package_id in direct_dependency_ids {
        let Some(package) = metadata.packages.iter().find(|pkg| pkg.id == package_id) else {
            continue;
        };
        let Some(aos) = package.aos_metadata() else {
            continue;
        };
        if !aos.exports.unwrap_or(false) && aos.air.is_none() {
            continue;
        }
        let package_root = PathBuf::from(&package.manifest_path)
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid cargo metadata manifest path '{}'",
                    package.manifest_path
                )
            })?;
        let air_dir = aos.air_dir.clone().unwrap_or_else(|| PathBuf::from("air"));
        let root = package_root.join(&air_dir);
        let generated = aos.air.as_deref() == Some("generated")
            || (aos.exports.unwrap_or(false) && aos.export_bin.is_some());
        let export_bin = aos.export_bin.clone();
        if generated {
            let package_manifest_path = PathBuf::from(&package.manifest_path);
            write_generated_air_from_cargo_export(
                &package_root,
                &package_manifest_path,
                None,
                export_bin.as_deref(),
            )
            .with_context(|| {
                format!(
                    "materialize generated AIR for discovered cargo package '{}'",
                    package.name
                )
            })?;
        }
        let defs_hash = import_defs_hash(&root)?;
        let module_names = import_module_names(&root)?;
        packages.push(ResolvedAirPackage {
            root: root.clone(),
            source: AirSource::imported_directory(root),
            package_name: package.name.clone(),
            version: package.version.clone(),
            source_id: package.source.clone(),
            manifest_path: PathBuf::from(&package.manifest_path),
            air_dir,
            defs_hash,
            module_names,
        });
    }

    packages.sort_by(|left, right| {
        left.package_name
            .cmp(&right.package_name)
            .then_with(|| left.version.cmp(&right.version))
            .then_with(|| left.root.cmp(&right.root))
    });
    Ok(packages)
}

fn direct_dependency_package_ids(metadata: &CargoMetadata) -> Result<BTreeSet<String>> {
    let Some(resolve) = metadata.resolve.as_ref() else {
        return Ok(BTreeSet::new());
    };
    let root_id = resolve.root.as_ref().cloned().or_else(|| {
        metadata
            .packages
            .iter()
            .find(|package| package.source.is_none())
            .map(|package| package.id.clone())
    });
    let Some(root_id) = root_id else {
        return Ok(BTreeSet::new());
    };
    let Some(root_node) = resolve.nodes.iter().find(|node| node.id == root_id) else {
        return Ok(BTreeSet::new());
    };
    Ok(root_node.dependencies.iter().cloned().collect())
}

fn local_export_bin_manifest(module_dir: &Path) -> Option<PathBuf> {
    let manifest_path = module_dir.join("Cargo.toml");
    let export_bin = module_dir
        .join("src")
        .join("bin")
        .join(format!("{}.rs", crate::generated::DEFAULT_AIR_EXPORT_BIN));
    if manifest_path.exists() && export_bin.exists() {
        Some(manifest_path)
    } else {
        None
    }
}

fn import_defs_hash(root: &Path) -> Result<String> {
    let mut entries: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut seen: HashMap<(String, String), String> = HashMap::new();

    for path in collect_json_files(root)? {
        for node in parse_air_nodes_for_import_hash(&path)
            .with_context(|| format!("parse AIR nodes from {}", path.display()))?
        {
            match node {
                AirNode::Manifest(_) => {
                    // Import roots may include authoring manifests; they do not participate in
                    // imported defs identity.
                    continue;
                }
                AirNode::Defschema(schema) => {
                    let name = schema.name.clone();
                    let node = AirNode::Defschema(schema);
                    add_def_entry(&mut entries, &mut seen, "defschema", name.as_str(), &node)?;
                }
                AirNode::Defmodule(module) => {
                    let name = module.name.clone();
                    let node = AirNode::Defmodule(module);
                    add_def_entry(&mut entries, &mut seen, "defmodule", name.as_str(), &node)?;
                }
                AirNode::Defsecret(secret) => {
                    let name = secret.name.clone();
                    let node = AirNode::Defsecret(secret);
                    add_def_entry(&mut entries, &mut seen, "defsecret", name.as_str(), &node)?;
                }
                AirNode::Defworkflow(workflow) => {
                    let name = workflow.name.clone();
                    let node = AirNode::Defworkflow(workflow);
                    add_def_entry(&mut entries, &mut seen, "defworkflow", name.as_str(), &node)?;
                }
                AirNode::Defeffect(effect) => {
                    let name = effect.name.clone();
                    let node = AirNode::Defeffect(effect);
                    add_def_entry(&mut entries, &mut seen, "defeffect", name.as_str(), &node)?;
                }
            }
        }
    }

    let digest = Hash::of_cbor(&entries)
        .map_err(|err| anyhow::anyhow!("hash import defs: {err}"))?
        .to_hex();
    Ok(digest)
}

fn import_module_names(root: &Path) -> Result<Vec<String>> {
    let mut names = BTreeSet::new();
    for path in collect_json_files(root)? {
        for node in parse_air_nodes_for_import_hash(&path)
            .with_context(|| format!("parse AIR nodes from {}", path.display()))?
        {
            if let AirNode::Defmodule(module) = node {
                names.insert(module.name);
            }
        }
    }
    Ok(names.into_iter().collect())
}

fn add_def_entry(
    entries: &mut BTreeSet<(String, String, String)>,
    seen: &mut HashMap<(String, String), String>,
    kind: &str,
    name: &str,
    node: &AirNode,
) -> Result<()> {
    let hash = Hash::of_cbor(node)
        .map_err(|err| anyhow::anyhow!("hash {kind} '{name}': {err}"))?
        .to_hex();
    let key = (kind.to_string(), name.to_string());
    if let Some(existing) = seen.get(&key) {
        if existing != &hash {
            anyhow::bail!(
                "duplicate {kind} '{}' has conflicting definitions ({}, {})",
                name,
                existing,
                hash
            );
        }
    } else {
        seen.insert(key, hash.clone());
    }
    entries.insert((kind.to_string(), name.to_string(), hash));
    Ok(())
}

fn default_metadata_manifest(world_root: &Path, default_module_dir: &Path) -> Result<PathBuf> {
    let module_manifest = default_module_dir.join("Cargo.toml");
    if module_manifest.exists() {
        return Ok(module_manifest);
    }
    let world_manifest = world_root.join("Cargo.toml");
    if world_manifest.exists() {
        return Ok(world_manifest);
    }
    anyhow::bail!(
        "cargo AIR discovery requires Cargo.toml; checked {} and {}",
        module_manifest.display(),
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
    #[serde(default)]
    resolve: Option<CargoMetadataResolve>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataPackage {
    id: String,
    name: String,
    version: String,
    source: Option<String>,
    manifest_path: String,
    #[serde(default)]
    metadata: Option<CargoPackageMetadata>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataResolve {
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    nodes: Vec<CargoMetadataResolveNode>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataResolveNode {
    id: String,
    #[serde(default)]
    dependencies: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoPackageMetadata {
    #[serde(default)]
    aos: Option<AosPackageMetadata>,
}

#[derive(Debug, Deserialize)]
struct AosPackageMetadata {
    #[serde(default)]
    air: Option<String>,
    #[serde(default)]
    air_dir: Option<PathBuf>,
    #[serde(default)]
    exports: Option<bool>,
    #[serde(default)]
    export_bin: Option<String>,
}

impl CargoMetadataPackage {
    fn aos_metadata(&self) -> Option<&AosPackageMetadata> {
        self.metadata
            .as_ref()
            .and_then(|metadata| metadata.aos.as_ref())
    }
}

fn collect_json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(dir) {
        let entry = entry.context("walk import directory")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if matches!(path.extension().and_then(|s| s.to_str()), Some(ext) if ext.eq_ignore_ascii_case("json"))
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn parse_air_nodes_for_import_hash(path: &Path) -> Result<Vec<AirNode>> {
    let data = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let trimmed = data.trim_start();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let mut value: Value = serde_json::from_str(&data).context("parse AIR json")?;
    normalize_authoring_hashes(&mut value);
    let items = match value {
        Value::Array(items) => items,
        other => vec![other],
    };

    let mut nodes = Vec::new();
    for item in items {
        let node = serde_json::from_value::<AirNode>(item).context("deserialize AIR node")?;
        nodes.push(node);
    }
    Ok(nodes)
}

fn normalize_authoring_hashes(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                normalize_authoring_hashes(item);
            }
        }
        Value::Object(map) => {
            if let Some(Value::String(kind)) = map.get("$kind") {
                match kind.as_str() {
                    "manifest" => normalize_manifest_authoring(map),
                    "defmodule" => ensure_module_artifact_hash(map),
                    _ => {}
                }
            }
            for entry in map.values_mut() {
                normalize_authoring_hashes(entry);
            }
        }
        _ => {}
    }
}

fn normalize_manifest_authoring(map: &mut serde_json::Map<String, Value>) {
    for key in ["schemas", "modules", "workflows", "effects", "secrets"] {
        if let Some(Value::Array(entries)) = map.get_mut(key) {
            for entry in entries {
                if let Value::Object(obj) = entry {
                    normalize_named_ref_authoring(obj);
                }
            }
        }
    }
}

fn ensure_module_artifact_hash(map: &mut serde_json::Map<String, Value>) {
    let Some(Value::Object(runtime)) = map.get_mut("runtime") else {
        return;
    };
    if runtime.get("kind").and_then(Value::as_str) != Some("wasm") {
        return;
    }
    let Some(Value::Object(artifact)) = runtime.get_mut("artifact") else {
        return;
    };
    if artifact.get("kind").and_then(Value::as_str) == Some("wasm_module") {
        ensure_hash_field(artifact, "hash");
    }
}

fn normalize_named_ref_authoring(map: &mut serde_json::Map<String, Value>) {
    if !matches!(map.get("name"), Some(Value::String(_))) {
        return;
    }
    ensure_hash_field(map, "hash");
}

fn ensure_hash_field(map: &mut serde_json::Map<String, Value>, key: &str) {
    let mut needs_insert = false;
    match map.get_mut(key) {
        Some(Value::String(current)) => {
            let trimmed = current.trim();
            if trimmed.is_empty()
                || trimmed.eq_ignore_ascii_case("sha256")
                || trimmed.eq_ignore_ascii_case("sha256:")
            {
                *current = ZERO_HASH_SENTINEL.to_string();
            }
        }
        Some(value @ Value::Null) => {
            *value = Value::String(ZERO_HASH_SENTINEL.to_string());
        }
        Some(_) => {}
        None => needs_insert = true,
    }

    if needs_insert {
        map.insert(
            key.to_string(),
            Value::String(ZERO_HASH_SENTINEL.to_string()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_all_world_secret_values_reads_dotenv_bindings() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(
            temp.path().join(".env"),
            "OPENAI_API_KEY=openai-test\nANTHROPIC_API_KEY=anthropic-test\n",
        )
        .expect("write .env");

        std::fs::write(
            temp.path().join("aos.world.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "secrets": {
                    "sources": [
                        { "name": "local_env", "kind": "dotenv", "path": ".env" }
                    ],
                    "bindings": [
                        {
                            "binding": "llm/openai_api",
                            "from": { "source": "local_env", "key": "OPENAI_API_KEY" }
                        },
                        {
                            "binding": "llm/anthropic_api",
                            "from": { "source": "local_env", "key": "ANTHROPIC_API_KEY" }
                        }
                    ]
                }
            }))
            .expect("encode world config"),
        )
        .expect("write world config");

        let (_map, _config, values) =
            load_all_world_secret_values(temp.path(), None).expect("load secret values");
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].binding, "llm/openai_api");
        assert_eq!(values[0].plaintext, b"openai-test");
        assert_eq!(values[1].binding, "llm/anthropic_api");
        assert_eq!(values[1].plaintext, b"anthropic-test");
    }

    #[test]
    fn load_world_config_is_optional() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let (path, config) = load_world_config(temp.path(), None).expect("load world config");
        assert!(path.is_none());
        assert_eq!(config.version, 1);
        assert!(config.workspaces.is_empty());
    }

    #[test]
    fn resolve_world_air_sources_uses_config_relative_paths() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let world_root = temp.path().join("world");
        let config_root = world_root.join("config");
        let air_dir = config_root.join("custom-air");
        let module_dir = config_root.join("custom-workflow");
        std::fs::create_dir_all(&air_dir).expect("mkdir air");
        std::fs::create_dir_all(&module_dir).expect("mkdir module dir");
        std::fs::write(
            air_dir.join("manifest.air.json"),
            r#"{"$kind":"manifest","air_version":"2","schemas":[],"modules":[],"workflows":[],"effects":[],"secrets":[]}"#,
        )
        .expect("write manifest");

        let config = WorldConfig {
            version: 1,
            air: Some(WorldAirConfig {
                dir: Some(PathBuf::from("custom-air")),
                mode: None,
            }),
            build: Some(BuildSync {
                module_dir: Some(PathBuf::from("custom-workflow")),
                module: None,
            }),
            secrets: None,
            workspaces: Vec::new(),
        };

        let resolved = resolve_world_air_sources(
            &world_root,
            Some(&config_root.join("aos.world.json")),
            &config,
            &world_root.join("air"),
            &world_root.join("workflow"),
        )
        .expect("resolve");

        assert_eq!(resolved.air_dir, air_dir);
        assert_eq!(resolved.module_dir, module_dir);
        assert_eq!(resolved.import_dirs.len(), 0);
        assert_eq!(resolved.sources.len(), 1);
    }

    #[test]
    fn resolve_world_air_sources_discovers_aos_metadata_dependency() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let world_root = workspace_root.join("worlds/demiurge");
        let workflow_dir = workspace_root.join("worlds/demiurge/workflow");
        let config = WorldConfig::default();

        let resolved = resolve_world_air_sources(
            &world_root,
            None,
            &config,
            &world_root.join("air"),
            &workflow_dir,
        )
        .expect("resolve");

        assert!(
            resolved
                .packages
                .iter()
                .any(|package| package.package_name == "aos-agent")
        );
        let actual = resolved
            .import_dirs
            .iter()
            .find(|path| path.ends_with("crates/aos-agent/air"))
            .and_then(|path| std::fs::canonicalize(path).ok())
            .expect("canonical actual");
        let expected = std::fs::canonicalize(workspace_root.join("crates/aos-agent/air"))
            .expect("canonical expected");
        assert_eq!(actual, expected);
        assert!(
            resolved
                .warnings
                .iter()
                .any(|warning| warning.contains("discovered AIR package aos-agent"))
        );
    }

    #[test]
    fn import_defs_hash_ignores_manifest_nodes() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let import_root = temp.path().join("sdk");
        std::fs::create_dir_all(&import_root).expect("mkdir");
        std::fs::write(
            import_root.join("defs.air.json"),
            r#"[{"$kind":"defschema","name":"demo/S@1","type":{"text":{}}}]"#,
        )
        .expect("write defs");
        std::fs::write(
            import_root.join("manifest.air.json"),
            r#"{"$kind":"manifest","air_version":"2","schemas":[],"modules":[],"workflows":[],"effects":[],"secrets":[]}"#,
        )
        .expect("write manifest");

        let hash = import_defs_hash(&import_root).expect("hash");
        assert!(hash.starts_with("sha256:"));
    }
}
