use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use aos_air_types::AirNode;
use aos_cbor::Hash;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

const ZERO_HASH_SENTINEL: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

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
    #[serde(default)]
    pub lock: Option<AirImportLock>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AirImportLock {
    DefsHash(String),
    Payload(ImportLockPayload),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ImportLockPayload {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub air_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub defs_hash: String,
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

#[derive(Debug, Clone)]
pub struct ResolvedAirImport {
    pub root: PathBuf,
    pub expected_lock: ImportLockPayload,
}

#[derive(Debug, Clone)]
pub struct ResolvedAirSources {
    pub air_dir: PathBuf,
    pub import_dirs: Vec<PathBuf>,
    pub imports: Vec<ResolvedAirImport>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockEnforcementMode {
    Warn,
    Error,
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

pub fn resolve_air_sources(
    world_root: &Path,
    map_root: &Path,
    config: &SyncConfig,
    default_air_dir: &Path,
    default_reducer_dir: &Path,
) -> Result<ResolvedAirSources> {
    resolve_air_sources_with_mode(
        world_root,
        map_root,
        config,
        default_air_dir,
        default_reducer_dir,
        lock_mode_from_env(),
    )
}

fn resolve_air_sources_with_mode(
    world_root: &Path,
    map_root: &Path,
    config: &SyncConfig,
    default_air_dir: &Path,
    default_reducer_dir: &Path,
    lock_mode: LockEnforcementMode,
) -> Result<ResolvedAirSources> {
    let air_dir = config
        .air
        .as_ref()
        .and_then(|air| air.dir.as_ref())
        .map(|dir| resolve_map_path(map_root, dir))
        .unwrap_or_else(|| default_air_dir.to_path_buf());

    let mut metadata_cache: HashMap<PathBuf, CargoMetadata> = HashMap::new();
    let mut import_dirs = Vec::new();
    let mut imports = Vec::new();
    let mut warnings = Vec::new();

    if let Some(air) = &config.air {
        for import in &air.imports {
            let resolved = resolve_air_import(
                world_root,
                map_root,
                default_reducer_dir,
                import,
                &mut metadata_cache,
            )?;
            validate_import_lock(
                &resolved.expected_lock,
                import.lock.as_ref(),
                lock_mode,
                &mut warnings,
            )?;
            import_dirs.push(resolved.root.clone());
            imports.push(resolved);
        }
    }

    Ok(ResolvedAirSources {
        air_dir,
        import_dirs,
        imports,
        warnings,
    })
}

fn resolve_air_import(
    world_root: &Path,
    map_root: &Path,
    default_reducer_dir: &Path,
    import: &AirImport,
    metadata_cache: &mut HashMap<PathBuf, CargoMetadata>,
) -> Result<ResolvedAirImport> {
    match (&import.path, &import.cargo) {
        (Some(path), None) => {
            let root = resolve_map_path(map_root, path);
            let defs_hash = import_defs_hash(&root)?;
            let expected_lock = ImportLockPayload {
                source: "path".into(),
                package: None,
                version: None,
                source_id: None,
                manifest_path: None,
                air_dir: None,
                path: Some(display_path_for_lock(&root, map_root)),
                defs_hash,
            };
            Ok(ResolvedAirImport {
                root,
                expected_lock,
            })
        }
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
) -> Result<ResolvedAirImport> {
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
    let root = package_root.join(&air_dir);
    let defs_hash = import_defs_hash(&root)?;

    let expected_lock = ImportLockPayload {
        source: "cargo".into(),
        package: Some(package.name.clone()),
        version: Some(package.version.clone()),
        source_id: package.source.clone(),
        manifest_path: Some(display_path_for_lock(&manifest_path, map_root)),
        air_dir: Some(air_dir.to_string_lossy().to_string()),
        path: None,
        defs_hash,
    };

    Ok(ResolvedAirImport {
        root,
        expected_lock,
    })
}

fn validate_import_lock(
    expected: &ImportLockPayload,
    actual: Option<&AirImportLock>,
    mode: LockEnforcementMode,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let target = lock_target(expected);
    let expected_json =
        serde_json::to_string(expected).context("serialize expected import lock")?;
    let expected_pretty =
        serde_json::to_string_pretty(expected).context("serialize expected import lock pretty")?;

    let mismatch = match actual {
        None => Some(format!("air.import lock missing for '{target}'")),
        Some(AirImportLock::DefsHash(found)) => {
            if found.trim() == expected.defs_hash {
                None
            } else {
                Some(format!(
                    "air.import lock hash mismatch for '{}': expected '{}', found '{}'",
                    target, expected.defs_hash, found
                ))
            }
        }
        Some(AirImportLock::Payload(found)) => {
            if found == expected {
                None
            } else {
                let found_json =
                    serde_json::to_string(found).context("serialize found import lock")?;
                Some(format!(
                    "air.import lock payload mismatch for '{}': expected {}, found {}",
                    target, expected_json, found_json
                ))
            }
        }
    };

    if let Some(message) = mismatch {
        let help = format!("set lock to:\n{}", expected_pretty);
        match mode {
            LockEnforcementMode::Warn => {
                warnings.push(format!("{message}; {help}"));
                Ok(())
            }
            LockEnforcementMode::Error => {
                anyhow::bail!("{message}; {help}");
            }
        }
    } else {
        Ok(())
    }
}

fn lock_target(expected: &ImportLockPayload) -> String {
    if let Some(path) = &expected.path {
        path.clone()
    } else if let Some(package) = &expected.package {
        package.clone()
    } else {
        "<unknown import>".to_string()
    }
}

fn lock_mode_from_env() -> LockEnforcementMode {
    if env_flag_true("AOS_IMPORT_LOCK_STRICT") || std::env::var_os("CI").is_some() {
        LockEnforcementMode::Error
    } else {
        LockEnforcementMode::Warn
    }
}

fn env_flag_true(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes" || normalized == "on"
        }
        Err(_) => false,
    }
}

fn import_defs_hash(root: &Path) -> Result<String> {
    let mut entries: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut seen: HashMap<(String, String), String> = HashMap::new();

    for path in collect_json_files(root)? {
        for node in parse_air_nodes(&path)
            .with_context(|| format!("parse AIR nodes from {}", path.display()))?
        {
            match node {
                AirNode::Manifest(_) => {
                    anyhow::bail!(
                        "import assets may not define manifest nodes (saw {})",
                        path.display()
                    );
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
                AirNode::Defplan(plan) => {
                    let name = plan.name.clone();
                    let node = AirNode::Defplan(plan);
                    add_def_entry(&mut entries, &mut seen, "defplan", name.as_str(), &node)?;
                }
                AirNode::Defcap(cap) => {
                    let name = cap.name.clone();
                    let node = AirNode::Defcap(cap);
                    add_def_entry(&mut entries, &mut seen, "defcap", name.as_str(), &node)?;
                }
                AirNode::Defpolicy(policy) => {
                    let name = policy.name.clone();
                    let node = AirNode::Defpolicy(policy);
                    add_def_entry(&mut entries, &mut seen, "defpolicy", name.as_str(), &node)?;
                }
                AirNode::Defsecret(secret) => {
                    let name = secret.name.clone();
                    let node = AirNode::Defsecret(secret);
                    add_def_entry(&mut entries, &mut seen, "defsecret", name.as_str(), &node)?;
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

fn display_path_for_lock(path: &Path, map_root: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(map_root) {
        relative.to_string_lossy().to_string()
    } else {
        path.to_string_lossy().to_string()
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

fn parse_air_nodes(path: &Path) -> Result<Vec<AirNode>> {
    let data = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let trimmed = data.trim_start();
    if trimmed.starts_with('[') {
        let mut value: Value = serde_json::from_str(&data).context("parse AIR node array")?;
        normalize_authoring_hashes(&mut value);
        serde_json::from_value(value).context("deserialize AIR node array")
    } else if trimmed.is_empty() {
        Ok(Vec::new())
    } else {
        let mut value: Value = serde_json::from_str(&data).context("parse AIR node")?;
        normalize_authoring_hashes(&mut value);
        let node: AirNode = serde_json::from_value(value).context("deserialize AIR node")?;
        Ok(vec![node])
    }
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
                    "defmodule" => ensure_hash_field(map, "wasm_hash"),
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
    for key in [
        "schemas", "modules", "plans", "caps", "policies", "effects", "secrets",
    ] {
        if let Some(Value::Array(entries)) = map.get_mut(key) {
            for entry in entries {
                if let Value::Object(obj) = entry {
                    normalize_named_ref_authoring(obj);
                }
            }
        }
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
        let temp = tempfile::TempDir::new().expect("tempdir");
        let world_root = temp.path().join("world");
        let map_root = world_root.clone();
        let reducer_dir = world_root.join("reducer");
        let default_air = world_root.join("air-default");
        let import_root = world_root.join("../sdk/air");
        std::fs::create_dir_all(&import_root).expect("mkdir import");
        std::fs::write(
            import_root.join("defs.air.json"),
            r#"[{"$kind":"defschema","name":"demo/S@1","type":{"text":{}}}]"#,
        )
        .expect("write defs");
        let lock_hash = import_defs_hash(&import_root).expect("import hash");

        let mut config = empty_config();
        config.air = Some(AirSync {
            dir: Some(PathBuf::from("air")),
            imports: vec![AirImport {
                path: Some(PathBuf::from("../sdk/air")),
                cargo: None,
                lock: Some(AirImportLock::DefsHash(lock_hash)),
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
        let import_root = workspace_root.join("crates/aos-agent-sdk/air/exports/session-contracts");
        let lock_hash = import_defs_hash(&import_root).expect("import hash");
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
                lock: Some(AirImportLock::DefsHash(lock_hash)),
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

    #[test]
    fn missing_lock_warns_in_warn_mode() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let import_root = temp.path().join("sdk");
        std::fs::create_dir_all(&import_root).expect("mkdir");
        std::fs::write(
            import_root.join("defs.air.json"),
            r#"[{"$kind":"defschema","name":"demo/S@1","type":{"text":{}}}]"#,
        )
        .expect("write defs");

        let mut config = empty_config();
        config.air = Some(AirSync {
            dir: None,
            imports: vec![AirImport {
                path: Some(PathBuf::from("sdk")),
                cargo: None,
                lock: None,
            }],
        });

        let resolved = resolve_air_sources_with_mode(
            temp.path(),
            temp.path(),
            &config,
            &temp.path().join("air"),
            &temp.path().join("reducer"),
            LockEnforcementMode::Warn,
        )
        .expect("resolve");

        assert_eq!(resolved.import_dirs.len(), 1);
        assert_eq!(resolved.warnings.len(), 1);
        assert!(resolved.warnings[0].contains("lock missing"));
    }

    #[test]
    fn missing_lock_fails_in_error_mode() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let import_root = temp.path().join("sdk");
        std::fs::create_dir_all(&import_root).expect("mkdir");
        std::fs::write(
            import_root.join("defs.air.json"),
            r#"[{"$kind":"defschema","name":"demo/S@1","type":{"text":{}}}]"#,
        )
        .expect("write defs");

        let mut config = empty_config();
        config.air = Some(AirSync {
            dir: None,
            imports: vec![AirImport {
                path: Some(PathBuf::from("sdk")),
                cargo: None,
                lock: None,
            }],
        });

        let err = resolve_air_sources_with_mode(
            temp.path(),
            temp.path(),
            &config,
            &temp.path().join("air"),
            &temp.path().join("reducer"),
            LockEnforcementMode::Error,
        )
        .expect_err("should fail");

        assert!(err.to_string().contains("lock missing"));
    }
}
