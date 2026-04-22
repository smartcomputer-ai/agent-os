use std::collections::HashMap;

use aos_air_types::{
    AirNode, DefEffect, DefModule, DefSchema, DefSecret, DefWorkflow, HashRef, Manifest, Name,
    NamedRef, builtins, catalog::EffectCatalog, validate_manifest,
};
use aos_cbor::Hash;
use aos_cbor::to_canonical_cbor;

use crate::Store;
use crate::error::KernelError;
use crate::governance::ManifestPatch;
use crate::manifest_catalog::{Catalog, load_manifest_from_bytes, load_manifest_from_path};

#[derive(Debug, Clone)]
pub struct LoadedManifest {
    pub manifest: Manifest,
    pub secrets: Vec<DefSecret>,
    pub modules: HashMap<Name, DefModule>,
    pub workflows: HashMap<Name, DefWorkflow>,
    pub effects: HashMap<Name, DefEffect>,
    pub schemas: HashMap<Name, DefSchema>,
    pub effect_catalog: EffectCatalog,
}

pub struct ManifestLoader;

impl ManifestLoader {
    pub fn load_from_bytes<S: Store>(
        store: &S,
        bytes: &[u8],
    ) -> Result<LoadedManifest, KernelError> {
        let catalog = load_manifest_from_bytes(store, bytes)
            .map_err(|err| KernelError::Manifest(format!("load manifest: {err}")))?;
        Self::from_catalog(catalog)
    }

    pub fn load_from_manifest<S: Store>(
        store: &S,
        manifest: &Manifest,
    ) -> Result<LoadedManifest, KernelError> {
        let bytes = to_canonical_cbor(manifest)
            .map_err(|err| KernelError::Manifest(format!("encode manifest: {err}")))?;
        Self::load_from_bytes(store, &bytes)
    }

    pub fn load_from_path<S: Store>(
        store: &S,
        path: impl AsRef<std::path::Path>,
    ) -> Result<LoadedManifest, KernelError> {
        let catalog = load_manifest_from_path(store, path)?;
        Self::from_catalog(catalog)
    }

    pub fn load_from_hash<S: Store>(store: &S, hash: Hash) -> Result<LoadedManifest, KernelError> {
        let manifest: Manifest = store.get_node(hash)?;
        Self::load_from_manifest(store, &manifest)
    }

    fn from_catalog(catalog: Catalog) -> Result<LoadedManifest, KernelError> {
        let mut modules = HashMap::new();
        let mut workflows = HashMap::new();
        let mut effects = HashMap::new();
        let mut schemas = HashMap::new();
        let mut secrets = HashMap::new();
        for (name, entry) in catalog.nodes {
            match entry.node {
                AirNode::Defmodule(module) => {
                    modules.insert(name, module);
                }
                AirNode::Defworkflow(workflow) => {
                    workflows.insert(name, workflow);
                }
                AirNode::Defeffect(effect) => {
                    effects.insert(name, effect);
                }
                AirNode::Defschema(schema) => {
                    schemas.insert(name, schema);
                }
                AirNode::Defsecret(secret) => {
                    secrets.insert(name, secret);
                }
                _ => {}
            }
        }
        for builtin in builtins::builtin_schemas() {
            schemas
                .entry(builtin.schema.name.clone())
                .or_insert_with(|| builtin.schema.clone());
        }
        for builtin in builtins::builtin_modules() {
            modules
                .entry(builtin.module.name.clone())
                .or_insert_with(|| builtin.module.clone());
        }
        for builtin in builtins::builtin_workflows() {
            workflows
                .entry(builtin.workflow.name.clone())
                .or_insert_with(|| builtin.workflow.clone());
        }
        for builtin in builtins::builtin_effects() {
            effects
                .entry(builtin.effect.name.clone())
                .or_insert_with(|| builtin.effect.clone());
        }
        let manifest = catalog.manifest;
        validate_manifest(
            &manifest, &modules, &schemas, &workflows, &effects, &secrets,
        )
        .map_err(|e| KernelError::ManifestValidation(e.to_string()))?;
        let effect_catalog = EffectCatalog::from_effects(effects.values().cloned());
        Ok(LoadedManifest {
            manifest,
            secrets: catalog.resolved_secrets,
            modules,
            workflows,
            effects,
            schemas,
            effect_catalog,
        })
    }
}

pub fn manifest_patch_from_loaded(loaded: &LoadedManifest) -> ManifestPatch {
    let mut nodes: Vec<AirNode> = loaded
        .modules
        .values()
        .cloned()
        .map(AirNode::Defmodule)
        .collect();
    nodes.extend(loaded.schemas.values().cloned().map(AirNode::Defschema));
    nodes.extend(loaded.workflows.values().cloned().map(AirNode::Defworkflow));
    nodes.extend(loaded.effects.values().cloned().map(AirNode::Defeffect));
    nodes.extend(loaded.secrets.iter().cloned().map(AirNode::Defsecret));

    ManifestPatch {
        manifest: loaded.manifest.clone(),
        nodes,
    }
}

pub fn store_loaded_manifest<S: Store + ?Sized>(
    store: &S,
    loaded: &LoadedManifest,
) -> Result<Hash, KernelError> {
    if !loaded.manifest.secrets.is_empty() {
        return Err(KernelError::Manifest(
            "store_loaded_manifest does not yet support manifests with defsecret references".into(),
        ));
    }
    let hashes = write_nodes(
        store,
        true,
        loaded
            .schemas
            .values()
            .filter(|schema| !schema.name.starts_with("sys/"))
            .cloned()
            .collect(),
        loaded
            .modules
            .values()
            .filter(|module| !module.name.starts_with("sys/"))
            .cloned()
            .collect(),
        Vec::new(),
        loaded
            .workflows
            .values()
            .filter(|workflow| !workflow.name.starts_with("sys/"))
            .cloned()
            .collect(),
        loaded
            .effects
            .values()
            .filter(|effect| !effect.name.starts_with("sys/"))
            .cloned()
            .collect(),
    )?;
    let mut manifest = loaded.manifest.clone();
    patch_manifest_refs(&mut manifest, &hashes)?;
    Ok(store.put_node(&AirNode::Manifest(manifest))?)
}

fn write_nodes<S: Store + ?Sized>(
    store: &S,
    allow_reserved_sys: bool,
    schemas: Vec<DefSchema>,
    modules: Vec<DefModule>,
    secrets: Vec<DefSecret>,
    workflows: Vec<DefWorkflow>,
    effects: Vec<DefEffect>,
) -> Result<StoredHashes, KernelError> {
    let mut hashes = StoredHashes::default();
    for schema in schemas {
        let name = schema.name.clone();
        if !allow_reserved_sys {
            reject_sys_name("defschema", name.as_str())?;
        }
        let hash = store.put_node(&AirNode::Defschema(schema))?;
        insert_or_verify_hash("defschema", &mut hashes.schemas, name, hash)?;
    }
    for module in modules {
        let name = module.name.clone();
        if !allow_reserved_sys {
            reject_sys_name("defmodule", name.as_str())?;
        }
        let hash = store.put_node(&AirNode::Defmodule(module))?;
        insert_or_verify_hash("defmodule", &mut hashes.modules, name, hash)?;
    }
    for secret in secrets {
        let name = secret.name.clone();
        if !allow_reserved_sys {
            reject_sys_name("defsecret", name.as_str())?;
        }
        let hash = store.put_node(&AirNode::Defsecret(secret))?;
        insert_or_verify_hash("defsecret", &mut hashes.secrets, name, hash)?;
    }
    for workflow in workflows {
        let name = workflow.name.clone();
        if !allow_reserved_sys {
            reject_sys_name("defworkflow", name.as_str())?;
        }
        let hash = store.put_node(&AirNode::Defworkflow(workflow))?;
        insert_or_verify_hash("defworkflow", &mut hashes.workflows, name, hash)?;
    }
    for effect in effects {
        let name = effect.name.clone();
        if !allow_reserved_sys {
            reject_sys_name("defeffect", name.as_str())?;
        }
        let hash = store.put_node(&AirNode::Defeffect(effect))?;
        insert_or_verify_hash("defeffect", &mut hashes.effects, name, hash)?;
    }
    Ok(hashes)
}

fn insert_or_verify_hash(
    kind: &str,
    map: &mut HashMap<Name, HashRef>,
    name: Name,
    hash: Hash,
) -> Result<(), KernelError> {
    let hash_ref =
        HashRef::new(hash.to_hex()).map_err(|err| KernelError::Manifest(err.to_string()))?;
    if let Some(existing) = map.get(name.as_str()) {
        if existing != &hash_ref {
            return Err(KernelError::Manifest(format!(
                "duplicate {kind} '{}' has conflicting definitions ({}, {})",
                name,
                existing.as_str(),
                hash_ref.as_str()
            )));
        }
        return Ok(());
    }
    map.insert(name, hash_ref);
    Ok(())
}

fn reject_sys_name(kind: &str, name: &str) -> Result<(), KernelError> {
    if name.starts_with("sys/") {
        return Err(KernelError::Manifest(format!(
            "{kind} '{name}' is reserved; sys/* definitions must come from built-ins"
        )));
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

fn patch_manifest_refs(manifest: &mut Manifest, hashes: &StoredHashes) -> Result<(), KernelError> {
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
) -> Result<(), KernelError> {
    for reference in refs {
        let actual = if let Some(found) = hashes.get(reference.name.as_str()) {
            found.clone()
        } else if let Some(builtin) = builtins::find_builtin_schema(reference.name.as_str()) {
            builtin.hash_ref.clone()
        } else if kind == "workflow" {
            if let Some(builtin) = builtins::find_builtin_workflow(reference.name.as_str()) {
                builtin.hash_ref.clone()
            } else {
                return Err(KernelError::Manifest(format!(
                    "manifest references unknown {kind} '{}'",
                    reference.name
                )));
            }
        } else if kind == "effect" {
            if let Some(builtin) = builtins::find_builtin_effect(reference.name.as_str()) {
                builtin.hash_ref.clone()
            } else {
                return Err(KernelError::Manifest(format!(
                    "manifest references unknown {kind} '{}'",
                    reference.name
                )));
            }
        } else if kind == "module" {
            if let Some(builtin) = builtins::find_builtin_module(reference.name.as_str()) {
                builtin.hash_ref.clone()
            } else {
                return Err(KernelError::Manifest(format!(
                    "manifest references unknown {kind} '{}'",
                    reference.name
                )));
            }
        } else {
            return Err(KernelError::Manifest(format!(
                "manifest references unknown {kind} '{}'",
                reference.name
            )));
        };
        reference.hash = actual;
    }
    Ok(())
}
