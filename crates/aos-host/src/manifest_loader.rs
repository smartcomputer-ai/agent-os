//! Load AIR manifests from JSON asset directories.
//!
//! This module provides utilities to load AIR JSON files from directories and
//! produce a `LoadedManifest` suitable for kernel initialization.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use aos_air_types::{
    self as air_types, AirNode, DefCap, DefEffect, DefModule, DefPlan, DefPolicy, DefSchema,
    DefSecret, HashRef, Manifest, Name, NamedRef, SecretEntry, catalog::EffectCatalog,
};
use aos_kernel::{LoadedManifest, governance::ManifestPatch};
use aos_store::{Catalog, FsStore, Store, load_manifest_from_bytes};
use log::warn;
use serde_json::Value;
use walkdir::WalkDir;

const ZERO_HASH_SENTINEL: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// Attempts to load a manifest for the provided example directory by reading AIR JSON assets
/// under `air/`, versioned `air.*` bundles, `plans/` (legacy), and `defs/`. The `asset_root`
/// can itself be an AIR bundle (e.g., `examples/06-safe-upgrade/air.v2`). Returns `Ok(None)`
/// if no manifest is found so callers can fall back to the legacy Rust-built manifests.
pub fn load_from_assets(store: Arc<FsStore>, asset_root: &Path) -> Result<Option<LoadedManifest>> {
    let mut manifest: Option<Manifest> = None;
    let mut schemas: Vec<DefSchema> = Vec::new();
    let mut modules: Vec<DefModule> = Vec::new();
    let mut plans: Vec<DefPlan> = Vec::new();
    let mut caps: Vec<DefCap> = Vec::new();
    let mut policies: Vec<DefPolicy> = Vec::new();
    let mut secrets: Vec<DefSecret> = Vec::new();
    let mut effects: Vec<DefEffect> = Vec::new();

    for dir_path in asset_search_dirs(asset_root)? {
        for path in collect_json_files(&dir_path)? {
            let nodes = parse_air_nodes(&path)
                .with_context(|| format!("parse AIR nodes from {}", path.display()))?;
            for node in nodes {
                match node {
                    AirNode::Manifest(found) => {
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
                    AirNode::Defplan(plan) => plans.push(plan),
                    AirNode::Defcap(cap) => caps.push(cap),
                    AirNode::Defpolicy(policy) => policies.push(policy),
                    AirNode::Defsecret(secret) => secrets.push(secret),
                    AirNode::Defeffect(effect) => effects.push(effect),
                }
            }
        }
    }

    let mut manifest = match manifest {
        Some(manifest) => manifest,
        None => return Ok(None),
    };

    let hashes = write_nodes(
        &store, schemas, modules, plans, caps, policies, secrets, effects,
    )?;
    patch_manifest_refs(&mut manifest, &hashes)?;
    let catalog = manifest_catalog(&store, manifest)?;
    Ok(Some(catalog_to_loaded(catalog)))
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
    nodes.extend(loaded.plans.values().cloned().map(AirNode::Defplan));
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
    plans: Vec<DefPlan>,
    caps: Vec<DefCap>,
    policies: Vec<DefPolicy>,
    secrets: Vec<DefSecret>,
    effects: Vec<DefEffect>,
) -> Result<StoredHashes> {
    ensure_unique_names(&schemas, "defschema")?;
    ensure_unique_names(&modules, "defmodule")?;
    ensure_unique_names(&plans, "defplan")?;
    ensure_unique_names(&caps, "defcap")?;
    ensure_unique_names(&policies, "defpolicy")?;
    ensure_unique_names(&secrets, "defsecret")?;
    ensure_unique_names(&effects, "defeffect")?;

    let mut hashes = StoredHashes::default();
    for schema in schemas {
        let name = schema.name.clone();
        let hash = store
            .put_node(&AirNode::Defschema(schema))
            .context("store defschema node")?;
        hashes.schemas.insert(name, HashRef::new(hash.to_hex())?);
    }
    for module in modules {
        let name = module.name.clone();
        let hash = store
            .put_node(&AirNode::Defmodule(module))
            .context("store defmodule node")?;
        hashes.modules.insert(name, HashRef::new(hash.to_hex())?);
    }
    for plan in plans {
        let name = plan.name.clone();
        let hash = store
            .put_node(&AirNode::Defplan(plan))
            .context("store defplan node")?;
        hashes.plans.insert(name, HashRef::new(hash.to_hex())?);
    }
    for cap in caps {
        let name = cap.name.clone();
        let hash = store
            .put_node(&AirNode::Defcap(cap))
            .context("store defcap node")?;
        hashes.caps.insert(name, HashRef::new(hash.to_hex())?);
    }
    for policy in policies {
        let name = policy.name.clone();
        let hash = store
            .put_node(&AirNode::Defpolicy(policy))
            .context("store defpolicy node")?;
        hashes.policies.insert(name, HashRef::new(hash.to_hex())?);
    }
    for secret in secrets {
        let name = secret.name.clone();
        let hash = store
            .put_node(&AirNode::Defsecret(secret))
            .context("store defsecret node")?;
        hashes.secrets.insert(name, HashRef::new(hash.to_hex())?);
    }
    for effect in effects {
        let name = effect.name.clone();
        let hash = store
            .put_node(&AirNode::Defeffect(effect))
            .context("store defeffect node")?;
        hashes.effects.insert(name, HashRef::new(hash.to_hex())?);
    }
    Ok(hashes)
}

fn ensure_unique_names<T>(items: &[T], kind: &str) -> Result<()>
where
    T: HasName,
{
    let mut seen = HashSet::new();
    for item in items {
        let name = item.name();
        if !seen.insert(name.clone()) {
            bail!("duplicate {kind} '{name}' detected in assets");
        }
    }
    Ok(())
}

#[derive(Default)]
struct StoredHashes {
    schemas: HashMap<Name, HashRef>,
    modules: HashMap<Name, HashRef>,
    plans: HashMap<Name, HashRef>,
    effects: HashMap<Name, HashRef>,
    caps: HashMap<Name, HashRef>,
    policies: HashMap<Name, HashRef>,
    secrets: HashMap<Name, HashRef>,
}

fn patch_manifest_refs(manifest: &mut Manifest, hashes: &StoredHashes) -> Result<()> {
    patch_named_refs("schema", &mut manifest.schemas, &hashes.schemas)?;
    patch_named_refs("module", &mut manifest.modules, &hashes.modules)?;
    patch_named_refs("plan", &mut manifest.plans, &hashes.plans)?;
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
            SecretEntry::Decl(_) => bail!(
                "inline secret declarations are unsupported; provide defsecret nodes instead"
            ),
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
    for key in ["schemas", "modules", "plans", "caps", "policies", "effects"] {
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

trait HasName {
    fn name(&self) -> Name;
}

impl HasName for DefSchema {
    fn name(&self) -> Name {
        self.name.clone()
    }
}

impl HasName for DefModule {
    fn name(&self) -> Name {
        self.name.clone()
    }
}

impl HasName for DefPlan {
    fn name(&self) -> Name {
        self.name.clone()
    }
}

impl HasName for DefEffect {
    fn name(&self) -> Name {
        self.name.clone()
    }
}

impl HasName for DefCap {
    fn name(&self) -> Name {
        self.name.clone()
    }
}

impl HasName for DefPolicy {
    fn name(&self) -> Name {
        self.name.clone()
    }
}

impl HasName for DefSecret {
    fn name(&self) -> Name {
        self.name.clone()
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
    let mut plans = HashMap::new();
    let mut effects = HashMap::new();
    let mut caps = HashMap::new();
    let mut policies = HashMap::new();
    let mut schemas = HashMap::new();

    for (_name, entry) in nodes {
        match entry.node {
            AirNode::Defmodule(module) => {
                modules.insert(module.name.clone(), module);
            }
            AirNode::Defplan(plan) => {
                plans.insert(plan.name.clone(), plan);
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
        plans,
        effects,
        caps,
        policies,
        schemas,
        effect_catalog,
    }
}

fn asset_search_dirs(asset_root: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if asset_root
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with("air") || n == "defs" || n == "plans")
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
            if name == "defs" || name == "plans" || name.starts_with("air") {
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
        HashRef, ModuleAbi, ModuleKind, ReducerAbi, SchemaRef, TypeExpr, TypePrimitive,
        TypePrimitiveNat,
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
        let event_schema = DefSchema {
            name: "demo/Event@1".into(),
            ty: nat_type(),
        };

        let module = DefModule {
            name: "demo/Reducer@1".into(),
            module_kind: ModuleKind::Reducer,
            wasm_hash: HashRef::new(Hash::of_bytes(b"wasm").to_hex()).unwrap(),
            key_schema: None,
            abi: ModuleAbi {
                reducer: Some(ReducerAbi {
                    state: SchemaRef::new(&state_schema.name).unwrap(),
                    event: SchemaRef::new(&event_schema.name).unwrap(),
                    annotations: None,
                    effects_emitted: Vec::new(),
                    cap_slots: IndexMap::new(),
                }),
            },
        };

        let state_node = AirNode::Defschema(state_schema.clone());
        let event_node = AirNode::Defschema(event_schema.clone());
        let module_node = AirNode::Defmodule(module.clone());
        write_node(
            &air_dir.join("schemas.air.json"),
            &[state_node.clone(), event_node.clone()],
        );
        write_node(&air_dir.join("module.air.json"), &[module_node.clone()]);

        let mut manifest = Manifest {
            air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
            schemas: vec![
                named_ref_from_node(&state_node),
                named_ref_from_node(&event_node),
            ],
            modules: vec![named_ref_from_node(&module_node)],
            plans: Vec::new(),
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
            triggers: Vec::new(),
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
        assert!(loaded.schemas.contains_key("demo/Event@1"));
    }

    #[test]
    fn returns_none_without_manifest() {
        let tmp = TempDir::new().expect("tmp");
        let store = Arc::new(FsStore::open(tmp.path()).expect("store"));
        let result = load_from_assets(store, tmp.path()).expect("ok");
        assert!(result.is_none());
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
            AirNode::Defplan(plan) => plan.name.clone(),
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
