//! World IO helpers for AIR bundle import/export and patch doc construction.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use aos_air_types::{
    AirNode, DefCap, DefEffect, DefModule, DefPlan, DefPolicy, DefSchema, DefSecret, Manifest,
    SecretEntry,
};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_kernel::LoadedManifest;
use aos_kernel::patch_doc::{
    AddDef, ManifestRef, PatchDocument, PatchOp, SetDefaults, SetManifestRefs, SetModuleBindings,
    SetRoutingEvents, SetRoutingInboxes, SetSecrets, SetTriggers,
};
use aos_store::{FsStore, Store};

use crate::control::ControlClient;
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
    pub plans: Vec<DefPlan>,
    pub caps: Vec<DefCap>,
    pub policies: Vec<DefPolicy>,
    pub effects: Vec<DefEffect>,
    pub secrets: Vec<DefSecret>,
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

#[derive(Debug, Clone, Copy)]
pub struct WriteOptions {
    pub include_sys: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self { include_sys: false }
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

#[derive(Debug, Clone)]
pub struct BaseManifest {
    pub manifest: Manifest,
    pub hash: String,
    pub bytes: Vec<u8>,
}

pub fn load_air_bundle(
    store: Arc<FsStore>,
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
    for schema in &bundle.schemas {
        store
            .put_node(&AirNode::Defschema(schema.clone()))
            .context("store defschema")?;
    }
    for module in &bundle.modules {
        store
            .put_node(&AirNode::Defmodule(module.clone()))
            .context("store defmodule")?;
    }
    for plan in &bundle.plans {
        store
            .put_node(&AirNode::Defplan(plan.clone()))
            .context("store defplan")?;
    }
    for cap in &bundle.caps {
        store
            .put_node(&AirNode::Defcap(cap.clone()))
            .context("store defcap")?;
    }
    for policy in &bundle.policies {
        store
            .put_node(&AirNode::Defpolicy(policy.clone()))
            .context("store defpolicy")?;
    }
    for effect in &bundle.effects {
        store
            .put_node(&AirNode::Defeffect(effect.clone()))
            .context("store defeffect")?;
    }
    for secret in &bundle.secrets {
        store
            .put_node(&AirNode::Defsecret(secret.clone()))
            .context("store defsecret")?;
    }

    let manifest_node = AirNode::Manifest(bundle.manifest.clone());
    store
        .put_node(&manifest_node)
        .context("store manifest")?;
    let manifest_bytes =
        to_canonical_cbor(&manifest_node).context("encode manifest to canonical CBOR")?;
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
    let node: AirNode = store
        .get_node(hash)
        .context("load manifest node from store")?;
    let manifest = match node {
        AirNode::Manifest(manifest) => manifest,
        _ => bail!("manifest hash does not point to a manifest node"),
    };
    let manifest_bytes = manifest_node_bytes(&manifest)?;
    let computed_hash = Hash::of_bytes(&manifest_bytes).to_hex();
    if computed_hash != manifest_hash {
        bail!(
            "manifest hash mismatch: expected {manifest_hash}, computed {computed_hash}"
        );
    }

    let catalog = aos_store::load_manifest_from_bytes(store, &manifest_bytes)
        .context("load manifest catalog")?;
    let mut bundle = bundle_from_catalog(catalog, options.include_sys);
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

    for schema in &bundle.schemas {
        patches.push(PatchOp::AddDef {
            add_def: AddDef {
                kind: "defschema".to_string(),
                node: AirNode::Defschema(schema.clone()),
            },
        });
    }
    for module in &bundle.modules {
        patches.push(PatchOp::AddDef {
            add_def: AddDef {
                kind: "defmodule".to_string(),
                node: AirNode::Defmodule(module.clone()),
            },
        });
    }
    for plan in &bundle.plans {
        patches.push(PatchOp::AddDef {
            add_def: AddDef {
                kind: "defplan".to_string(),
                node: AirNode::Defplan(plan.clone()),
            },
        });
    }
    for cap in &bundle.caps {
        patches.push(PatchOp::AddDef {
            add_def: AddDef {
                kind: "defcap".to_string(),
                node: AirNode::Defcap(cap.clone()),
            },
        });
    }
    for policy in &bundle.policies {
        patches.push(PatchOp::AddDef {
            add_def: AddDef {
                kind: "defpolicy".to_string(),
                node: AirNode::Defpolicy(policy.clone()),
            },
        });
    }
    for effect in &bundle.effects {
        patches.push(PatchOp::AddDef {
            add_def: AddDef {
                kind: "defeffect".to_string(),
                node: AirNode::Defeffect(effect.clone()),
            },
        });
    }
    for secret in &bundle.secrets {
        patches.push(PatchOp::AddDef {
            add_def: AddDef {
                kind: "defsecret".to_string(),
                node: AirNode::Defsecret(secret.clone()),
            },
        });
    }

    let mut add_refs = Vec::new();
    add_refs.extend(manifest_refs_from("defschema", &bundle.manifest.schemas));
    add_refs.extend(manifest_refs_from("defmodule", &bundle.manifest.modules));
    add_refs.extend(manifest_refs_from("defplan", &bundle.manifest.plans));
    add_refs.extend(manifest_refs_from("defcap", &bundle.manifest.caps));
    add_refs.extend(manifest_refs_from("defpolicy", &bundle.manifest.policies));
    add_refs.extend(manifest_refs_from("defeffect", &bundle.manifest.effects));
    for secret in &bundle.manifest.secrets {
        if let SecretEntry::Ref(named) = secret {
            add_refs.push(ManifestRef {
                kind: "defsecret".to_string(),
                name: named.name.clone(),
                hash: named.hash.as_str().to_string(),
            });
        }
    }
    if !add_refs.is_empty() {
        patches.push(PatchOp::SetManifestRefs {
            set_manifest_refs: SetManifestRefs {
                add: add_refs,
                remove: Vec::new(),
            },
        });
    }

    if let Some(defaults) = &bundle.manifest.defaults {
        patches.push(PatchOp::SetDefaults {
            set_defaults: SetDefaults {
                policy: Some(defaults.policy.clone()),
                cap_grants: Some(defaults.cap_grants.clone()),
            },
        });
    }

    let base_events = base_manifest
        .routing
        .as_ref()
        .map(|r| r.events.clone())
        .unwrap_or_default();
    let pre_hash = Hash::of_cbor(&base_events)
        .context("hash base routing.events")?
        .to_hex();
    let events = bundle
        .manifest
        .routing
        .as_ref()
        .map(|r| r.events.clone())
        .unwrap_or_default();
    patches.push(PatchOp::SetRoutingEvents {
        set_routing_events: SetRoutingEvents { pre_hash, events },
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

    let pre_hash = Hash::of_cbor(&base_manifest.triggers)
        .context("hash base triggers")?
        .to_hex();
    patches.push(PatchOp::SetTriggers {
        set_triggers: SetTriggers {
            pre_hash,
            triggers: bundle.manifest.triggers.clone(),
        },
    });

    let pre_hash = Hash::of_cbor(&base_manifest.module_bindings)
        .context("hash base module_bindings")?
        .to_hex();
    patches.push(PatchOp::SetModuleBindings {
        set_module_bindings: SetModuleBindings {
            pre_hash,
            bindings: bundle.manifest.module_bindings.clone(),
        },
    });

    let pre_hash = Hash::of_cbor(&base_manifest.secrets)
        .context("hash base secrets")?
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

pub async fn resolve_base_manifest(
    store: &FsStore,
    base_override: Option<String>,
    mut control: Option<&mut ControlClient>,
    manifest_path: &Path,
) -> Result<BaseManifest> {
    if let Some(hash) = base_override {
        if let Ok(manifest) = manifest_from_store(store, &hash) {
            let bytes = manifest_node_bytes(&manifest)?;
            return Ok(BaseManifest {
                manifest,
                hash,
                bytes,
            });
        }
        let manifest = manifest_from_path(manifest_path)?;
        let bytes = manifest_node_bytes(&manifest)?;
        let computed = Hash::of_bytes(&bytes).to_hex();
        if computed != hash {
            bail!(
                "base manifest hash mismatch: expected {hash}, computed {computed}"
            );
        }
        return Ok(BaseManifest {
            manifest,
            hash,
            bytes,
        });
    }

    if let Some(client) = control.as_mut() {
        if let Ok((_meta, bytes)) = client.manifest_read("cli-base-manifest", None).await {
            let manifest = decode_manifest_bytes(&bytes)?;
            let hash = manifest_node_hash(&manifest)?;
            let bytes = manifest_node_bytes(&manifest)?;
            return Ok(BaseManifest {
                manifest,
                hash,
                bytes,
            });
        }
    }

    let manifest = manifest_from_path(manifest_path)?;
    let bytes = manifest_node_bytes(&manifest)?;
    let hash = Hash::of_bytes(&bytes).to_hex();
    if let Ok(from_store) = manifest_from_store(store, &hash) {
        return Ok(BaseManifest {
            manifest: from_store,
            hash,
            bytes,
        });
    }
    Ok(BaseManifest {
        manifest,
        hash,
        bytes,
    })
}

pub fn manifest_node_hash(manifest: &Manifest) -> Result<String> {
    let bytes = manifest_node_bytes(manifest)?;
    Ok(Hash::of_bytes(&bytes).to_hex())
}

pub fn manifest_node_bytes(manifest: &Manifest) -> Result<Vec<u8>> {
    let node = AirNode::Manifest(manifest.clone());
    to_canonical_cbor(&node).context("encode manifest node")
}

pub fn decode_manifest_bytes(bytes: &[u8]) -> Result<Manifest> {
    if let Ok(node) = serde_cbor::from_slice::<AirNode>(bytes) {
        if let AirNode::Manifest(manifest) = node {
            return Ok(manifest);
        }
        bail!("manifest bytes decoded to non-manifest AIR node");
    }
    if let Ok(manifest) = serde_cbor::from_slice::<Manifest>(bytes) {
        return Ok(manifest);
    }
    if let Ok(node) = serde_json::from_slice::<AirNode>(bytes) {
        if let AirNode::Manifest(manifest) = node {
            return Ok(manifest);
        }
        bail!("manifest bytes decoded to non-manifest AIR node");
    }
    if let Ok(manifest) = serde_json::from_slice::<Manifest>(bytes) {
        return Ok(manifest);
    }
    bail!("unsupported manifest encoding");
}

pub fn write_air_layout(
    bundle: &WorldBundle,
    manifest_cbor: &[u8],
    out_dir: &Path,
) -> Result<()> {
    write_air_layout_with_options(bundle, manifest_cbor, out_dir, WriteOptions::default())
}

pub fn write_air_layout_with_options(
    bundle: &WorldBundle,
    manifest_cbor: &[u8],
    out_dir: &Path,
    options: WriteOptions,
) -> Result<()> {
    let air_dir = out_dir.join("air");
    fs::create_dir_all(&air_dir).context("create air dir")?;
    fs::create_dir_all(out_dir.join(".aos")).context("create .aos dir")?;

    let (schemas, sys_schemas) = split_sys_defs(&bundle.schemas, options.include_sys);
    let (modules, sys_modules) = split_sys_defs(&bundle.modules, options.include_sys);
    let (plans, sys_plans) = split_sys_defs(&bundle.plans, options.include_sys);
    let (effects, sys_effects) = split_sys_defs(&bundle.effects, options.include_sys);
    let (caps, sys_caps) = split_sys_defs(&bundle.caps, options.include_sys);
    let (policies, sys_policies) = split_sys_defs(&bundle.policies, options.include_sys);
    let (secrets, sys_secrets) = split_sys_defs(&bundle.secrets, options.include_sys);

    write_json(
        &air_dir.join("manifest.air.json"),
        &AirNode::Manifest(bundle.manifest.clone()),
    )?;
    write_node_array(
        &air_dir.join("schemas.air.json"),
        schemas
            .iter()
            .cloned()
            .map(AirNode::Defschema)
            .collect(),
    )?;
    write_node_array(
        &air_dir.join("module.air.json"),
        modules
            .iter()
            .cloned()
            .map(AirNode::Defmodule)
            .collect(),
    )?;
    write_node_array(
        &air_dir.join("plans.air.json"),
        plans
            .iter()
            .cloned()
            .map(AirNode::Defplan)
            .collect(),
    )?;
    write_node_array(
        &air_dir.join("effects.air.json"),
        effects
            .iter()
            .cloned()
            .map(AirNode::Defeffect)
            .collect(),
    )?;
    write_node_array(
        &air_dir.join("capabilities.air.json"),
        caps.iter().cloned().map(AirNode::Defcap).collect(),
    )?;
    write_node_array(
        &air_dir.join("policies.air.json"),
        policies
            .iter()
            .cloned()
            .map(AirNode::Defpolicy)
            .collect(),
    )?;
    write_node_array(
        &air_dir.join("secrets.air.json"),
        secrets
            .iter()
            .cloned()
            .map(AirNode::Defsecret)
            .collect(),
    )?;

    if options.include_sys {
        let sys_nodes = collect_sys_nodes(
            sys_schemas,
            sys_modules,
            sys_plans,
            sys_effects,
            sys_caps,
            sys_policies,
            sys_secrets,
        );
        write_node_array(&air_dir.join("sys.air.json"), sys_nodes)?;
    }

    fs::write(out_dir.join(".aos/manifest.air.cbor"), manifest_cbor)
        .context("write manifest.air.cbor")?;
    Ok(())
}

impl WorldBundle {
    pub fn from_loaded(loaded: LoadedManifest) -> Self {
        let mut bundle = WorldBundle {
            manifest: loaded.manifest,
            schemas: loaded.schemas.into_values().collect(),
            modules: loaded.modules.into_values().collect(),
            plans: loaded.plans.into_values().collect(),
            caps: loaded.caps.into_values().collect(),
            policies: loaded.policies.into_values().collect(),
            effects: loaded.effects.into_values().collect(),
            secrets: Vec::new(),
        };
        bundle.sort_defs();
        bundle
    }

    pub fn from_loaded_assets(loaded: LoadedManifest, secrets: Vec<DefSecret>) -> Self {
        WorldBundle {
            manifest: loaded.manifest,
            schemas: loaded.schemas.into_values().collect(),
            modules: loaded.modules.into_values().collect(),
            plans: loaded.plans.into_values().collect(),
            caps: loaded.caps.into_values().collect(),
            policies: loaded.policies.into_values().collect(),
            effects: loaded.effects.into_values().collect(),
            secrets,
        }
        .sorted()
    }

    fn sort_defs(&mut self) {
        self.schemas.sort_by(|a, b| a.name.cmp(&b.name));
        self.modules.sort_by(|a, b| a.name.cmp(&b.name));
        self.plans.sort_by(|a, b| a.name.cmp(&b.name));
        self.caps.sort_by(|a, b| a.name.cmp(&b.name));
        self.policies.sort_by(|a, b| a.name.cmp(&b.name));
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

fn split_sys_defs<T: HasName + Clone>(
    defs: &[T],
    include_sys: bool,
) -> (Vec<T>, Vec<T>) {
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
    plans: Vec<DefPlan>,
    effects: Vec<DefEffect>,
    caps: Vec<DefCap>,
    policies: Vec<DefPolicy>,
    secrets: Vec<DefSecret>,
) -> Vec<AirNode> {
    let mut nodes = Vec::new();
    nodes.extend(schemas.into_iter().map(AirNode::Defschema));
    nodes.extend(modules.into_iter().map(AirNode::Defmodule));
    nodes.extend(plans.into_iter().map(AirNode::Defplan));
    nodes.extend(effects.into_iter().map(AirNode::Defeffect));
    nodes.extend(caps.into_iter().map(AirNode::Defcap));
    nodes.extend(policies.into_iter().map(AirNode::Defpolicy));
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

impl HasName for DefPlan {
    fn name(&self) -> &str {
        self.name.as_str()
    }
}

impl HasName for DefEffect {
    fn name(&self) -> &str {
        self.name.as_str()
    }
}

impl HasName for DefCap {
    fn name(&self) -> &str {
        self.name.as_str()
    }
}

impl HasName for DefPolicy {
    fn name(&self) -> &str {
        self.name.as_str()
    }
}

impl HasName for DefSecret {
    fn name(&self) -> &str {
        self.name.as_str()
    }
}

fn bundle_from_catalog(catalog: aos_store::Catalog, include_sys: bool) -> WorldBundle {
    let mut schemas = Vec::new();
    let mut modules = Vec::new();
    let mut plans = Vec::new();
    let mut caps = Vec::new();
    let mut policies = Vec::new();
    let mut effects = Vec::new();
    let mut secrets = Vec::new();

    for (name, entry) in catalog.nodes {
        if !include_sys && name.starts_with("sys/") {
            continue;
        }
        match entry.node {
            AirNode::Defschema(schema) => schemas.push(schema),
            AirNode::Defmodule(module) => modules.push(module),
            AirNode::Defplan(plan) => plans.push(plan),
            AirNode::Defcap(cap) => caps.push(cap),
            AirNode::Defpolicy(policy) => policies.push(policy),
            AirNode::Defeffect(effect) => effects.push(effect),
            AirNode::Defsecret(secret) => secrets.push(secret),
            AirNode::Manifest(_) => {}
        }
    }

    WorldBundle {
        manifest: catalog.manifest,
        schemas,
        modules,
        plans,
        caps,
        policies,
        effects,
        secrets,
    }
}

fn manifest_from_store(store: &FsStore, hash_hex: &str) -> Result<Manifest> {
    let hash = Hash::from_hex_str(hash_hex).context("parse base manifest hash")?;
    let node: AirNode = store
        .get_node(hash)
        .context("load base manifest from store")?;
    match node {
        AirNode::Manifest(manifest) => Ok(manifest),
        _ => bail!("base_manifest_hash does not point to a manifest"),
    }
}

fn manifest_from_path(path: &Path) -> Result<Manifest> {
    let bytes =
        fs::read(path).with_context(|| format!("read manifest at {}", path.display()))?;
    decode_manifest_bytes(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{CURRENT_AIR_VERSION, DefSchema, EmptyObject, HashRef, NamedRef, TypeExpr, TypePrimitive, TypePrimitiveBool};
    use aos_store::{MemStore, Store};

    #[test]
    fn manifest_node_hash_matches_store() {
        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: Vec::new(),
            modules: Vec::new(),
            plans: Vec::new(),
            effects: Vec::new(),
            caps: Vec::new(),
            policies: Vec::new(),
            secrets: Vec::new(),
            defaults: None,
            module_bindings: Default::default(),
            routing: None,
            triggers: Vec::new(),
        };
        let store = MemStore::new();
        let stored = store
            .put_node(&AirNode::Manifest(manifest.clone()))
            .expect("store manifest");
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
            plans: Vec::new(),
            effects: Vec::new(),
            caps: Vec::new(),
            policies: Vec::new(),
            secrets: Vec::new(),
            defaults: None,
            module_bindings: Default::default(),
            routing: None,
            triggers: Vec::new(),
        };
        let manifest_hash = store
            .put_node(&AirNode::Manifest(manifest.clone()))
            .expect("store manifest")
            .to_hex();

        let exported = export_bundle(&store, &manifest_hash, ExportOptions::default())
            .expect("export bundle");
        assert_eq!(exported.manifest_hash, manifest_hash);
        assert_eq!(exported.bundle.schemas.len(), 1);

        let store2 = MemStore::new();
        let imported = import_genesis(&store2, &exported.bundle).expect("import genesis");
        assert_eq!(imported.manifest_hash, manifest_hash);
        assert!(store2.has_node(schema_hash).expect("schema stored"));
        let manifest_node_hash =
            Hash::from_hex_str(&imported.manifest_hash).expect("manifest hash parse");
        assert!(
            store2.has_node(manifest_node_hash).expect("manifest stored"),
            "manifest node should be stored in CAS"
        );
    }
}
