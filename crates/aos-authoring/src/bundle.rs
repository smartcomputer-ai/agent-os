//! World IO helpers for AIR bundle import/export and patch doc construction.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use aos_air_types::{
    AirNode, DefEffect, DefModule, DefSchema, DefSecret, HashRef, Manifest, NamedRef, SecretEntry,
    builtins,
};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_kernel::LoadedManifest;
use aos_kernel::Store;
use aos_kernel::patch_doc::{
    AddDef, ManifestRef, PatchDocument, PatchOp, SetManifestRefs, SetRoutingEvents,
    SetRoutingInboxes, SetSecrets,
};

use crate::manifest_loader;

#[derive(Debug, Clone, Copy)]
pub enum BundleFilter {
    AirOnly,
    Full,
}

#[derive(Debug, Clone)]
pub struct WorldBundle {
    pub manifest: Manifest,
    pub schemas: Vec<DefSchema>,
    pub modules: Vec<DefModule>,
    pub effects: Vec<DefEffect>,
    pub secrets: Vec<DefSecret>,
    pub wasm_blobs: Option<std::collections::BTreeMap<String, Vec<u8>>>,
}

#[derive(Debug, Clone, Copy)]
pub struct ExportOptions {
    pub include_sys: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { include_sys: false }
    }
}

#[derive(Debug, Clone)]
pub struct ExportedBundle {
    pub bundle: WorldBundle,
    pub manifest_hash: String,
    pub manifest_bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct WriteOptions {
    pub include_sys: bool,
    pub defs_bundle: bool,
    pub strip_wasm_hashes: bool,
    pub write_manifest_cbor: bool,
    pub air_dir: Option<std::path::PathBuf>,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            include_sys: false,
            defs_bundle: false,
            strip_wasm_hashes: false,
            write_manifest_cbor: true,
            air_dir: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GenesisImport {
    pub manifest_hash: String,
    pub manifest_bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum ImportMode {
    Genesis,
    Patch {
        base_manifest: Manifest,
        base_manifest_hash: String,
    },
}

#[derive(Debug)]
pub enum ImportOutcome {
    Genesis(GenesisImport),
    Patch(PatchDocument),
}

pub fn load_air_bundle<S: Store + 'static>(
    store: Arc<S>,
    dir: &Path,
    _filter: BundleFilter,
) -> Result<WorldBundle> {
    let assets = manifest_loader::load_from_assets_with_defs(store, dir)
        .with_context(|| format!("load AIR bundle from {}", dir.display()))?
        .ok_or_else(|| anyhow::anyhow!("no manifest found under {}", dir.display()))?;
    Ok(WorldBundle::from_loaded_assets(
        assets.loaded,
        assets.secrets,
    ))
}

pub fn import_genesis<S: Store>(store: &S, bundle: &WorldBundle) -> Result<GenesisImport> {
    let stored = store_bundle_defs(store, bundle)?;
    let manifest = patch_manifest_for_genesis(&bundle.manifest, &stored)?;
    store.put_node(&manifest).context("store manifest")?;
    let manifest_bytes =
        to_canonical_cbor(&manifest).context("encode manifest to canonical CBOR")?;
    let manifest_hash = Hash::of_bytes(&manifest_bytes).to_hex();
    Ok(GenesisImport {
        manifest_hash,
        manifest_bytes,
    })
}

pub fn import_bundle<S: Store>(
    store: &S,
    bundle: &WorldBundle,
    mode: ImportMode,
) -> Result<ImportOutcome> {
    match mode {
        ImportMode::Genesis => Ok(ImportOutcome::Genesis(import_genesis(store, bundle)?)),
        ImportMode::Patch {
            base_manifest,
            base_manifest_hash,
        } => {
            let doc = build_patch_document(bundle, &base_manifest, &base_manifest_hash)?;
            Ok(ImportOutcome::Patch(doc))
        }
    }
}

pub fn export_bundle<S: Store>(
    store: &S,
    manifest_hash: &str,
    options: ExportOptions,
) -> Result<ExportedBundle> {
    let hash = Hash::from_hex_str(manifest_hash).context("parse manifest hash")?;
    let manifest: Manifest = store
        .get_node(hash)
        .context("load manifest node from store")?;
    let manifest_bytes = manifest_node_bytes(&manifest)?;
    let computed_hash = Hash::of_bytes(&manifest_bytes).to_hex();
    if computed_hash != manifest_hash {
        bail!("manifest hash mismatch: expected {manifest_hash}, computed {computed_hash}");
    }

    let catalog = aos_kernel::load_manifest_from_bytes(store, &manifest_bytes)
        .context("load manifest catalog")?;
    let mut bundle = bundle_from_catalog(catalog, options.include_sys);
    if options.include_sys {
        extend_with_builtins(&mut bundle);
    }
    bundle.manifest = manifest;
    bundle.sort_defs();
    Ok(ExportedBundle {
        bundle,
        manifest_hash: computed_hash,
        manifest_bytes,
    })
}

pub fn build_patch_document(
    bundle: &WorldBundle,
    base_manifest: &Manifest,
    base_manifest_hash: &str,
) -> Result<PatchDocument> {
    let mut patches = Vec::new();
    let mut secrets_pre_state = base_manifest.secrets.clone();
    let referenced_secret_names: std::collections::BTreeSet<String> = bundle
        .manifest
        .secrets
        .iter()
        .filter_map(|entry| match entry {
            aos_air_types::SecretEntry::Ref(named) if !is_sys_name(named.name.as_str()) => {
                Some(named.name.to_string())
            }
            _ => None,
        })
        .collect();

    for schema in &bundle.schemas {
        if is_sys_name(schema.name.as_str()) {
            continue;
        }
        patches.push(PatchOp::AddDef {
            add_def: AddDef {
                kind: "defschema".to_string(),
                node: AirNode::Defschema(schema.clone()),
            },
        });
    }
    for module in &bundle.modules {
        if is_sys_name(module.name.as_str()) {
            continue;
        }
        patches.push(PatchOp::AddDef {
            add_def: AddDef {
                kind: "defmodule".to_string(),
                node: AirNode::Defmodule(module.clone()),
            },
        });
    }
    for effect in &bundle.effects {
        if is_sys_name(effect.name.as_str()) {
            continue;
        }
        patches.push(PatchOp::AddDef {
            add_def: AddDef {
                kind: "defeffect".to_string(),
                node: AirNode::Defeffect(effect.clone()),
            },
        });
    }
    for secret in &bundle.secrets {
        if is_sys_name(secret.name.as_str())
            || !referenced_secret_names.contains(secret.name.as_str())
        {
            continue;
        }
        apply_placeholder_secret_ref(&mut secrets_pre_state, secret.name.as_str())?;
        patches.push(PatchOp::AddDef {
            add_def: AddDef {
                kind: "defsecret".to_string(),
                node: AirNode::Defsecret(secret.clone()),
            },
        });
    }

    let mut add_refs = Vec::new();
    add_refs.extend(manifest_refs_from(
        "defschema",
        &filter_sys_refs(&bundle.manifest.schemas),
    ));
    add_refs.extend(manifest_refs_from(
        "defmodule",
        &filter_sys_refs(&bundle.manifest.modules),
    ));
    add_refs.extend(manifest_refs_from(
        "defeffect",
        &filter_sys_refs(&bundle.manifest.effects),
    ));
    // Secrets are updated atomically via SetSecrets below. Adding defsecret refs
    // here would mutate manifest.secrets before SetSecrets pre-hash verification.
    if !add_refs.is_empty() {
        patches.push(PatchOp::SetManifestRefs {
            set_manifest_refs: SetManifestRefs {
                add: add_refs,
                remove: Vec::new(),
            },
        });
    }

    let base_events = base_manifest
        .routing
        .as_ref()
        .map(|r| r.subscriptions.clone())
        .unwrap_or_default();
    let pre_hash = Hash::of_cbor(&base_events)
        .context("hash base routing.subscriptions")?
        .to_hex();
    let subscriptions = bundle
        .manifest
        .routing
        .as_ref()
        .map(|r| r.subscriptions.clone())
        .unwrap_or_default();
    patches.push(PatchOp::SetRoutingEvents {
        set_routing_events: SetRoutingEvents {
            pre_hash,
            subscriptions,
        },
    });

    let base_inboxes = base_manifest
        .routing
        .as_ref()
        .map(|r| r.inboxes.clone())
        .unwrap_or_default();
    let pre_hash = Hash::of_cbor(&base_inboxes)
        .context("hash base routing.inboxes")?
        .to_hex();
    let inboxes = bundle
        .manifest
        .routing
        .as_ref()
        .map(|r| r.inboxes.clone())
        .unwrap_or_default();
    patches.push(PatchOp::SetRoutingInboxes {
        set_routing_inboxes: SetRoutingInboxes { pre_hash, inboxes },
    });

    let pre_hash = Hash::of_cbor(&secrets_pre_state)
        .context("hash pre-state secrets for set_secrets")?
        .to_hex();
    patches.push(PatchOp::SetSecrets {
        set_secrets: SetSecrets {
            pre_hash,
            secrets: bundle.manifest.secrets.clone(),
        },
    });

    Ok(PatchDocument {
        version: "1".to_string(),
        base_manifest_hash: base_manifest_hash.to_string(),
        patches,
    })
}

pub fn manifest_node_hash(manifest: &Manifest) -> Result<String> {
    let bytes = manifest_node_bytes(manifest)?;
    Ok(Hash::of_bytes(&bytes).to_hex())
}

#[derive(Default)]
struct StoredBundleHashes {
    schemas: std::collections::HashMap<String, HashRef>,
    modules: std::collections::HashMap<String, HashRef>,
    effects: std::collections::HashMap<String, HashRef>,
    secrets: std::collections::HashMap<String, HashRef>,
}

fn store_bundle_defs<S: Store>(store: &S, bundle: &WorldBundle) -> Result<StoredBundleHashes> {
    let mut stored = StoredBundleHashes::default();
    for schema in &bundle.schemas {
        let hash = store
            .put_node(&AirNode::Defschema(schema.clone()))
            .context("store defschema")?;
        stored.schemas.insert(
            schema.name.to_string(),
            HashRef::new(hash.to_hex()).context("hash defschema")?,
        );
    }
    for module in &bundle.modules {
        let hash = store
            .put_node(&AirNode::Defmodule(module.clone()))
            .context("store defmodule")?;
        stored.modules.insert(
            module.name.to_string(),
            HashRef::new(hash.to_hex()).context("hash defmodule")?,
        );
    }
    for effect in &bundle.effects {
        let hash = store
            .put_node(&AirNode::Defeffect(effect.clone()))
            .context("store defeffect")?;
        stored.effects.insert(
            effect.name.to_string(),
            HashRef::new(hash.to_hex()).context("hash defeffect")?,
        );
    }
    for secret in &bundle.secrets {
        let hash = store
            .put_node(&AirNode::Defsecret(secret.clone()))
            .context("store defsecret")?;
        stored.secrets.insert(
            secret.name.to_string(),
            HashRef::new(hash.to_hex()).context("hash defsecret")?,
        );
    }
    Ok(stored)
}

fn patch_manifest_for_genesis(
    manifest: &Manifest,
    stored: &StoredBundleHashes,
) -> Result<Manifest> {
    let mut manifest = manifest.clone();
    patch_named_refs_for_genesis("schema", &mut manifest.schemas, &stored.schemas)?;
    patch_named_refs_for_genesis("module", &mut manifest.modules, &stored.modules)?;
    patch_named_refs_for_genesis("effect", &mut manifest.effects, &stored.effects)?;
    let mut secret_refs = manifest_secret_refs(&manifest.secrets)?;
    patch_named_refs_for_genesis("secret", &mut secret_refs, &stored.secrets)?;
    manifest.secrets = secret_refs.into_iter().map(SecretEntry::Ref).collect();
    Ok(manifest)
}

fn manifest_secret_refs(entries: &[SecretEntry]) -> Result<Vec<NamedRef>> {
    let mut refs = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            SecretEntry::Ref(reference) => refs.push(reference.clone()),
            SecretEntry::Decl(_) => bail!("inline secret declarations are unsupported in bundles"),
        }
    }
    Ok(refs)
}

fn patch_named_refs_for_genesis(
    kind: &str,
    refs: &mut [NamedRef],
    stored: &std::collections::HashMap<String, HashRef>,
) -> Result<()> {
    for reference in refs {
        let actual = if is_sys_name(reference.name.as_str()) {
            match kind {
                "schema" => builtins::find_builtin_schema(reference.name.as_str())
                    .map(|builtin| builtin.hash_ref.clone()),
                "module" => stored.get(reference.name.as_str()).cloned().or_else(|| {
                    builtins::find_builtin_module(reference.name.as_str())
                        .map(|builtin| builtin.hash_ref.clone())
                }),
                "effect" => builtins::find_builtin_effect(reference.name.as_str())
                    .map(|builtin| builtin.hash_ref.clone()),
                _ => None,
            }
        } else {
            match kind {
                "schema" | "module" | "effect" | "secret" => {
                    stored.get(reference.name.as_str()).cloned()
                }
                _ => bail!("unsupported manifest ref kind '{kind}'"),
            }
        }
        .ok_or_else(|| {
            anyhow::anyhow!("manifest references unknown {kind} '{}'", reference.name)
        })?;
        reference.hash = actual;
    }
    Ok(())
}

pub fn manifest_node_bytes(manifest: &Manifest) -> Result<Vec<u8>> {
    to_canonical_cbor(manifest).context("encode manifest")
}

pub fn decode_manifest_bytes(bytes: &[u8]) -> Result<Manifest> {
    if let Ok(manifest) = serde_cbor::from_slice::<Manifest>(bytes) {
        return Ok(manifest);
    }
    if let Ok(manifest) = serde_json::from_slice::<Manifest>(bytes) {
        return Ok(manifest);
    }
    bail!("unsupported manifest encoding");
}

pub fn write_air_layout(bundle: &WorldBundle, manifest_cbor: &[u8], out_dir: &Path) -> Result<()> {
    write_air_layout_with_options(bundle, manifest_cbor, out_dir, WriteOptions::default())
}

pub fn write_air_layout_with_options(
    bundle: &WorldBundle,
    manifest_cbor: &[u8],
    out_dir: &Path,
    options: WriteOptions,
) -> Result<()> {
    let air_dir = options
        .air_dir
        .clone()
        .unwrap_or_else(|| out_dir.join("air"));
    fs::create_dir_all(&air_dir).context("create air dir")?;
    fs::create_dir_all(out_dir.join(".aos")).context("create .aos dir")?;

    let (schemas, sys_schemas) = split_sys_defs(&bundle.schemas, options.include_sys);
    let (modules, sys_modules) = split_sys_defs(&bundle.modules, options.include_sys);
    let (effects, sys_effects) = split_sys_defs(&bundle.effects, options.include_sys);
    let (secrets, sys_secrets) = split_sys_defs(&bundle.secrets, options.include_sys);

    write_json(
        &air_dir.join("manifest.air.json"),
        &AirNode::Manifest(bundle.manifest.clone()),
    )?;
    if options.defs_bundle {
        let defs = collect_def_nodes(schemas, modules, effects, secrets);
        write_node_array_with_options(
            &air_dir.join("defs.air.json"),
            defs,
            options.strip_wasm_hashes,
        )?;
    } else {
        write_node_array(
            &air_dir.join("schemas.air.json"),
            schemas.iter().cloned().map(AirNode::Defschema).collect(),
        )?;
        write_node_array_with_options(
            &air_dir.join("module.air.json"),
            modules.iter().cloned().map(AirNode::Defmodule).collect(),
            options.strip_wasm_hashes,
        )?;
        write_node_array(
            &air_dir.join("effects.air.json"),
            effects.iter().cloned().map(AirNode::Defeffect).collect(),
        )?;
        write_node_array(
            &air_dir.join("secrets.air.json"),
            secrets.iter().cloned().map(AirNode::Defsecret).collect(),
        )?;
    }

    if options.include_sys {
        let sys_nodes = collect_sys_nodes(sys_schemas, sys_modules, sys_effects, sys_secrets);
        write_node_array(&air_dir.join("sys.air.json"), sys_nodes)?;
    }

    if options.write_manifest_cbor {
        fs::write(out_dir.join(".aos/manifest.air.cbor"), manifest_cbor)
            .context("write manifest.air.cbor")?;
    }
    Ok(())
}

impl WorldBundle {
    pub fn from_loaded(loaded: LoadedManifest) -> Self {
        let mut bundle = WorldBundle {
            manifest: loaded.manifest,
            schemas: loaded.schemas.into_values().collect(),
            modules: loaded.modules.into_values().collect(),
            effects: loaded.effects.into_values().collect(),
            secrets: Vec::new(),
            wasm_blobs: None,
        };
        bundle.sort_defs();
        bundle
    }

    pub fn from_loaded_assets(loaded: LoadedManifest, secrets: Vec<DefSecret>) -> Self {
        WorldBundle {
            manifest: loaded.manifest,
            schemas: loaded.schemas.into_values().collect(),
            modules: loaded.modules.into_values().collect(),
            effects: loaded.effects.into_values().collect(),
            secrets,
            wasm_blobs: None,
        }
        .sorted()
    }

    fn sort_defs(&mut self) {
        self.schemas.sort_by(|a, b| a.name.cmp(&b.name));
        self.modules.sort_by(|a, b| a.name.cmp(&b.name));
        self.effects.sort_by(|a, b| a.name.cmp(&b.name));
        self.secrets.sort_by(|a, b| a.name.cmp(&b.name));
    }

    fn sorted(mut self) -> Self {
        self.sort_defs();
        self
    }
}

fn manifest_refs_from(kind: &str, refs: &[aos_air_types::NamedRef]) -> Vec<ManifestRef> {
    refs.iter()
        .map(|r| ManifestRef {
            kind: kind.to_string(),
            name: r.name.clone(),
            hash: r.hash.as_str().to_string(),
        })
        .collect()
}

fn is_sys_name(name: &str) -> bool {
    name.starts_with("sys/")
}

fn filter_sys_refs(refs: &[aos_air_types::NamedRef]) -> Vec<aos_air_types::NamedRef> {
    refs.iter()
        .filter(|r| !is_sys_name(r.name.as_str()))
        .cloned()
        .collect()
}

fn apply_placeholder_secret_ref(
    secrets: &mut Vec<aos_air_types::SecretEntry>,
    name: &str,
) -> Result<()> {
    let zero_hash = HashRef::new(format!("sha256:{}", "0".repeat(64)))?;
    if let Some(pos) = secrets
        .iter()
        .position(|entry| matches!(entry, aos_air_types::SecretEntry::Ref(named) if named.name.as_str() == name))
    {
        secrets[pos] = aos_air_types::SecretEntry::Ref(aos_air_types::NamedRef {
            name: name.to_string(),
            hash: zero_hash,
        });
    } else {
        secrets.push(aos_air_types::SecretEntry::Ref(aos_air_types::NamedRef {
            name: name.to_string(),
            hash: zero_hash,
        }));
    }
    Ok(())
}

fn write_json(path: &Path, value: &AirNode) -> Result<()> {
    let json = serde_json::to_string_pretty(value).context("serialize AIR node")?;
    fs::write(path, json).with_context(|| format!("write {}", path.display()))
}

fn write_node_array(path: &Path, nodes: Vec<AirNode>) -> Result<()> {
    if nodes.is_empty() {
        return Ok(());
    }
    let json = serde_json::to_string_pretty(&nodes).context("serialize AIR node array")?;
    fs::write(path, json).with_context(|| format!("write {}", path.display()))
}

fn write_node_array_with_options(
    path: &Path,
    nodes: Vec<AirNode>,
    strip_wasm_hashes: bool,
) -> Result<()> {
    if nodes.is_empty() {
        return Ok(());
    }
    if !strip_wasm_hashes {
        return write_node_array(path, nodes);
    }
    let mut values: Vec<serde_json::Value> = Vec::with_capacity(nodes.len());
    for node in nodes {
        let mut value = serde_json::to_value(&node).context("serialize AIR node array")?;
        strip_module_wasm_hash(&mut value);
        values.push(value);
    }
    let json = serde_json::to_string_pretty(&values).context("serialize AIR node array")?;
    fs::write(path, json).with_context(|| format!("write {}", path.display()))
}

fn strip_module_wasm_hash(value: &mut serde_json::Value) {
    let serde_json::Value::Object(map) = value else {
        return;
    };
    let kind = map.get("$kind").and_then(|v| v.as_str());
    if kind == Some("defmodule") {
        map.remove("wasm_hash");
    }
}

fn split_sys_defs<T: HasName + Clone>(defs: &[T], include_sys: bool) -> (Vec<T>, Vec<T>) {
    let mut normal = Vec::new();
    let mut sys = Vec::new();
    for def in defs {
        if def.name().starts_with("sys/") {
            if include_sys {
                sys.push(def.clone());
            }
        } else {
            normal.push(def.clone());
        }
    }
    (normal, sys)
}

fn collect_sys_nodes(
    schemas: Vec<DefSchema>,
    modules: Vec<DefModule>,
    effects: Vec<DefEffect>,
    secrets: Vec<DefSecret>,
) -> Vec<AirNode> {
    let mut nodes = Vec::new();
    nodes.extend(schemas.into_iter().map(AirNode::Defschema));
    nodes.extend(modules.into_iter().map(AirNode::Defmodule));
    nodes.extend(effects.into_iter().map(AirNode::Defeffect));
    nodes.extend(secrets.into_iter().map(AirNode::Defsecret));
    nodes
}

fn collect_def_nodes(
    schemas: Vec<DefSchema>,
    modules: Vec<DefModule>,
    effects: Vec<DefEffect>,
    secrets: Vec<DefSecret>,
) -> Vec<AirNode> {
    let mut nodes = Vec::new();
    nodes.extend(schemas.into_iter().map(AirNode::Defschema));
    nodes.extend(modules.into_iter().map(AirNode::Defmodule));
    nodes.extend(effects.into_iter().map(AirNode::Defeffect));
    nodes.extend(secrets.into_iter().map(AirNode::Defsecret));
    nodes
}

trait HasName {
    fn name(&self) -> &str;
}

impl HasName for DefSchema {
    fn name(&self) -> &str {
        self.name.as_str()
    }
}

impl HasName for DefModule {
    fn name(&self) -> &str {
        self.name.as_str()
    }
}

impl HasName for DefEffect {
    fn name(&self) -> &str {
        self.name.as_str()
    }
}

impl HasName for DefSecret {
    fn name(&self) -> &str {
        self.name.as_str()
    }
}

fn extend_with_builtins(bundle: &mut WorldBundle) {
    let mut existing: std::collections::HashSet<String> = std::collections::HashSet::new();
    for schema in &bundle.schemas {
        existing.insert(schema.name.clone());
    }
    for effect in &bundle.effects {
        existing.insert(effect.name.clone());
    }
    for module in &bundle.modules {
        existing.insert(module.name.clone());
    }

    for builtin in builtins::builtin_schemas() {
        if existing.insert(builtin.schema.name.clone()) {
            bundle.schemas.push(builtin.schema.clone());
        }
    }
    for builtin in builtins::builtin_effects() {
        if existing.insert(builtin.effect.name.clone()) {
            bundle.effects.push(builtin.effect.clone());
        }
    }
    for builtin in builtins::builtin_modules() {
        if existing.insert(builtin.module.name.clone()) {
            bundle.modules.push(builtin.module.clone());
        }
    }
}

fn bundle_from_catalog(catalog: aos_kernel::Catalog, include_sys: bool) -> WorldBundle {
    let mut schemas = Vec::new();
    let mut modules = Vec::new();
    let mut effects = Vec::new();
    let mut secrets = Vec::new();

    for (name, entry) in catalog.nodes {
        if !include_sys && name.starts_with("sys/") {
            continue;
        }
        match entry.node {
            AirNode::Defschema(schema) => schemas.push(schema),
            AirNode::Defmodule(module) => modules.push(module),
            AirNode::Defeffect(effect) => effects.push(effect),
            AirNode::Defsecret(secret) => secrets.push(secret),
            AirNode::Manifest(_) => {}
        }
    }

    WorldBundle {
        manifest: catalog.manifest,
        schemas,
        modules,
        effects,
        secrets,
        wasm_blobs: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::DefSecret;
    use aos_air_types::{
        CURRENT_AIR_VERSION, DefModule, DefSchema, EmptyObject, HashRef, ModuleAbi, ModuleKind,
        NamedRef, SchemaRef, SecretEntry, TypeExpr, TypePrimitive, TypePrimitiveBool, WorkflowAbi,
    };
    use aos_kernel::patch_doc::compile_patch_document;
    use aos_kernel::{MemStore, Store};
    use tempfile::tempdir;

    #[test]
    fn manifest_node_hash_matches_store() {
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: Vec::new(),
            modules: Vec::new(),
            effects: Vec::new(),
            effect_bindings: vec![],
            secrets: Vec::new(),
            routing: None,
        };
        let store = MemStore::new();
        let stored = store.put_node(&manifest).expect("store manifest");
        let computed = manifest_node_hash(&manifest).expect("compute hash");
        assert_eq!(stored.to_hex(), computed);
    }

    #[test]
    fn export_import_round_trip_manifest_hash() {
        let store = MemStore::new();
        let schema = DefSchema {
            name: "demo/State@1".into(),
            ty: TypeExpr::Primitive(TypePrimitive::Bool(TypePrimitiveBool {
                bool: EmptyObject::default(),
            })),
        };
        let schema_hash = store
            .put_node(&AirNode::Defschema(schema.clone()))
            .expect("store schema");
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: vec![NamedRef {
                name: schema.name.clone(),
                hash: HashRef::new(schema_hash.to_hex()).expect("hash ref"),
            }],
            modules: Vec::new(),
            effects: Vec::new(),
            effect_bindings: vec![],
            secrets: Vec::new(),
            routing: None,
        };
        let manifest_hash = store
            .put_node(&manifest.clone())
            .expect("store manifest")
            .to_hex();

        let exported =
            export_bundle(&store, &manifest_hash, ExportOptions::default()).expect("export bundle");
        assert_eq!(exported.manifest_hash, manifest_hash);
        assert_eq!(exported.bundle.schemas.len(), 1);

        let store2 = MemStore::new();
        let imported = import_genesis(&store2, &exported.bundle).expect("import genesis");
        assert_eq!(imported.manifest_hash, manifest_hash);
        assert!(store2.has_node(schema_hash).expect("schema stored"));
        let manifest_node_hash =
            Hash::from_hex_str(&imported.manifest_hash).expect("manifest hash parse");
        assert!(
            store2
                .has_node(manifest_node_hash)
                .expect("manifest stored"),
            "manifest node should be stored in CAS"
        );
    }

    #[test]
    fn import_genesis_rewrites_manifest_refs_to_imported_defs() {
        let store = MemStore::new();
        let old_module = DefModule {
            name: "demo/Workflow@1".into(),
            module_kind: ModuleKind::Workflow,
            wasm_hash: HashRef::new(format!("sha256:{}", "0".repeat(64))).expect("zero hash"),
            key_schema: None,
            abi: ModuleAbi {
                workflow: Some(WorkflowAbi {
                    state: SchemaRef::new("demo/State@1").expect("state schema"),
                    event: SchemaRef::new("demo/Event@1").expect("event schema"),
                    context: None,
                    annotations: None,
                    effects_emitted: Vec::new(),
                }),
                pure: None,
            },
        };
        let old_hash = store
            .put_node(&AirNode::Defmodule(old_module.clone()))
            .expect("store old module");
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: Vec::new(),
            modules: vec![NamedRef {
                name: old_module.name.clone(),
                hash: HashRef::new(old_hash.to_hex()).expect("module hash ref"),
            }],
            effects: Vec::new(),
            effect_bindings: vec![],
            secrets: Vec::new(),
            routing: None,
        };

        let new_module = DefModule {
            wasm_hash: HashRef::new(Hash::of_bytes(b"patched-wasm").to_hex()).expect("wasm hash"),
            ..old_module
        };
        let bundle = WorldBundle {
            manifest,
            schemas: Vec::new(),
            modules: vec![new_module.clone()],
            effects: Vec::new(),
            secrets: Vec::new(),
            wasm_blobs: None,
        };

        let imported = import_genesis(&store, &bundle).expect("import genesis");
        let imported_manifest_hash =
            Hash::from_hex_str(&imported.manifest_hash).expect("parse manifest hash");
        let imported_manifest: Manifest = store
            .get_node(imported_manifest_hash)
            .expect("load imported manifest");
        let imported_ref = imported_manifest
            .modules
            .first()
            .expect("module ref")
            .hash
            .clone();
        assert_ne!(
            imported_ref.as_str(),
            old_hash.to_hex(),
            "manifest should not keep stale module hash"
        );
        let imported_module: DefModule = store
            .get_node(Hash::from_hex_str(imported_ref.as_str()).expect("parse imported ref"))
            .expect("load imported module");
        assert_eq!(imported_module.wasm_hash, new_module.wasm_hash);
    }

    #[test]
    fn export_with_sys_includes_builtins() {
        let store = MemStore::new();
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: Vec::new(),
            modules: Vec::new(),
            effects: Vec::new(),
            effect_bindings: vec![],
            secrets: Vec::new(),
            routing: None,
        };
        let manifest_hash = store.put_node(&manifest).expect("store manifest").to_hex();

        let exported = export_bundle(&store, &manifest_hash, ExportOptions { include_sys: true })
            .expect("export bundle");
        let has_sys_schema = exported
            .bundle
            .schemas
            .iter()
            .any(|s| s.name.as_str().starts_with("sys/"));
        let has_sys_effect = exported
            .bundle
            .effects
            .iter()
            .any(|e| e.name.as_str().starts_with("sys/"));
        assert!(has_sys_schema, "expected built-in sys schema");
        assert!(has_sys_effect, "expected built-in sys effect");
    }

    #[test]
    fn write_defs_bundle_emits_single_defs_file() {
        let store = MemStore::new();
        let schema = DefSchema {
            name: "demo/State@1".into(),
            ty: TypeExpr::Primitive(TypePrimitive::Bool(TypePrimitiveBool {
                bool: EmptyObject::default(),
            })),
        };
        let schema_hash = store
            .put_node(&AirNode::Defschema(schema.clone()))
            .expect("store schema");
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: vec![NamedRef {
                name: schema.name.clone(),
                hash: HashRef::new(schema_hash.to_hex()).expect("hash ref"),
            }],
            modules: Vec::new(),
            effects: Vec::new(),
            effect_bindings: vec![],
            secrets: Vec::new(),
            routing: None,
        };
        let bundle = WorldBundle {
            manifest: manifest.clone(),
            schemas: vec![schema.clone()],
            modules: Vec::new(),
            effects: Vec::new(),
            secrets: Vec::new(),
            wasm_blobs: None,
        };

        let tmp = tempdir().expect("tempdir");
        let manifest_bytes = manifest_node_bytes(&manifest).expect("manifest bytes");
        write_air_layout_with_options(
            &bundle,
            &manifest_bytes,
            tmp.path(),
            WriteOptions {
                include_sys: false,
                defs_bundle: true,
                strip_wasm_hashes: false,
                write_manifest_cbor: true,
                air_dir: None,
            },
        )
        .expect("write layout");

        let defs_path = tmp.path().join("air/defs.air.json");
        assert!(defs_path.exists(), "defs bundle should exist");
        assert!(
            !tmp.path().join("air/schemas.air.json").exists(),
            "per-kind file should not be written"
        );
        let defs_json = fs::read_to_string(&defs_path).expect("read defs bundle");
        let nodes: Vec<AirNode> = serde_json::from_str(&defs_json).expect("parse defs bundle");
        assert!(
            nodes
                .iter()
                .any(|node| matches!(node, AirNode::Defschema(def) if def.name == schema.name)),
            "defs bundle should include schema"
        );
    }

    #[test]
    fn patch_doc_compile_with_secret_defs_uses_set_secrets_only() {
        let store = MemStore::new();
        let base_manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: Vec::new(),
            modules: Vec::new(),
            effects: Vec::new(),
            effect_bindings: vec![],
            secrets: Vec::new(),
            routing: None,
        };
        let base_hash = store.put_node(&base_manifest).expect("store base").to_hex();

        let secret = DefSecret {
            name: "llm/openai_api@1".into(),
            binding_id: "env:OPENAI_API_KEY".into(),
            expected_digest: None,
        };
        let secret_hash = store
            .put_node(&AirNode::Defsecret(secret.clone()))
            .expect("store secret")
            .to_hex();
        let mut bundle_manifest = base_manifest.clone();
        bundle_manifest.secrets = vec![SecretEntry::Ref(NamedRef {
            name: secret.name.clone(),
            hash: HashRef::new(secret_hash).expect("hash ref"),
        })];
        let bundle = WorldBundle {
            manifest: bundle_manifest,
            schemas: Vec::new(),
            modules: Vec::new(),
            effects: Vec::new(),
            secrets: vec![secret],
            wasm_blobs: None,
        };

        let doc =
            build_patch_document(&bundle, &base_manifest, &base_hash).expect("build patch doc");
        compile_patch_document(&store, doc).expect("compile patch doc");
    }
}
