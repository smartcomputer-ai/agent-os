//! Load AIR manifests from JSON asset directories.
//!
//! This module provides utilities to load AIR JSON files from directories and
//! produce a `LoadedManifest` suitable for kernel initialization.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use aos_air_types::{
    self as air_types, AirNode, DefCap, DefEffect, DefModule, DefPolicy, DefSchema,
    DefSecret, HashRef, Manifest, Name, NamedRef, SecretEntry, catalog::EffectCatalog,
    validate_manifest,
};
use aos_cbor::Hash;
use aos_kernel::{LoadedManifest, governance::ManifestPatch};
use aos_store::{Catalog, FsStore, Store, load_manifest_from_bytes};
use log::warn;
use serde_json::Value;
use walkdir::WalkDir;

/// Placeholder hash used for authoring AIR JSON manifests. When a module's
/// `wasm_hash` equals this value, it indicates the hash should be patched
/// at load time with the actual compiled WASM hash.
pub const ZERO_HASH_SENTINEL: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

pub struct LoadedAssets {
    pub loaded: LoadedManifest,
    pub secrets: Vec<DefSecret>,
}

/// Attempts to load a manifest for the provided example directory by reading AIR JSON assets
/// under `air/`, versioned `air.*` bundles, and `defs/`. The `asset_root`
/// can itself be an AIR bundle (e.g., `examples/06-safe-upgrade/air.v2`). Returns `Ok(None)`
/// if no manifest is found so callers can fall back to the legacy Rust-built manifests.
pub fn load_from_assets(store: Arc<FsStore>, asset_root: &Path) -> Result<Option<LoadedManifest>> {
    Ok(load_from_assets_with_defs(store, asset_root)?.map(|assets| assets.loaded))
}

pub fn load_from_assets_with_defs(
    store: Arc<FsStore>,
    asset_root: &Path,
) -> Result<Option<LoadedAssets>> {
    load_from_assets_with_imports_and_defs(store, asset_root, &[])
}

pub fn load_from_assets_with_imports(
    store: Arc<FsStore>,
    asset_root: &Path,
    import_roots: &[PathBuf],
) -> Result<Option<LoadedManifest>> {
    Ok(
        load_from_assets_with_imports_and_defs(store, asset_root, import_roots)?
            .map(|assets| assets.loaded),
    )
}

pub fn load_from_assets_with_imports_and_defs(
    store: Arc<FsStore>,
    asset_root: &Path,
    import_roots: &[PathBuf],
) -> Result<Option<LoadedAssets>> {
    let mut manifest: Option<Manifest> = None;
    let mut schemas: Vec<DefSchema> = Vec::new();
    let mut modules: Vec<DefModule> = Vec::new();
    let mut caps: Vec<DefCap> = Vec::new();
    let mut policies: Vec<DefPolicy> = Vec::new();
    let mut secrets: Vec<DefSecret> = Vec::new();
    let mut effects: Vec<DefEffect> = Vec::new();

    let mut roots = Vec::with_capacity(import_roots.len() + 1);
    roots.push(AssetRoot {
        path: asset_root.to_path_buf(),
        allow_manifest: true,
        include_root: false,
    });
    roots.extend(import_roots.iter().cloned().map(|path| AssetRoot {
        path,
        allow_manifest: false,
        include_root: true,
    }));

    for root in roots {
        for dir_path in asset_search_dirs(&root.path, root.include_root)? {
            for path in collect_json_files(&dir_path)? {
                let nodes = parse_air_nodes(&path)
                    .with_context(|| format!("parse AIR nodes from {}", path.display()))?;
                for node in nodes {
                    match node {
                        AirNode::Manifest(found) => {
                            if !root.allow_manifest {
                                // Import roots may contain authoring manifests; only the primary
                                // world asset root contributes the manifest for load.
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
                        AirNode::Defcap(cap) => caps.push(cap),
                        AirNode::Defpolicy(policy) => policies.push(policy),
                        AirNode::Defsecret(secret) => secrets.push(secret),
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
        &store,
        schemas,
        modules,
        caps,
        policies,
        secrets.clone(),
        effects,
    )?;
    patch_manifest_refs(&mut manifest, &hashes)?;
    let catalog = manifest_catalog(&store, manifest)?;
    let loaded = catalog_to_loaded(catalog);
    if let Err(err) = validate_manifest(
        &loaded.manifest,
        &loaded.modules,
        &loaded.schemas,
        &loaded.effects,
        &loaded.caps,
        &loaded.policies,
    ) {
        bail!("manifest validation failed: {err}");
    }
    Ok(Some(LoadedAssets { loaded, secrets }))
}

pub fn manifest_patch_from_loaded(loaded: &LoadedManifest) -> ManifestPatch {
    let mut nodes: Vec<AirNode> = loaded
        .modules
        .values()
        .cloned()
        .map(AirNode::Defmodule)
        .collect();
    nodes.extend(loaded.schemas.values().cloned().map(AirNode::Defschema));
    nodes.extend(loaded.caps.values().cloned().map(AirNode::Defcap));
    nodes.extend(loaded.policies.values().cloned().map(AirNode::Defpolicy));
    nodes.extend(loaded.effects.values().cloned().map(AirNode::Defeffect));

    ManifestPatch {
        manifest: loaded.manifest.clone(),
        nodes,
    }
}

fn write_nodes(
    store: &FsStore,
    schemas: Vec<DefSchema>,
    modules: Vec<DefModule>,
    caps: Vec<DefCap>,
    policies: Vec<DefPolicy>,
    secrets: Vec<DefSecret>,
    effects: Vec<DefEffect>,
) -> Result<StoredHashes> {
    let mut hashes = StoredHashes::default();
    for schema in schemas {
        let name = schema.name.clone();
        reject_sys_name("defschema", name.as_str())?;
        let hash = store
            .put_node(&AirNode::Defschema(schema))
            .context("store defschema node")?;
        insert_or_verify_hash("defschema", &mut hashes.schemas, name, hash)?;
    }
    for module in modules {
        let name = module.name.clone();
        reject_sys_name("defmodule", name.as_str())?;
        let hash = store
            .put_node(&AirNode::Defmodule(module))
            .context("store defmodule node")?;
        insert_or_verify_hash("defmodule", &mut hashes.modules, name, hash)?;
    }
    for cap in caps {
        let name = cap.name.clone();
        reject_sys_name("defcap", name.as_str())?;
        let hash = store
            .put_node(&AirNode::Defcap(cap))
            .context("store defcap node")?;
        insert_or_verify_hash("defcap", &mut hashes.caps, name, hash)?;
    }
    for policy in policies {
        let name = policy.name.clone();
        reject_sys_name("defpolicy", name.as_str())?;
        let hash = store
            .put_node(&AirNode::Defpolicy(policy))
            .context("store defpolicy node")?;
        insert_or_verify_hash("defpolicy", &mut hashes.policies, name, hash)?;
    }
    for secret in secrets {
        let name = secret.name.clone();
        reject_sys_name("defsecret", name.as_str())?;
        let hash = store
            .put_node(&AirNode::Defsecret(secret))
            .context("store defsecret node")?;
        insert_or_verify_hash("defsecret", &mut hashes.secrets, name, hash)?;
    }
    for effect in effects {
        let name = effect.name.clone();
        reject_sys_name("defeffect", name.as_str())?;
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
    effects: HashMap<Name, HashRef>,
    caps: HashMap<Name, HashRef>,
    policies: HashMap<Name, HashRef>,
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
    patch_named_refs("effect", &mut manifest.effects, &hashes.effects)?;
    patch_named_refs("cap", &mut manifest.caps, &hashes.caps)?;
    patch_named_refs("policy", &mut manifest.policies, &hashes.policies)?;
    let mut secret_refs = secrets_as_named_refs(&manifest.secrets)?;
    patch_named_refs("secret", &mut secret_refs, &hashes.secrets)?;
    manifest.secrets = secret_refs.into_iter().map(SecretEntry::Ref).collect();
    Ok(())
}

fn secrets_as_named_refs(entries: &[SecretEntry]) -> Result<Vec<NamedRef>> {
    let mut refs = Vec::new();
    for entry in entries {
        match entry {
            SecretEntry::Ref(r) => refs.push(r.clone()),
            SecretEntry::Decl(_) => {
                bail!("inline secret declarations are unsupported; provide defsecret nodes instead")
            }
        }
    }
    Ok(refs)
}

fn patch_named_refs(
    kind: &str,
    refs: &mut [aos_air_types::NamedRef],
    hashes: &HashMap<Name, HashRef>,
) -> Result<()> {
    for reference in refs {
        let actual = if let Some(found) = hashes.get(reference.name.as_str()) {
            found.clone()
        } else if let Some(builtin) =
            air_types::builtins::find_builtin_schema(reference.name.as_str())
        {
            builtin.hash_ref.clone()
        } else if kind == "effect" {
            if let Some(builtin) = air_types::builtins::find_builtin_effect(reference.name.as_str())
            {
                builtin.hash_ref.clone()
            } else {
                bail!("manifest references unknown {kind} '{}'", reference.name);
            }
        } else if kind == "module" {
            if let Some(builtin) = air_types::builtins::find_builtin_module(reference.name.as_str())
            {
                builtin.hash_ref.clone()
            } else {
                bail!("manifest references unknown {kind} '{}'", reference.name);
            }
        } else if kind == "cap" {
            if let Some(builtin) = air_types::builtins::find_builtin_cap(reference.name.as_str()) {
                builtin.hash_ref.clone()
            } else {
                bail!("manifest references unknown {kind} '{}'", reference.name);
            }
        } else {
            bail!("manifest references unknown {kind} '{}'", reference.name);
        };
        if reference.hash != actual {
            if !is_zero_hash(&reference.hash) {
                warn!(
                    "manifest hash for {kind} '{}' is stale (saw {}, using {})",
                    reference.name, reference.hash, actual
                );
            }
            reference.hash = actual;
        }
    }
    Ok(())
}

fn is_zero_hash(value: &HashRef) -> bool {
    value.as_str() == ZERO_HASH_SENTINEL
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
        "schemas", "modules", "caps", "policies", "effects", "secrets",
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

fn manifest_catalog(store: &FsStore, manifest: Manifest) -> Result<Catalog> {
    let bytes = serde_cbor::to_vec(&manifest).context("serialize manifest to CBOR")?;
    load_manifest_from_bytes(store, &bytes).context("load manifest catalog")
}

fn catalog_to_loaded(catalog: Catalog) -> LoadedManifest {
    let Catalog {
        manifest,
        nodes,
        resolved_secrets,
    } = catalog;
    let mut modules = HashMap::new();
    let mut effects = HashMap::new();
    let mut caps = HashMap::new();
    let mut policies = HashMap::new();
    let mut schemas = HashMap::new();

    for (_name, entry) in nodes {
        match entry.node {
            AirNode::Defmodule(module) => {
                modules.insert(module.name.clone(), module);
            }
            AirNode::Defcap(cap) => {
                caps.insert(cap.name.clone(), cap);
            }
            AirNode::Defpolicy(policy) => {
                policies.insert(policy.name.clone(), policy);
            }
            AirNode::Defeffect(effect) => {
                effects.insert(effect.name.clone(), effect);
            }
            AirNode::Defschema(schema) => {
                schemas.insert(schema.name.clone(), schema);
            }
            AirNode::Defsecret(_) => {}
            AirNode::Manifest(_) => {}
        }
    }

    let effect_catalog = EffectCatalog::from_defs(effects.values().cloned());

    LoadedManifest {
        manifest,
        secrets: resolved_secrets,
        modules,
        effects,
        caps,
        policies,
        schemas,
        effect_catalog,
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

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{
        HashRef, ModuleAbi, ModuleKind, ReducerAbi, SchemaRef, SecretEntry, TypeExpr,
        TypePrimitive, TypePrimitiveNat, TypeRef, TypeVariant,
    };
    use aos_cbor::Hash;
    use indexmap::IndexMap;
    use tempfile::TempDir;

    #[test]
    fn manifest_named_refs_allow_authoring_placeholders() {
        let json = r#"{
            "$kind": "manifest",
            "air_version": "1",
            "schemas": [
                { "name": "demo/State@1", "hash": "" },
                { "name": "demo/Event@1" },
                { "name": "demo/Zero@1", "hash": "sha256" }
            ],
            "secrets": [
                { "name": "llm/api@1" }
            ],
            "modules": [],
            "plans": [],
            "caps": [],
            "policies": [],
            "module_bindings": {},
            "triggers": []
        }"#;

        let mut value: Value = serde_json::from_str(json).expect("json");
        normalize_authoring_hashes(&mut value);
        let node: AirNode = serde_json::from_value(value).expect("deserialize manifest");
        let manifest = match node {
            AirNode::Manifest(manifest) => manifest,
            _ => panic!("expected manifest"),
        };

        for reference in manifest.schemas {
            assert_eq!(reference.hash.as_str(), ZERO_HASH_SENTINEL);
        }
        for reference in manifest.secrets {
            match reference {
                SecretEntry::Ref(named) => {
                    assert_eq!(named.hash.as_str(), ZERO_HASH_SENTINEL);
                }
                SecretEntry::Decl(_) => panic!("expected manifest secret ref"),
            }
        }
    }

    #[test]
    fn defmodule_allows_missing_wasm_hash() {
        let json = r#"{
            "$kind": "defmodule",
            "name": "demo/Reducer@1",
            "module_kind": "reducer",
            "abi": {
                "reducer": {
                    "state": "demo/State@1",
                    "event": "demo/Event@1"
                }
            }
        }"#;

        let mut value: Value = serde_json::from_str(json).expect("json");
        normalize_authoring_hashes(&mut value);
        let node: AirNode = serde_json::from_value(value).expect("deserialize module");
        let module = match node {
            AirNode::Defmodule(module) => module,
            _ => panic!("expected defmodule"),
        };

        assert_eq!(module.wasm_hash.as_str(), ZERO_HASH_SENTINEL);
    }

    #[test]
    fn loads_manifest_from_json_assets() {
        let tmp = TempDir::new().expect("tmp");
        let example_root = tmp.path();
        let air_dir = example_root.join("air");
        fs::create_dir_all(&air_dir).expect("mkdir air");

        let state_schema = DefSchema {
            name: "demo/State@1".into(),
            ty: nat_type(),
        };
        let event_payload_schema = DefSchema {
            name: "demo/EventPayload@1".into(),
            ty: nat_type(),
        };
        let event_schema = DefSchema {
            name: "demo/Event@1".into(),
            ty: TypeExpr::Variant(TypeVariant {
                variant: IndexMap::from([(
                    "Payload".to_string(),
                    TypeExpr::Ref(TypeRef {
                        reference: SchemaRef::new(&event_payload_schema.name).unwrap(),
                    }),
                )]),
            }),
        };

        let module = DefModule {
            name: "demo/Reducer@1".into(),
            module_kind: ModuleKind::Workflow,
            wasm_hash: HashRef::new(Hash::of_bytes(b"wasm").to_hex()).unwrap(),
            key_schema: None,
            abi: ModuleAbi {
                reducer: Some(ReducerAbi {
                    state: SchemaRef::new(&state_schema.name).unwrap(),
                    event: SchemaRef::new(&event_schema.name).unwrap(),
                    context: Some(SchemaRef::new("sys/ReducerContext@1").unwrap()),
                    annotations: None,
                    effects_emitted: Vec::new(),
                    cap_slots: IndexMap::new(),
                }),
                pure: None,
            },
        };

        let state_node = AirNode::Defschema(state_schema.clone());
        let event_payload_node = AirNode::Defschema(event_payload_schema.clone());
        let event_node = AirNode::Defschema(event_schema.clone());
        let module_node = AirNode::Defmodule(module.clone());
        write_node(
            &air_dir.join("schemas.air.json"),
            &[
                state_node.clone(),
                event_payload_node.clone(),
                event_node.clone(),
            ],
        );
        write_node(&air_dir.join("module.air.json"), &[module_node.clone()]);

        let mut manifest = Manifest {
            air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
            schemas: vec![
                named_ref_from_node(&state_node),
                named_ref_from_node(&event_payload_node),
                named_ref_from_node(&event_node),
            ],
            modules: vec![named_ref_from_node(&module_node)],
            effects: aos_air_types::builtins::builtin_effects()
                .iter()
                .map(|e| NamedRef {
                    name: e.effect.name.clone(),
                    hash: e.hash_ref.clone(),
                })
                .collect(),
            caps: Vec::new(),
            policies: Vec::new(),
            secrets: Vec::new(),
            defaults: None,
            module_bindings: IndexMap::new(),
            routing: None,
        };
        manifest
            .schemas
            .extend(
                aos_air_types::builtins::builtin_schemas()
                    .iter()
                    .map(|s| NamedRef {
                        name: s.schema.name.clone(),
                        hash: s.hash_ref.clone(),
                    }),
            );
        write_node(
            &air_dir.join("manifest.air.json"),
            &[AirNode::Manifest(manifest)],
        );

        let store = Arc::new(FsStore::open(example_root).expect("store"));
        let loaded = load_from_assets(store, example_root).expect("load");

        let loaded = loaded.expect("manifest present");
        assert!(loaded.modules.contains_key("demo/Reducer@1"));
        assert!(loaded.schemas.contains_key("demo/State@1"));
        assert!(loaded.schemas.contains_key("demo/EventPayload@1"));
        assert!(loaded.schemas.contains_key("demo/Event@1"));
    }

    #[test]
    fn returns_none_without_manifest() {
        let tmp = TempDir::new().expect("tmp");
        let store = Arc::new(FsStore::open(tmp.path()).expect("store"));
        let result = load_from_assets(store, tmp.path()).expect("ok");
        assert!(result.is_none());
    }

    #[test]
    fn loads_manifest_with_imported_defs() {
        let tmp = TempDir::new().expect("tmp");
        let world_air = tmp.path().join("air");
        let import_air = tmp.path().join("sdk-exports");
        fs::create_dir_all(&world_air).expect("mkdir world air");
        fs::create_dir_all(&import_air).expect("mkdir import air");

        let shared_schema = DefSchema {
            name: "aos.agent/SessionId@1".into(),
            ty: TypeExpr::Primitive(TypePrimitive::Nat(TypePrimitiveNat {
                nat: Default::default(),
            })),
        };
        let node = AirNode::Defschema(shared_schema.clone());
        write_node(&import_air.join("defs.air.json"), &[node.clone()]);

        // Include an identical local def to exercise dedupe-by-hash merge semantics.
        write_node(&world_air.join("schemas.air.json"), &[node]);

        let manifest = Manifest {
            air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
            schemas: vec![aos_air_types::NamedRef {
                name: shared_schema.name.clone(),
                hash: aos_air_types::HashRef::new(ZERO_HASH_SENTINEL).expect("zero hash"),
            }],
            modules: Vec::new(),
            effects: Vec::new(),
            caps: Vec::new(),
            policies: Vec::new(),
            secrets: Vec::new(),
            defaults: None,
            module_bindings: IndexMap::new(),
            routing: None,
        };
        write_node(
            &world_air.join("manifest.air.json"),
            &[AirNode::Manifest(manifest)],
        );

        let store = Arc::new(FsStore::open(tmp.path()).expect("store"));
        let loaded =
            load_from_assets_with_imports(store, &world_air, &[import_air]).expect("load assets");
        let loaded = loaded.expect("manifest present");
        assert!(loaded.schemas.contains_key("aos.agent/SessionId@1"));
    }

    #[test]
    fn rejects_conflicting_import_defs() {
        let tmp = TempDir::new().expect("tmp");
        let world_air = tmp.path().join("air");
        let import_air = tmp.path().join("sdk-exports");
        fs::create_dir_all(&world_air).expect("mkdir world air");
        fs::create_dir_all(&import_air).expect("mkdir import air");

        let schema_one = DefSchema {
            name: "aos.agent/SessionId@1".into(),
            ty: nat_type(),
        };
        let schema_two = DefSchema {
            name: "aos.agent/SessionId@1".into(),
            ty: TypeExpr::Primitive(TypePrimitive::Text(aos_air_types::TypePrimitiveText {
                text: Default::default(),
            })),
        };
        write_node(
            &world_air.join("schemas.air.json"),
            &[AirNode::Defschema(schema_one.clone())],
        );
        write_node(
            &import_air.join("defs.air.json"),
            &[AirNode::Defschema(schema_two.clone())],
        );

        let manifest = Manifest {
            air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
            schemas: vec![aos_air_types::NamedRef {
                name: schema_one.name.clone(),
                hash: aos_air_types::HashRef::new(ZERO_HASH_SENTINEL).expect("zero hash"),
            }],
            modules: Vec::new(),
            effects: Vec::new(),
            caps: Vec::new(),
            policies: Vec::new(),
            secrets: Vec::new(),
            defaults: None,
            module_bindings: IndexMap::new(),
            routing: None,
        };
        write_node(
            &world_air.join("manifest.air.json"),
            &[AirNode::Manifest(manifest)],
        );

        let store = Arc::new(FsStore::open(tmp.path()).expect("store"));
        let err = match load_from_assets_with_imports(store, &world_air, &[import_air]) {
            Ok(_) => panic!("expected conflicting def error"),
            Err(err) => err,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("conflicting definitions"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn ignores_manifest_nodes_in_import_assets() {
        let tmp = TempDir::new().expect("tmp");
        let world_air = tmp.path().join("air");
        let import_air = tmp.path().join("sdk-exports");
        fs::create_dir_all(&world_air).expect("mkdir world air");
        fs::create_dir_all(&import_air).expect("mkdir import air");

        let manifest = Manifest {
            air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
            schemas: Vec::new(),
            modules: Vec::new(),
            effects: Vec::new(),
            caps: Vec::new(),
            policies: Vec::new(),
            secrets: Vec::new(),
            defaults: None,
            module_bindings: IndexMap::new(),
            routing: None,
        };
        write_node(
            &world_air.join("manifest.air.json"),
            &[AirNode::Manifest(manifest.clone())],
        );
        write_node(
            &import_air.join("manifest.air.json"),
            &[AirNode::Manifest(manifest)],
        );

        let store = Arc::new(FsStore::open(tmp.path()).expect("store"));
        load_from_assets_with_imports(store, &world_air, &[import_air])
            .expect("load should ignore import manifest nodes");
    }

    fn write_node(path: &Path, nodes: &[AirNode]) {
        let json = if nodes.len() == 1 {
            serde_json::to_string_pretty(&nodes[0]).unwrap()
        } else {
            serde_json::to_string_pretty(nodes).unwrap()
        };
        fs::write(path, json).unwrap();
    }

    fn named_ref_from_node(node: &AirNode) -> aos_air_types::NamedRef {
        let hash = Hash::of_cbor(node).expect("hash");
        let name = match node {
            AirNode::Defschema(schema) => schema.name.clone(),
            AirNode::Defmodule(module) => module.name.clone(),
            AirNode::Defcap(cap) => cap.name.clone(),
            AirNode::Defpolicy(policy) => policy.name.clone(),
            AirNode::Defsecret(secret) => secret.name.clone(),
            AirNode::Defeffect(effect) => effect.name.clone(),
            AirNode::Manifest(_) => panic!("cannot build ref for manifest"),
        };
        aos_air_types::NamedRef {
            name,
            hash: aos_air_types::HashRef::new(hash.to_hex()).unwrap(),
        }
    }

    fn nat_type() -> TypeExpr {
        TypeExpr::Primitive(TypePrimitive::Nat(TypePrimitiveNat {
            nat: Default::default(),
        }))
    }
}
