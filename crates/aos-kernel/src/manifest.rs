use std::collections::HashMap;

use aos_air_types::{
    self as air_types, AirNode, DefCap, DefEffect, DefModule, DefPolicy, DefSchema, HashRef,
    Manifest, Name, NamedRef, SecretDecl, SecretEntry, catalog::EffectCatalog, validate_manifest,
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
    pub secrets: Vec<SecretDecl>,
    pub modules: HashMap<Name, DefModule>,
    pub effects: HashMap<Name, DefEffect>,
    pub caps: HashMap<Name, DefCap>,
    pub policies: HashMap<Name, DefPolicy>,
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
        let mut effects = HashMap::new();
        let mut caps = HashMap::new();
        let mut policies = HashMap::new();
        let mut schemas = HashMap::new();
        for (name, entry) in catalog.nodes {
            match entry.node {
                AirNode::Defmodule(module) => {
                    modules.insert(name, module);
                }
                AirNode::Defcap(cap) => {
                    caps.insert(name, cap);
                }
                AirNode::Defpolicy(policy) => {
                    policies.insert(name, policy);
                }
                AirNode::Defeffect(effect) => {
                    effects.insert(name, effect);
                }
                AirNode::Defschema(schema) => {
                    schemas.insert(name, schema);
                }
                _ => {}
            }
        }
        let manifest = catalog.manifest;
        validate_manifest(&manifest, &modules, &schemas, &effects, &caps, &policies)
            .map_err(|e| KernelError::ManifestValidation(e.to_string()))?;
        let effect_catalog = EffectCatalog::from_defs(effects.values().cloned());
        Ok(LoadedManifest {
            manifest,
            secrets: catalog.resolved_secrets,
            modules,
            effects,
            caps,
            policies,
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
    nodes.extend(loaded.caps.values().cloned().map(AirNode::Defcap));
    nodes.extend(loaded.policies.values().cloned().map(AirNode::Defpolicy));
    nodes.extend(loaded.effects.values().cloned().map(AirNode::Defeffect));

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
        loaded.modules.values().cloned().collect(),
        loaded
            .caps
            .values()
            .filter(|cap| !cap.name.starts_with("sys/"))
            .cloned()
            .collect(),
        loaded.policies.values().cloned().collect(),
        Vec::new(),
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
    caps: Vec<DefCap>,
    policies: Vec<DefPolicy>,
    secrets: Vec<aos_air_types::DefSecret>,
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
    for cap in caps {
        let name = cap.name.clone();
        if !allow_reserved_sys {
            reject_sys_name("defcap", name.as_str())?;
        }
        let hash = store.put_node(&AirNode::Defcap(cap))?;
        insert_or_verify_hash("defcap", &mut hashes.caps, name, hash)?;
    }
    for policy in policies {
        let name = policy.name.clone();
        if !allow_reserved_sys {
            reject_sys_name("defpolicy", name.as_str())?;
        }
        let hash = store.put_node(&AirNode::Defpolicy(policy))?;
        insert_or_verify_hash("defpolicy", &mut hashes.policies, name, hash)?;
    }
    for secret in secrets {
        let name = secret.name.clone();
        if !allow_reserved_sys {
            reject_sys_name("defsecret", name.as_str())?;
        }
        let hash = store.put_node(&AirNode::Defsecret(secret))?;
        insert_or_verify_hash("defsecret", &mut hashes.secrets, name, hash)?;
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
    effects: HashMap<Name, HashRef>,
    caps: HashMap<Name, HashRef>,
    policies: HashMap<Name, HashRef>,
    secrets: HashMap<Name, HashRef>,
}

fn patch_manifest_refs(manifest: &mut Manifest, hashes: &StoredHashes) -> Result<(), KernelError> {
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

fn secrets_as_named_refs(entries: &[SecretEntry]) -> Result<Vec<NamedRef>, KernelError> {
    let mut refs = Vec::new();
    for entry in entries {
        match entry {
            SecretEntry::Ref(r) => refs.push(r.clone()),
            SecretEntry::Decl(_) => {
                return Err(KernelError::Manifest(
                    "inline secret declarations are unsupported; provide defsecret nodes instead"
                        .into(),
                ));
            }
        }
    }
    Ok(refs)
}

fn patch_named_refs(
    kind: &str,
    refs: &mut [NamedRef],
    hashes: &HashMap<Name, HashRef>,
) -> Result<(), KernelError> {
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
                return Err(KernelError::Manifest(format!(
                    "manifest references unknown {kind} '{}'",
                    reference.name
                )));
            }
        } else if kind == "module" {
            if let Some(builtin) = air_types::builtins::find_builtin_module(reference.name.as_str())
            {
                builtin.hash_ref.clone()
            } else {
                return Err(KernelError::Manifest(format!(
                    "manifest references unknown {kind} '{}'",
                    reference.name
                )));
            }
        } else if kind == "cap" {
            if let Some(builtin) = air_types::builtins::find_builtin_cap(reference.name.as_str()) {
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
