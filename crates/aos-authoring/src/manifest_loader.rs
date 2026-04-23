use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use aos_air_types::{
    AirNode, DefEffect, DefModule, DefSchema, DefSecret, DefWorkflow, HashRef, Manifest, Name,
    NamedRef, builtins,
};
use aos_cbor::Hash;
use aos_kernel::{LoadedManifest, ManifestLoader, Store};
use serde_json::Value;
use walkdir::WalkDir;

use crate::generated::{GENERATED_AIR_DIR, write_generated_air_from_cargo_export};

pub const ZERO_HASH_SENTINEL: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone)]
pub enum AirSource {
    /// AIR JSON already present on disk.
    Directory {
        path: PathBuf,
        allow_manifest: bool,
        include_root: bool,
    },
    /// A Rust-authored package that must run its export binary before loading AIR JSON.
    GeneratedRustPackage {
        package_root: PathBuf,
        manifest_path: PathBuf,
        package_name: Option<String>,
        bin_name: Option<String>,
        allow_manifest: bool,
    },
}

impl AirSource {
    pub fn local_directory(path: impl Into<PathBuf>) -> Self {
        Self::Directory {
            path: path.into(),
            allow_manifest: true,
            include_root: false,
        }
    }

    pub fn imported_directory(path: impl Into<PathBuf>) -> Self {
        Self::Directory {
            path: path.into(),
            allow_manifest: false,
            include_root: true,
        }
    }

    pub fn generated_rust_package(
        package_root: impl Into<PathBuf>,
        manifest_path: impl Into<PathBuf>,
        package_name: Option<String>,
        bin_name: Option<String>,
        allow_manifest: bool,
    ) -> Self {
        Self::GeneratedRustPackage {
            package_root: package_root.into(),
            manifest_path: manifest_path.into(),
            package_name,
            bin_name,
            allow_manifest,
        }
    }
}

pub struct LoadedAssets {
    pub loaded: LoadedManifest,
    pub secrets: Vec<DefSecret>,
}

pub fn load_from_assets<S: Store + 'static>(
    store: Arc<S>,
    asset_root: &Path,
) -> Result<Option<LoadedManifest>> {
    Ok(load_from_assets_with_defs(store, asset_root)?.map(|assets| assets.loaded))
}

pub fn load_from_assets_with_defs<S: Store + 'static>(
    store: Arc<S>,
    asset_root: &Path,
) -> Result<Option<LoadedAssets>> {
    load_from_assets_with_imports_and_defs(store, asset_root, &[])
}

pub fn load_from_assets_with_imports<S: Store + 'static>(
    store: Arc<S>,
    asset_root: &Path,
    import_roots: &[PathBuf],
) -> Result<Option<LoadedManifest>> {
    Ok(
        load_from_assets_with_imports_and_defs(store, asset_root, import_roots)?
            .map(|assets| assets.loaded),
    )
}

pub fn load_from_assets_with_imports_and_defs<S: Store + 'static>(
    store: Arc<S>,
    asset_root: &Path,
    import_roots: &[PathBuf],
) -> Result<Option<LoadedAssets>> {
    let mut sources = Vec::with_capacity(import_roots.len() + 1);
    sources.push(AirSource::local_directory(asset_root));
    sources.extend(
        import_roots
            .iter()
            .cloned()
            .map(AirSource::imported_directory),
    );
    load_from_air_sources_with_defs(store, &sources)
}

pub fn load_from_air_sources<S: Store + 'static>(
    store: Arc<S>,
    sources: &[AirSource],
) -> Result<Option<LoadedManifest>> {
    Ok(load_from_air_sources_with_defs(store, sources)?.map(|assets| assets.loaded))
}

pub fn load_from_air_sources_with_defs<S: Store + 'static>(
    store: Arc<S>,
    sources: &[AirSource],
) -> Result<Option<LoadedAssets>> {
    let mut manifest: Option<Manifest> = None;
    let mut schemas: Vec<DefSchema> = Vec::new();
    let mut modules: Vec<DefModule> = Vec::new();
    let mut secrets: Vec<DefSecret> = Vec::new();
    let mut workflows: Vec<DefWorkflow> = Vec::new();
    let mut effects: Vec<DefEffect> = Vec::new();

    for root in prepare_air_sources(sources)? {
        for dir_path in asset_search_dirs(&root.path, root.include_root)? {
            for path in collect_json_files(&dir_path)? {
                let nodes = parse_air_nodes(&path)
                    .with_context(|| format!("parse AIR nodes from {}", path.display()))?;
                for node in nodes {
                    match node {
                        AirNode::Manifest(found) => {
                            if !root.allow_manifest {
                                continue;
                            }
                            if manifest.is_some() {
                                bail!(
                                    "multiple manifest nodes found (latest at {})",
                                    path.display()
                                );
                            }
                            manifest = Some(found);
                        }
                        AirNode::Defschema(schema) => schemas.push(schema),
                        AirNode::Defmodule(module) => modules.push(module),
                        AirNode::Defsecret(secret) => secrets.push(secret),
                        AirNode::Defworkflow(workflow) => workflows.push(workflow),
                        AirNode::Defeffect(effect) => effects.push(effect),
                    }
                }
            }
        }
    }

    let mut manifest = match manifest {
        Some(manifest) => manifest,
        None => return Ok(None),
    };

    secrets.sort_by(|a, b| a.name.cmp(&b.name));
    let hashes = write_nodes(
        store.as_ref(),
        false,
        schemas,
        modules,
        secrets.clone(),
        workflows,
        effects,
    )?;
    patch_manifest_refs(&mut manifest, &hashes)?;
    let loaded = ManifestLoader::load_from_manifest(store.as_ref(), &manifest)
        .context("load manifest after authoring asset patching")?;
    Ok(Some(LoadedAssets { loaded, secrets }))
}

fn prepare_air_sources(sources: &[AirSource]) -> Result<Vec<AssetRoot>> {
    sources
        .iter()
        .map(|source| match source {
            AirSource::Directory {
                path,
                allow_manifest,
                include_root,
            } => Ok(AssetRoot {
                path: path.clone(),
                allow_manifest: *allow_manifest,
                include_root: *include_root,
            }),
            AirSource::GeneratedRustPackage {
                package_root,
                manifest_path,
                package_name,
                bin_name,
                allow_manifest,
            } => {
                write_generated_air_from_cargo_export(
                    package_root,
                    manifest_path,
                    package_name.as_deref(),
                    bin_name.as_deref(),
                )
                .with_context(|| {
                    format!(
                        "materialize generated AIR for package source {}",
                        package_root.display()
                    )
                })?;
                Ok(AssetRoot {
                    path: package_root.join(GENERATED_AIR_DIR),
                    allow_manifest: *allow_manifest,
                    include_root: true,
                })
            }
        })
        .collect()
}

fn write_nodes<S: Store + ?Sized>(
    store: &S,
    allow_reserved_sys: bool,
    schemas: Vec<DefSchema>,
    modules: Vec<DefModule>,
    secrets: Vec<DefSecret>,
    workflows: Vec<DefWorkflow>,
    effects: Vec<DefEffect>,
) -> Result<StoredHashes> {
    let mut hashes = StoredHashes::default();
    for schema in schemas {
        let name = schema.name.clone();
        if !allow_reserved_sys {
            reject_sys_name("defschema", name.as_str())?;
        }
        let hash = store
            .put_node(&AirNode::Defschema(schema))
            .context("store defschema node")?;
        insert_or_verify_hash("defschema", &mut hashes.schemas, name, hash)?;
    }
    for module in modules {
        let name = module.name.clone();
        if !allow_reserved_sys {
            reject_sys_name("defmodule", name.as_str())?;
        }
        let hash = store
            .put_node(&AirNode::Defmodule(module))
            .context("store defmodule node")?;
        insert_or_verify_hash("defmodule", &mut hashes.modules, name, hash)?;
    }
    for secret in secrets {
        let name = secret.name.clone();
        if !allow_reserved_sys {
            reject_sys_name("defsecret", name.as_str())?;
        }
        let hash = store
            .put_node(&AirNode::Defsecret(secret))
            .context("store defsecret node")?;
        insert_or_verify_hash("defsecret", &mut hashes.secrets, name, hash)?;
    }
    for workflow in workflows {
        let name = workflow.name.clone();
        if !allow_reserved_sys {
            reject_sys_name("defworkflow", name.as_str())?;
        }
        let hash = store
            .put_node(&AirNode::Defworkflow(workflow))
            .context("store defworkflow node")?;
        insert_or_verify_hash("defworkflow", &mut hashes.workflows, name, hash)?;
    }
    for effect in effects {
        let name = effect.name.clone();
        if !allow_reserved_sys {
            reject_sys_name("defeffect", name.as_str())?;
        }
        let hash = store
            .put_node(&AirNode::Defeffect(effect))
            .context("store defeffect node")?;
        insert_or_verify_hash("defeffect", &mut hashes.effects, name, hash)?;
    }
    Ok(hashes)
}

fn insert_or_verify_hash(
    kind: &str,
    map: &mut HashMap<Name, HashRef>,
    name: Name,
    hash: Hash,
) -> Result<()> {
    let hash_ref = HashRef::new(hash.to_hex())?;
    if let Some(existing) = map.get(name.as_str()) {
        if existing != &hash_ref {
            bail!(
                "duplicate {kind} '{}' has conflicting definitions ({}, {})",
                name,
                existing.as_str(),
                hash_ref.as_str()
            );
        }
        return Ok(());
    }
    map.insert(name, hash_ref);
    Ok(())
}

fn reject_sys_name(kind: &str, name: &str) -> Result<()> {
    if name.starts_with("sys/") {
        bail!("{kind} '{name}' is reserved; sys/* definitions must come from built-ins");
    }
    Ok(())
}

#[derive(Default)]
struct StoredHashes {
    schemas: HashMap<Name, HashRef>,
    modules: HashMap<Name, HashRef>,
    workflows: HashMap<Name, HashRef>,
    effects: HashMap<Name, HashRef>,
    secrets: HashMap<Name, HashRef>,
}

#[derive(Debug, Clone)]
struct AssetRoot {
    path: PathBuf,
    allow_manifest: bool,
    include_root: bool,
}

fn patch_manifest_refs(manifest: &mut Manifest, hashes: &StoredHashes) -> Result<()> {
    patch_named_refs("schema", &mut manifest.schemas, &hashes.schemas)?;
    patch_named_refs("module", &mut manifest.modules, &hashes.modules)?;
    patch_named_refs("workflow", &mut manifest.workflows, &hashes.workflows)?;
    patch_named_refs("effect", &mut manifest.effects, &hashes.effects)?;
    patch_named_refs("secret", &mut manifest.secrets, &hashes.secrets)?;
    Ok(())
}

fn patch_named_refs(
    kind: &str,
    refs: &mut [NamedRef],
    hashes: &HashMap<Name, HashRef>,
) -> Result<()> {
    for reference in refs {
        let actual = if let Some(found) = hashes.get(reference.name.as_str()) {
            found.clone()
        } else if let Some(builtin) = builtins::find_builtin_schema(reference.name.as_str()) {
            builtin.hash_ref.clone()
        } else if kind == "workflow" {
            if let Some(builtin) = builtins::find_builtin_workflow(reference.name.as_str()) {
                builtin.hash_ref.clone()
            } else {
                bail!("manifest references unknown {kind} '{}'", reference.name);
            }
        } else if kind == "effect" {
            if let Some(builtin) = builtins::find_builtin_effect(reference.name.as_str()) {
                builtin.hash_ref.clone()
            } else {
                bail!("manifest references unknown {kind} '{}'", reference.name);
            }
        } else if kind == "module" {
            if let Some(builtin) = builtins::find_builtin_module(reference.name.as_str()) {
                builtin.hash_ref.clone()
            } else {
                bail!("manifest references unknown {kind} '{}'", reference.name);
            }
        } else {
            bail!("manifest references unknown {kind} '{}'", reference.name);
        };
        reference.hash = actual;
    }
    Ok(())
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

fn asset_search_dirs(asset_root: &Path, include_root: bool) -> Result<Vec<PathBuf>> {
    if include_root {
        return Ok(vec![asset_root.to_path_buf()]);
    }

    let mut dirs: Vec<PathBuf> = Vec::new();

    if asset_root
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with("air") || n == "defs")
        .unwrap_or(false)
    {
        dirs.push(asset_root.to_path_buf());
    }

    if asset_root.is_dir() {
        for entry in fs::read_dir(asset_root).context("read asset root")? {
            let entry = entry.context("read asset dir entry")?;
            if !entry.file_type().context("stat asset dir entry")?.is_dir() {
                continue;
            }
            let name_os = entry.file_name();
            let name = match name_os.to_str() {
                Some(s) => s.to_owned(),
                None => continue,
            };
            if name == "defs" || name.starts_with("air") {
                dirs.push(entry.path());
            }
        }
    }

    dirs.sort();
    Ok(dirs)
}

fn collect_json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(dir) {
        let entry = entry.context("walk assets directory")?;
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
    let data = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    parse_air_nodes_from_str(&data)
        .with_context(|| format!("parse AIR nodes from {}", path.display()))
}

/// Parse generated or authored AIR JSON into AIR nodes.
///
/// This is the shared entry point for host-side Rust AIR generation. Generated files under
/// `air/generated/` should use the same authoring normalization path as hand-authored AIR.
pub fn parse_air_nodes_from_str(data: &str) -> Result<Vec<AirNode>> {
    let trimmed = data.trim_start();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let mut value: Value = if trimmed.starts_with('[') {
        serde_json::from_str(&data).context("parse AIR node array")?
    } else {
        serde_json::from_str(&data).context("parse AIR node")?
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{TypeExpr, TypePrimitive};

    #[test]
    fn parse_air_nodes_from_str_accepts_generated_defschema_json() {
        let nodes = parse_air_nodes_from_str(
            r#"{"$kind":"defschema","name":"demo/Generated@1","type":{"record":{"task":{"text":{}}}}}"#,
        )
        .expect("parse generated AIR");

        let [AirNode::Defschema(schema)] = nodes.as_slice() else {
            panic!("expected one defschema node");
        };
        assert_eq!(schema.name, "demo/Generated@1");
        let TypeExpr::Record(record) = &schema.ty else {
            panic!("expected record schema");
        };
        assert!(matches!(
            record.record.get("task"),
            Some(TypeExpr::Primitive(TypePrimitive::Text(_)))
        ));
    }
}
