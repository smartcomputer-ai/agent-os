use std::collections::HashMap;

use aos_air_types::{AirNode, HashRef, NamedRef, TypeExpr, builtins, plan_literals::SchemaIndex};
use aos_cbor::Hash;
use aos_store::Store;

use crate::error::KernelError;
use crate::governance::ManifestPatch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NamedRefDiffKind {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NamedRefDiff {
    pub name: String,
    pub change: NamedRefDiffKind,
}

pub(crate) fn diff_named_refs(current: &[NamedRef], candidate: &[NamedRef]) -> Vec<NamedRefDiff> {
    let mut deltas = Vec::new();
    let current_map: HashMap<&str, &NamedRef> = current
        .iter()
        .map(|reference| (reference.name.as_str(), reference))
        .collect();
    let next_map: HashMap<&str, &NamedRef> = candidate
        .iter()
        .map(|reference| (reference.name.as_str(), reference))
        .collect();

    for (name, reference) in &next_map {
        match current_map.get(name) {
            None => deltas.push(NamedRefDiff {
                name: reference.name.as_str().to_string(),
                change: NamedRefDiffKind::Added,
            }),
            Some(current_ref) if current_ref.hash.as_str() != reference.hash.as_str() => {
                deltas.push(NamedRefDiff {
                    name: reference.name.as_str().to_string(),
                    change: NamedRefDiffKind::Changed,
                });
            }
            _ => {}
        }
    }

    for (name, reference) in &current_map {
        if !next_map.contains_key(name) {
            deltas.push(NamedRefDiff {
                name: reference.name.as_str().to_string(),
                change: NamedRefDiffKind::Removed,
            });
        }
    }

    deltas
}

pub fn canonicalize_patch<S: Store>(
    store: &S,
    patch: ManifestPatch,
) -> Result<ManifestPatch, KernelError> {
    let mut canonical = patch.clone();

    // Keep schema index warm for callers that rely on this helper's schema loading side effects.
    let mut schema_map = HashMap::new();
    for builtin in builtins::builtin_schemas() {
        schema_map.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
    }
    for node in &canonical.nodes {
        if let AirNode::Defschema(schema) = node {
            schema_map.insert(schema.name.clone(), schema.ty.clone());
        }
    }
    extend_schema_map_from_store(store, &canonical.manifest.schemas, &mut schema_map)?;
    let _schema_index = SchemaIndex::new(schema_map);

    normalize_patch_manifest_refs(&mut canonical)?;
    Ok(canonical)
}

fn normalize_patch_manifest_refs(patch: &mut ManifestPatch) -> Result<(), KernelError> {
    let mut schema_hashes = HashMap::new();
    let mut module_hashes = HashMap::new();
    let mut effect_hashes = HashMap::new();
    let mut cap_hashes = HashMap::new();
    let mut policy_hashes = HashMap::new();

    for node in &patch.nodes {
        match node {
            AirNode::Defschema(schema) => {
                let hash = Hash::of_cbor(&AirNode::Defschema(schema.clone())).map_err(|err| {
                    KernelError::Manifest(format!("hash schema '{}': {err}", schema.name))
                })?;
                schema_hashes.insert(schema.name.clone(), hash);
            }
            AirNode::Defmodule(module) => {
                let hash = Hash::of_cbor(&AirNode::Defmodule(module.clone())).map_err(|err| {
                    KernelError::Manifest(format!("hash module '{}': {err}", module.name))
                })?;
                module_hashes.insert(module.name.clone(), hash);
            }
            AirNode::Defeffect(effect) => {
                let hash = Hash::of_cbor(&AirNode::Defeffect(effect.clone())).map_err(|err| {
                    KernelError::Manifest(format!("hash effect '{}': {err}", effect.name))
                })?;
                effect_hashes.insert(effect.name.clone(), hash);
            }
            AirNode::Defcap(cap) => {
                let hash = Hash::of_cbor(&AirNode::Defcap(cap.clone())).map_err(|err| {
                    KernelError::Manifest(format!("hash cap '{}': {err}", cap.name))
                })?;
                cap_hashes.insert(cap.name.clone(), hash);
            }
            AirNode::Defpolicy(policy) => {
                let hash = Hash::of_cbor(&AirNode::Defpolicy(policy.clone())).map_err(|err| {
                    KernelError::Manifest(format!("hash policy '{}': {err}", policy.name))
                })?;
                policy_hashes.insert(policy.name.clone(), hash);
            }
            _ => {}
        }
    }

    for reference in &mut patch.manifest.schemas {
        if let Some(builtin) = builtins::find_builtin_schema(reference.name.as_str()) {
            reference.hash = builtin.hash_ref.clone();
        } else if let Some(hash) = schema_hashes.get(&reference.name) {
            reference.hash = HashRef::new(hash.to_hex()).map_err(|err| {
                KernelError::Manifest(format!("schema hash '{}': {err}", reference.name))
            })?;
        }
    }

    for reference in &mut patch.manifest.modules {
        if let Some(builtin) = builtins::find_builtin_module(reference.name.as_str()) {
            reference.hash = builtin.hash_ref.clone();
        } else if let Some(hash) = module_hashes.get(&reference.name) {
            reference.hash = HashRef::new(hash.to_hex()).map_err(|err| {
                KernelError::Manifest(format!("module hash '{}': {err}", reference.name))
            })?;
        }
    }

    for reference in &mut patch.manifest.effects {
        if let Some(builtin) = builtins::find_builtin_effect(reference.name.as_str()) {
            reference.hash = builtin.hash_ref.clone();
        } else if let Some(hash) = effect_hashes.get(&reference.name) {
            reference.hash = HashRef::new(hash.to_hex()).map_err(|err| {
                KernelError::Manifest(format!("effect hash '{}': {err}", reference.name))
            })?;
        }
    }

    for reference in &mut patch.manifest.caps {
        if let Some(builtin) = builtins::find_builtin_cap(reference.name.as_str()) {
            reference.hash = builtin.hash_ref.clone();
        } else if let Some(hash) = cap_hashes.get(&reference.name) {
            reference.hash = HashRef::new(hash.to_hex()).map_err(|err| {
                KernelError::Manifest(format!("cap hash '{}': {err}", reference.name))
            })?;
        }
    }

    for reference in &mut patch.manifest.policies {
        if let Some(hash) = policy_hashes.get(&reference.name) {
            reference.hash = HashRef::new(hash.to_hex()).map_err(|err| {
                KernelError::Manifest(format!("policy hash '{}': {err}", reference.name))
            })?;
        }
    }

    Ok(())
}

fn extend_schema_map_from_store<S: Store>(
    store: &S,
    refs: &[NamedRef],
    schemas: &mut HashMap<String, TypeExpr>,
) -> Result<(), KernelError> {
    for reference in refs {
        if schemas.contains_key(reference.name.as_str()) {
            continue;
        }
        if let Some(hash) = parse_nonzero_hash(reference.hash.as_str())? {
            let node: AirNode = store.get_node(hash)?;
            if let AirNode::Defschema(schema) = node {
                schemas.insert(schema.name.clone(), schema.ty.clone());
            }
        }
    }
    Ok(())
}

fn parse_nonzero_hash(value: &str) -> Result<Option<Hash>, KernelError> {
    let hash = Hash::from_hex_str(value)
        .map_err(|err| KernelError::Manifest(format!("invalid hash '{value}': {err}")))?;
    if hash.as_bytes().iter().all(|b| *b == 0) {
        Ok(None)
    } else {
        Ok(Some(hash))
    }
}
