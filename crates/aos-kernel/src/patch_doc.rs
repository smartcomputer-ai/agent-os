use std::collections::HashMap;

use aos_air_types::{AirNode, HashRef, Manifest, ManifestDefaults, NamedRef};
use aos_cbor::Hash;
use aos_store::Store;
use serde::Deserialize;

use crate::error::KernelError;
use crate::governance::ManifestPatch;
use crate::world::canonicalize_patch;

/// Patch document as described in spec/03-air.md §15 and patch.schema.json.
#[derive(Debug, Deserialize)]
pub struct PatchDocument {
    pub base_manifest_hash: String,
    pub patches: Vec<PatchOp>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum PatchOp {
    AddDef { add_def: AddDef },
    ReplaceDef { replace_def: ReplaceDef },
    RemoveDef { remove_def: RemoveDef },
    SetManifestRefs { set_manifest_refs: SetManifestRefs },
    SetDefaults { set_defaults: SetDefaults },
}

#[derive(Debug, Deserialize)]
pub struct AddDef {
    pub kind: String,
    pub node: AirNode,
}

#[derive(Debug, Deserialize)]
pub struct ReplaceDef {
    pub kind: String,
    pub name: String,
    pub new_node: AirNode,
    pub pre_hash: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoveDef {
    pub kind: String,
    pub name: String,
    pub pre_hash: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct SetManifestRefs {
    #[serde(default)]
    pub add: Vec<ManifestRef>,
    #[serde(default)]
    pub remove: Vec<ManifestRefRemove>,
}

#[derive(Debug, Deserialize)]
pub struct ManifestRef {
    pub kind: String,
    pub name: String,
    pub hash: String,
}

#[derive(Debug, Deserialize)]
pub struct ManifestRefRemove {
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct SetDefaults {
    pub policy: Option<String>,
    #[serde(default)]
    pub cap_grants: Vec<aos_air_types::CapGrant>,
}

/// Compile a patch document into a canonical ManifestPatch ready for submission.
/// - Loads the base manifest from the store.
/// - Applies patch ops (add/replace/remove/set_manifest_refs/set_defaults).
/// - Normalizes plans via `canonicalize_patch`.
/// - Computes hashes for new/updated defs and updates manifest references to match.
pub fn compile_patch_document<S: Store>(
    store: &S,
    doc: PatchDocument,
) -> Result<ManifestPatch, KernelError> {
    // Load base manifest
    let base_hash = Hash::from_hex_str(&doc.base_manifest_hash)
        .map_err(|e| KernelError::Manifest(format!("invalid base_manifest_hash: {e}")))?;
    let manifest_node: AirNode = store
        .get_node(base_hash)
        .map_err(|e| KernelError::Manifest(format!("load base manifest: {e}")))?;
    let mut manifest = match manifest_node {
        AirNode::Manifest(m) => m,
        _ => {
            return Err(KernelError::Manifest(
                "base_manifest_hash did not point to a manifest node".into(),
            ))
        }
    };

    let mut nodes: Vec<AirNode> = Vec::new();

    // Apply ops (structure only; hashes updated after canonicalization).
    for op in doc.patches {
        match op {
            PatchOp::AddDef { add_def } => {
                enforce_kind(&add_def.kind, &add_def.node)?;
                if let Some(name) = node_name(&add_def.node) {
                    insert_placeholder_ref(&mut manifest, &add_def.kind, name)?;
                }
                nodes.push(add_def.node);
            }
            PatchOp::ReplaceDef { replace_def } => {
                enforce_kind(&replace_def.kind, &replace_def.new_node)?;
                update_manifest_ref_hash(
                    &mut manifest,
                    &replace_def.kind,
                    &replace_def.name,
                    Some(&replace_def.pre_hash),
                    None,
                )?;
                nodes.push(replace_def.new_node);
            }
            PatchOp::RemoveDef { remove_def } => {
                update_manifest_ref_hash(
                    &mut manifest,
                    &remove_def.kind,
                    &remove_def.name,
                    Some(&remove_def.pre_hash),
                    Some(RemoveAction::Remove),
                )?;
            }
            PatchOp::SetManifestRefs { set_manifest_refs } => {
                apply_manifest_refs(&mut manifest, set_manifest_refs)?;
            }
            PatchOp::SetDefaults { set_defaults } => {
                apply_defaults(&mut manifest, set_defaults);
            }
        }
    }

    // Canonicalize (plan literal normalization, built-ins)
    let patch = ManifestPatch { manifest, nodes };
    let mut canonical = canonicalize_patch(store, patch)?;

    // Compute hashes for new nodes and rewrite manifest refs to match.
    let mut hash_map: HashMap<String, Hash> = HashMap::new();
    for node in &canonical.nodes {
        let h = Hash::of_cbor(node)
            .map_err(|e| KernelError::Manifest(format!("hash node: {e}")))?;
        if let Some(name) = node_name(node) {
            hash_map.insert(name.to_string(), h);
        }
    }
    rewrite_manifest_refs(&mut canonical.manifest, &hash_map);

    Ok(canonical)
}

fn enforce_kind(expected: &str, node: &AirNode) -> Result<(), KernelError> {
    let actual = match node {
        AirNode::Defmodule(_) => "defmodule",
        AirNode::Defplan(_) => "defplan",
        AirNode::Defschema(_) => "defschema",
        AirNode::Defcap(_) => "defcap",
        AirNode::Defpolicy(_) => "defpolicy",
        AirNode::Defeffect(_) => "defeffect",
        AirNode::Defsecret(_) => "defsecret",
        AirNode::Manifest(_) => "manifest",
    };
    if expected != actual {
        return Err(KernelError::Manifest(format!(
            "kind mismatch: patch declared {expected} but node is {actual}"
        )));
    }
    Ok(())
}

enum RemoveAction {
    Remove,
}

fn update_manifest_ref_hash(
    manifest: &mut Manifest,
    kind: &str,
    name: &str,
    pre_hash: Option<&str>,
    remove: Option<RemoveAction>,
) -> Result<(), KernelError> {
    let refs = refs_for_kind_mut(manifest, kind)?;
    if let Some(idx) = refs.iter().position(|r| r.name.as_str() == name) {
        if let Some(pre) = pre_hash {
            if refs[idx].hash.as_str() != pre {
                return Err(KernelError::Manifest(format!(
                    "pre_hash mismatch for {name}"
                )));
            }
        }
        if remove.is_some() {
            refs.remove(idx);
        }
    } else if remove.is_none() {
        // if replace referenced a non-existent ref, add placeholder so rewrite_manifest_refs updates it
        refs.push(NamedRef {
            name: name.into(),
            hash: zero_hash_ref()?,
        });
    }
    Ok(())
}

fn apply_manifest_refs(manifest: &mut Manifest, refs: SetManifestRefs) -> Result<(), KernelError> {
    for add in refs.add {
        let target = refs_for_kind_mut(manifest, &add.kind)?;
        if let Some(pos) = target.iter().position(|r| r.name.as_str() == add.name) {
            target[pos].hash = aos_air_types::HashRef::new(add.hash)
                .map_err(|e| KernelError::Manifest(e.to_string()))?;
        } else {
            target.push(NamedRef {
                name: add.name.clone(),
                hash: aos_air_types::HashRef::new(add.hash)
                    .map_err(|e| KernelError::Manifest(e.to_string()))?,
            });
        }
    }
    for rem in refs.remove {
        let target = refs_for_kind_mut(manifest, &rem.kind)?;
        target.retain(|r| r.name.as_str() != rem.name);
    }
    Ok(())
}

fn apply_defaults(manifest: &mut Manifest, defaults: SetDefaults) {
    let mut new_defaults = manifest.defaults.clone().unwrap_or(ManifestDefaults {
        policy: None,
        cap_grants: Vec::new(),
    });
    if let Some(policy) = defaults.policy {
        new_defaults.policy = Some(policy);
    }
    if !defaults.cap_grants.is_empty() {
        new_defaults.cap_grants = defaults.cap_grants;
    }
    manifest.defaults = Some(new_defaults);
}

fn rewrite_manifest_refs(manifest: &mut Manifest, hash_map: &HashMap<String, Hash>) {
    for refs in [
        &mut manifest.schemas,
        &mut manifest.modules,
        &mut manifest.plans,
        &mut manifest.effects,
        &mut manifest.caps,
        &mut manifest.policies,
    ] {
        for nr in refs.iter_mut() {
            if let Some(h) = hash_map.get(nr.name.as_str()) {
                if let Ok(hr) = aos_air_types::HashRef::new(h.to_hex()) {
                    nr.hash = hr;
                }
            }
        }
    }
    for entry in manifest.secrets.iter_mut() {
        if let aos_air_types::SecretEntry::Ref(nr) = entry {
            if let Some(h) = hash_map.get(nr.name.as_str()) {
                if let Ok(hr) = aos_air_types::HashRef::new(h.to_hex()) {
                    nr.hash = hr;
                }
            }
        }
    }
}

fn refs_for_kind_mut<'a>(
    manifest: &'a mut Manifest,
    kind: &str,
) -> Result<&'a mut Vec<NamedRef>, KernelError> {
    match kind {
        "defschema" => Ok(&mut manifest.schemas),
        "defmodule" => Ok(&mut manifest.modules),
        "defplan" => Ok(&mut manifest.plans),
        "defeffect" => Ok(&mut manifest.effects),
        "defcap" => Ok(&mut manifest.caps),
        "defpolicy" => Ok(&mut manifest.policies),
        "defsecret" => {
            // Only secret refs (not decls) can be addressed here.
            Err(KernelError::Manifest(
                "set_manifest_refs for defsecret not supported yet".into(),
            ))
        }
        other => Err(KernelError::Manifest(format!(
            "unknown kind in patch: {other}"
        ))),
    }
}

fn insert_placeholder_ref(manifest: &mut Manifest, kind: &str, name: &str) -> Result<(), KernelError> {
    let refs = refs_for_kind_mut(manifest, kind)?;
    if refs.iter().any(|r| r.name.as_str() == name) {
        return Ok(());
    }
    refs.push(NamedRef {
        name: name.to_string(),
        hash: zero_hash_ref()?,
    });
    Ok(())
}

fn node_name(node: &AirNode) -> Option<&str> {
    match node {
        AirNode::Defmodule(m) => Some(m.name.as_str()),
        AirNode::Defplan(p) => Some(p.name.as_str()),
        AirNode::Defschema(s) => Some(s.name.as_str()),
        AirNode::Defcap(c) => Some(c.name.as_str()),
        AirNode::Defpolicy(p) => Some(p.name.as_str()),
        AirNode::Defeffect(e) => Some(e.name.as_str()),
        AirNode::Defsecret(s) => Some(s.name.as_str()),
        AirNode::Manifest(_) => None,
    }
}

fn zero_hash_ref() -> Result<HashRef, KernelError> {
    HashRef::new("sha256:0000000000000000000000000000000000000000000000000000000000000000")
        .map_err(|e| KernelError::Manifest(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{
        CapGrant, DefSchema, EmptyObject, HashRef, TypeExpr, TypePrimitive, TypePrimitiveBool,
        ValueLiteral, ValueNull,
    };
    use aos_store::MemStore;

    fn empty_manifest() -> Manifest {
        Manifest {
            air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
            schemas: vec![],
            modules: vec![],
            plans: vec![],
            effects: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing: None,
            triggers: vec![],
        }
    }

    fn store_manifest(store: &MemStore, manifest: Manifest) -> String {
        let hash = store.put_node(&AirNode::Manifest(manifest)).expect("store manifest");
        hash.to_hex()
    }

    fn defschema(name: &str) -> AirNode {
        AirNode::Defschema(DefSchema {
            name: name.to_string(),
            ty: TypeExpr::Primitive(TypePrimitive::Bool(TypePrimitiveBool { bool: EmptyObject {} })),
        })
    }

    #[test]
    fn add_def_updates_manifest_refs() {
        let store = MemStore::new();
        let base_hash = store_manifest(&store, empty_manifest());
        let doc = PatchDocument {
            base_manifest_hash: base_hash.clone(),
            patches: vec![PatchOp::AddDef {
                add_def: AddDef {
                    kind: "defschema".into(),
                    node: defschema("demo/Foo@1"),
                },
            }],
        };
        let patch = compile_patch_document(&store, doc).expect("compile");
        assert!(patch
            .manifest
            .schemas
            .iter()
            .any(|nr| nr.name.as_str() == "demo/Foo@1"));
        assert!(patch
            .nodes
            .iter()
            .any(|n| matches!(n, AirNode::Defschema(s) if s.name == "demo/Foo@1")));
    }

    #[test]
    fn remove_def_respects_pre_hash() {
        let store = MemStore::new();
        // store a schema node
        let schema_node = defschema("demo/ToRemove@1");
        let schema_hash = store.put_node(&schema_node).unwrap().to_hex();
        let mut manifest = empty_manifest();
        manifest.schemas.push(NamedRef {
            name: "demo/ToRemove@1".into(),
            hash: HashRef::new(schema_hash.clone()).unwrap(),
        });
        let base_hash = store_manifest(&store, manifest);
        let doc = PatchDocument {
            base_manifest_hash: base_hash.clone(),
            patches: vec![PatchOp::RemoveDef {
                remove_def: RemoveDef {
                    kind: "defschema".into(),
                    name: "demo/ToRemove@1".into(),
                    pre_hash: schema_hash,
                },
            }],
        };
        let patch = compile_patch_document(&store, doc).expect("compile");
        assert!(!patch
            .manifest
            .schemas
            .iter()
            .any(|nr| nr.name.as_str() == "demo/ToRemove@1"));
    }

    #[test]
    fn replace_def_pre_hash_mismatch_errors() {
        let store = MemStore::new();
        let schema_node = defschema("demo/Old@1");
        let schema_hash = store.put_node(&schema_node).unwrap().to_hex();
        let mut manifest = empty_manifest();
        manifest.schemas.push(NamedRef {
            name: "demo/Old@1".into(),
            hash: HashRef::new(schema_hash.clone()).unwrap(),
        });
        let base_hash = store_manifest(&store, manifest);
        let doc = PatchDocument {
            base_manifest_hash: base_hash,
            patches: vec![PatchOp::ReplaceDef {
                replace_def: ReplaceDef {
                    kind: "defschema".into(),
                    name: "demo/Old@1".into(),
                    new_node: defschema("demo/Old@1"),
                    pre_hash: "sha256:deadbeef".into(),
                },
            }],
        };
        let err = compile_patch_document(&store, doc).unwrap_err();
        assert!(format!("{err}").contains("pre_hash mismatch"));
    }

    #[test]
    fn set_defaults_sets_policy_and_cap_grants() {
        let store = MemStore::new();
        let base_hash = store_manifest(&store, empty_manifest());
        let doc = PatchDocument {
            base_manifest_hash: base_hash,
            patches: vec![PatchOp::SetDefaults {
                set_defaults: SetDefaults {
                    policy: Some("demo/policy@1".into()),
                    cap_grants: vec![CapGrant {
                        name: "g1".into(),
                        cap: "cap/demo@1".into(),
                        params: ValueLiteral::Null(ValueNull { null: EmptyObject {} }),
                        expiry_ns: None,
                        budget: None,
                    }],
                },
            }],
        };
        let patch = compile_patch_document(&store, doc).expect("compile");
        let defaults = patch.manifest.defaults.expect("defaults");
        assert_eq!(defaults.policy.as_deref(), Some("demo/policy@1"));
        assert_eq!(defaults.cap_grants.len(), 1);
    }
}
