use std::collections::HashMap;

use crate::Store;
use aos_air_types::{AirNode, HashRef, Manifest, NamedRef};
use aos_cbor::Hash;
use serde::{Deserialize, Serialize};

use crate::error::KernelError;
use crate::governance::ManifestPatch;
use crate::governance_utils::canonicalize_patch;

#[derive(Debug, Serialize, Deserialize)]
pub struct PatchDocument {
    #[serde(default = "default_patch_version")]
    pub version: String,
    pub base_manifest_hash: String,
    pub patches: Vec<PatchOp>,
}

fn default_patch_version() -> String {
    "2".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PatchOp {
    AddDef {
        add_def: AddDef,
    },
    ReplaceDef {
        replace_def: ReplaceDef,
    },
    RemoveDef {
        remove_def: RemoveDef,
    },
    SetManifestRefs {
        set_manifest_refs: SetManifestRefs,
    },
    SetRoutingSubscriptions {
        set_routing_subscriptions: SetRoutingSubscriptions,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddDef {
    pub kind: String,
    pub node: AirNode,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceDef {
    pub kind: String,
    pub name: String,
    pub new_node: AirNode,
    pub pre_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoveDef {
    pub kind: String,
    pub name: String,
    pub pre_hash: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SetManifestRefs {
    #[serde(default)]
    pub add: Vec<ManifestRef>,
    #[serde(default)]
    pub remove: Vec<ManifestRefRemove>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ManifestRef {
    pub kind: String,
    pub name: String,
    pub hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ManifestRefRemove {
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SetRoutingSubscriptions {
    pub pre_hash: String,
    pub subscriptions: Vec<aos_air_types::RoutingEvent>,
}

pub fn compile_patch_document<S: Store>(
    store: &S,
    doc: PatchDocument,
) -> Result<ManifestPatch, KernelError> {
    if doc.version != "2" {
        return Err(KernelError::Manifest(format!(
            "unsupported patch document version: {}",
            doc.version
        )));
    }

    let base_hash = Hash::from_hex_str(&doc.base_manifest_hash)
        .map_err(|e| KernelError::Manifest(format!("invalid base_manifest_hash: {e}")))?;
    let mut manifest: Manifest = store
        .get_node(base_hash)
        .map_err(|e| KernelError::Manifest(format!("load base manifest: {e}")))?;

    let mut nodes: Vec<AirNode> = Vec::new();

    for op in doc.patches {
        match op {
            PatchOp::AddDef { add_def } => {
                enforce_kind(&add_def.kind, &add_def.node)?;
                if let Some(name) = node_name(&add_def.node) {
                    reject_sys_name(name, "add")?;
                    insert_placeholder_ref(&mut manifest, &add_def.kind, name)?;
                }
                nodes.push(add_def.node);
            }
            PatchOp::ReplaceDef { replace_def } => {
                enforce_kind(&replace_def.kind, &replace_def.new_node)?;
                reject_sys_name(&replace_def.name, "replace")?;
                if let Some(name) = node_name(&replace_def.new_node) {
                    reject_sys_name(name, "replace")?;
                }
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
                reject_sys_name(&remove_def.name, "remove")?;
                update_manifest_ref_hash(
                    &mut manifest,
                    &remove_def.kind,
                    &remove_def.name,
                    Some(&remove_def.pre_hash),
                    Some(RemoveAction::Remove),
                )?;
            }
            PatchOp::SetManifestRefs { set_manifest_refs } => {
                for reference in &set_manifest_refs.add {
                    reject_sys_name(&reference.name, "add manifest ref for")?;
                }
                for reference in &set_manifest_refs.remove {
                    reject_sys_name(&reference.name, "remove manifest ref for")?;
                }
                apply_manifest_refs(&mut manifest, set_manifest_refs)?;
            }
            PatchOp::SetRoutingSubscriptions {
                set_routing_subscriptions,
            } => {
                apply_routing_subscriptions(&mut manifest, set_routing_subscriptions)?;
            }
        }
    }

    let patch = ManifestPatch { manifest, nodes };
    let mut canonical = canonicalize_patch(store, patch)?;

    let mut hash_map: HashMap<String, Hash> = HashMap::new();
    for node in &canonical.nodes {
        let h =
            Hash::of_cbor(node).map_err(|e| KernelError::Manifest(format!("hash node: {e}")))?;
        if let Some(name) = node_name(node) {
            hash_map.insert(name.to_string(), h);
        }
    }
    rewrite_manifest_refs(&mut canonical.manifest, &hash_map);

    for node in &canonical.nodes {
        store
            .put_node(node)
            .map_err(|e| KernelError::Manifest(e.to_string()))?;
    }

    Ok(canonical)
}

fn enforce_kind(expected: &str, node: &AirNode) -> Result<(), KernelError> {
    let actual = match node {
        AirNode::Defmodule(_) => "defmodule",
        AirNode::Defschema(_) => "defschema",
        AirNode::Defop(_) => "defop",
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
        if let Some(pre) = pre_hash
            && refs[idx].hash.as_str() != pre
        {
            return Err(KernelError::Manifest(format!(
                "pre_hash mismatch for {name}"
            )));
        }
        if remove.is_some() {
            refs.remove(idx);
        }
    } else if remove.is_none() {
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
        let hash = HashRef::new(add.hash).map_err(|e| KernelError::Manifest(e.to_string()))?;
        if let Some(pos) = target.iter().position(|r| r.name.as_str() == add.name) {
            target[pos].hash = hash;
        } else {
            target.push(NamedRef {
                name: add.name,
                hash,
            });
        }
    }

    for rem in refs.remove {
        let target = refs_for_kind_mut(manifest, &rem.kind)?;
        if let Some(pos) = target.iter().position(|r| r.name.as_str() == rem.name) {
            target.remove(pos);
        }
    }

    Ok(())
}

fn apply_routing_subscriptions(
    manifest: &mut Manifest,
    op: SetRoutingSubscriptions,
) -> Result<(), KernelError> {
    let current = manifest
        .routing
        .as_ref()
        .map(|r| r.subscriptions.clone())
        .unwrap_or_default();
    verify_block_pre_hash(&current, &op.pre_hash, "routing.subscriptions")?;
    let routing = manifest
        .routing
        .get_or_insert_with(|| aos_air_types::Routing {
            subscriptions: vec![],
        });
    routing.subscriptions = op.subscriptions;
    Ok(())
}

fn verify_block_pre_hash<T: serde::Serialize>(
    current: &T,
    pre_hash_hex: &str,
    label: &str,
) -> Result<(), KernelError> {
    let expected = Hash::of_cbor(current)
        .map_err(|err| KernelError::Manifest(format!("hash {label}: {err}")))?;
    let found = Hash::from_hex_str(pre_hash_hex)
        .map_err(|err| KernelError::Manifest(format!("invalid pre_hash for {label}: {err}")))?;
    if expected != found {
        return Err(KernelError::Manifest(format!(
            "pre_hash mismatch for {label}"
        )));
    }
    Ok(())
}

fn refs_for_kind_mut<'a>(
    manifest: &'a mut Manifest,
    kind: &str,
) -> Result<&'a mut Vec<NamedRef>, KernelError> {
    match kind {
        "defschema" => Ok(&mut manifest.schemas),
        "defmodule" => Ok(&mut manifest.modules),
        "defop" => Ok(&mut manifest.ops),
        "defsecret" => Ok(&mut manifest.secrets),
        _ => Err(KernelError::Manifest(format!(
            "unsupported manifest ref kind: {kind}"
        ))),
    }
}

fn rewrite_manifest_refs(manifest: &mut Manifest, hash_map: &HashMap<String, Hash>) {
    let rewrite = |refs: &mut Vec<NamedRef>| {
        for nr in refs.iter_mut() {
            if let Some(h) = hash_map.get(&nr.name)
                && let Ok(hash_ref) = HashRef::new(h.to_hex())
            {
                nr.hash = hash_ref;
            }
        }
    };

    rewrite(&mut manifest.schemas);
    rewrite(&mut manifest.modules);
    rewrite(&mut manifest.ops);
    rewrite(&mut manifest.secrets);
}

fn node_name(node: &AirNode) -> Option<&str> {
    match node {
        AirNode::Defschema(s) => Some(s.name.as_str()),
        AirNode::Defmodule(m) => Some(m.name.as_str()),
        AirNode::Defop(o) => Some(o.name.as_str()),
        AirNode::Defsecret(s) => Some(s.name.as_str()),
        AirNode::Manifest(_) => None,
    }
}

fn zero_hash_ref() -> Result<HashRef, KernelError> {
    HashRef::new(format!("sha256:{}", "0".repeat(64)))
        .map_err(|e| KernelError::Manifest(e.to_string()))
}

fn insert_placeholder_ref(
    manifest: &mut Manifest,
    kind: &str,
    name: &str,
) -> Result<(), KernelError> {
    let target = refs_for_kind_mut(manifest, kind)?;
    if !target.iter().any(|r| r.name.as_str() == name) {
        target.push(NamedRef {
            name: name.to_string(),
            hash: zero_hash_ref()?,
        });
    }
    Ok(())
}

fn reject_sys_name(name: &str, action: &str) -> Result<(), KernelError> {
    if name.starts_with("sys/") {
        Err(KernelError::Manifest(format!(
            "cannot {action} reserved sys/* def '{name}'"
        )))
    } else {
        Ok(())
    }
}
