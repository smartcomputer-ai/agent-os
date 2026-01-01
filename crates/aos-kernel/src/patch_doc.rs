use indexmap::IndexMap;
use std::collections::HashMap;

use aos_air_types::{AirNode, HashRef, Manifest, ManifestDefaults, NamedRef};
use aos_cbor::Hash;
use aos_store::Store;
use serde::{Deserialize, Serialize};

use crate::error::KernelError;
use crate::governance::ManifestPatch;
use crate::world::canonicalize_patch;

/// Patch document as described in spec/03-air.md §15 and patch.schema.json.
#[derive(Debug, Serialize, Deserialize)]
pub struct PatchDocument {
    #[serde(default = "default_patch_version")]
    pub version: String,
    pub base_manifest_hash: String,
    pub patches: Vec<PatchOp>,
}

fn default_patch_version() -> String {
    "1".to_string()
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
    SetDefaults {
        set_defaults: SetDefaults,
    },
    SetRoutingEvents {
        set_routing_events: SetRoutingEvents,
    },
    SetRoutingInboxes {
        set_routing_inboxes: SetRoutingInboxes,
    },
    SetTriggers {
        set_triggers: SetTriggers,
    },
    SetModuleBindings {
        set_module_bindings: SetModuleBindings,
    },
    SetSecrets {
        set_secrets: SetSecrets,
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

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SetDefaults {
    /// None => omit; Some(None) => clear; Some(Some(name)) => set
    pub policy: Option<Option<String>>,
    /// None => omit; Some(vec![]) => clear; Some(non-empty) => replace
    pub cap_grants: Option<Vec<aos_air_types::CapGrant>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SetRoutingEvents {
    pub pre_hash: String,
    pub events: Vec<aos_air_types::RoutingEvent>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SetRoutingInboxes {
    pub pre_hash: String,
    pub inboxes: Vec<aos_air_types::InboxRoute>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SetTriggers {
    pub pre_hash: String,
    pub triggers: Vec<aos_air_types::Trigger>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SetModuleBindings {
    pub pre_hash: String,
    pub bindings: indexmap::IndexMap<String, aos_air_types::ModuleBinding>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SetSecrets {
    pub pre_hash: String,
    pub secrets: Vec<aos_air_types::SecretEntry>,
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
    if doc.version != "1" {
        return Err(KernelError::Manifest(format!(
            "unsupported patch document version: {}",
            doc.version
        )));
    }
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
            ));
        }
    };

    let mut nodes: Vec<AirNode> = Vec::new();

    // Apply ops (structure only; hashes updated after canonicalization).
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
            PatchOp::SetDefaults { set_defaults } => {
                apply_defaults(&mut manifest, set_defaults);
            }
            PatchOp::SetRoutingEvents { set_routing_events } => {
                apply_routing_events(&mut manifest, set_routing_events)?;
            }
            PatchOp::SetRoutingInboxes {
                set_routing_inboxes,
            } => {
                apply_routing_inboxes(&mut manifest, set_routing_inboxes)?;
            }
            PatchOp::SetTriggers { set_triggers } => {
                apply_triggers(&mut manifest, set_triggers)?;
            }
            PatchOp::SetModuleBindings {
                set_module_bindings,
            } => {
                apply_module_bindings(&mut manifest, set_module_bindings)?;
            }
            PatchOp::SetSecrets { set_secrets } => {
                apply_secrets(&mut manifest, set_secrets)?;
            }
        }
    }

    // Canonicalize (plan literal normalization, built-ins)
    let patch = ManifestPatch { manifest, nodes };
    let mut canonical = canonicalize_patch(store, patch)?;

    // Compute hashes for new nodes and rewrite manifest refs to match.
    let mut hash_map: HashMap<String, Hash> = HashMap::new();
    for node in &canonical.nodes {
        let h =
            Hash::of_cbor(node).map_err(|e| KernelError::Manifest(format!("hash node: {e}")))?;
        if let Some(name) = node_name(node) {
            hash_map.insert(name.to_string(), h);
        }
    }
    rewrite_manifest_refs(&mut canonical.manifest, &hash_map);

    // Store new/updated nodes so downstream canonicalization/validation can resolve refs.
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
    if kind == "defsecret" {
        // operate over SecretEntry::Ref
        if let Some(pos) = manifest.secrets.iter().position(
            |e| matches!(e, aos_air_types::SecretEntry::Ref(nr) if nr.name.as_str() == name),
        ) {
            if let aos_air_types::SecretEntry::Ref(nr) = &manifest.secrets[pos] {
                if let Some(pre) = pre_hash {
                    if nr.hash.as_str() != pre {
                        return Err(KernelError::Manifest(format!(
                            "pre_hash mismatch for {name}"
                        )));
                    }
                }
            }
            if remove.is_some() {
                manifest.secrets.remove(pos);
            }
        } else if remove.is_none() {
            manifest
                .secrets
                .push(aos_air_types::SecretEntry::Ref(NamedRef {
                    name: name.into(),
                    hash: zero_hash_ref()?,
                }));
        }
        Ok(())
    } else {
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
}

fn apply_manifest_refs(manifest: &mut Manifest, refs: SetManifestRefs) -> Result<(), KernelError> {
    for add in refs.add {
        if add.kind == "defsecret" {
            apply_secret_ref_add(manifest, &add.name, &add.hash)?;
        } else {
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
    }
    for rem in refs.remove {
        if rem.kind == "defsecret" {
            manifest
                .secrets
                .retain(|e| !matches!(e, aos_air_types::SecretEntry::Ref(nr) if nr.name.as_str() == rem.name));
        } else {
            let target = refs_for_kind_mut(manifest, &rem.kind)?;
            target.retain(|r| r.name.as_str() != rem.name);
        }
    }
    Ok(())
}

fn apply_defaults(manifest: &mut Manifest, defaults: SetDefaults) {
    let mut new_defaults = manifest.defaults.clone().unwrap_or(ManifestDefaults {
        policy: None,
        cap_grants: Vec::new(),
    });
    match defaults.policy {
        Some(Some(policy)) => new_defaults.policy = Some(policy),
        Some(None) => new_defaults.policy = None,
        None => {}
    }
    if let Some(cap_grants) = defaults.cap_grants {
        new_defaults.cap_grants = cap_grants;
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

fn apply_routing_events(manifest: &mut Manifest, op: SetRoutingEvents) -> Result<(), KernelError> {
    let routing = manifest
        .routing
        .get_or_insert_with(|| aos_air_types::Routing {
            events: Vec::new(),
            inboxes: Vec::new(),
        });
    verify_block_pre_hash(&routing.events, &op.pre_hash, "routing.events")?;
    routing.events = op.events;
    Ok(())
}

fn apply_routing_inboxes(
    manifest: &mut Manifest,
    op: SetRoutingInboxes,
) -> Result<(), KernelError> {
    let routing = manifest
        .routing
        .get_or_insert_with(|| aos_air_types::Routing {
            events: Vec::new(),
            inboxes: Vec::new(),
        });
    verify_block_pre_hash(&routing.inboxes, &op.pre_hash, "routing.inboxes")?;
    routing.inboxes = op.inboxes;
    Ok(())
}

fn apply_triggers(manifest: &mut Manifest, op: SetTriggers) -> Result<(), KernelError> {
    verify_block_pre_hash(&manifest.triggers, &op.pre_hash, "triggers")?;
    manifest.triggers = op.triggers;
    Ok(())
}

fn apply_module_bindings(
    manifest: &mut Manifest,
    op: SetModuleBindings,
) -> Result<(), KernelError> {
    let current: IndexMap<String, aos_air_types::ModuleBinding> = manifest
        .module_bindings
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    verify_block_pre_hash(&current, &op.pre_hash, "module_bindings")?;
    manifest.module_bindings = op.bindings;
    Ok(())
}

fn apply_secrets(manifest: &mut Manifest, op: SetSecrets) -> Result<(), KernelError> {
    verify_block_pre_hash(&manifest.secrets, &op.pre_hash, "secrets")?;
    manifest.secrets = op.secrets;
    Ok(())
}

fn apply_secret_ref_add(
    manifest: &mut Manifest,
    name: &str,
    hash: &str,
) -> Result<(), KernelError> {
    let hash_ref =
        aos_air_types::HashRef::new(hash).map_err(|e| KernelError::Manifest(e.to_string()))?;
    if let Some(entry) = manifest
        .secrets
        .iter_mut()
        .find(|e| matches!(e, aos_air_types::SecretEntry::Ref(nr) if nr.name.as_str() == name))
    {
        if let aos_air_types::SecretEntry::Ref(nr) = entry {
            nr.hash = hash_ref;
        }
        return Ok(());
    }
    manifest
        .secrets
        .push(aos_air_types::SecretEntry::Ref(NamedRef {
            name: name.into(),
            hash: hash_ref,
        }));
    Ok(())
}

fn verify_block_pre_hash<T: serde::Serialize>(
    block: &T,
    pre_hash: &str,
    label: &str,
) -> Result<(), KernelError> {
    let current =
        Hash::of_cbor(block).map_err(|e| KernelError::Manifest(format!("hash {label}: {e}")))?;
    if current.to_hex() != pre_hash {
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
        "defplan" => Ok(&mut manifest.plans),
        "defeffect" => Ok(&mut manifest.effects),
        "defcap" => Ok(&mut manifest.caps),
        "defpolicy" => Ok(&mut manifest.policies),
        "defsecret" => {
            // Only handled via specialized path.
            Err(KernelError::Manifest(
                "set_manifest_refs for defsecret handled separately".into(),
            ))
        }
        other => Err(KernelError::Manifest(format!(
            "unknown kind in patch: {other}"
        ))),
    }
}

fn insert_placeholder_ref(
    manifest: &mut Manifest,
    kind: &str,
    name: &str,
) -> Result<(), KernelError> {
    if kind == "defsecret" {
        if manifest
            .secrets
            .iter()
            .any(|e| matches!(e, aos_air_types::SecretEntry::Ref(nr) if nr.name.as_str() == name))
        {
            return Ok(());
        }
        manifest
            .secrets
            .push(aos_air_types::SecretEntry::Ref(NamedRef {
                name: name.to_string(),
                hash: zero_hash_ref()?,
            }));
    } else {
        let refs = refs_for_kind_mut(manifest, kind)?;
        if refs.iter().any(|r| r.name.as_str() == name) {
            return Ok(());
        }
        refs.push(NamedRef {
            name: name.to_string(),
            hash: zero_hash_ref()?,
        });
    }
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

fn reject_sys_name(name: &str, op: &str) -> Result<(), KernelError> {
    if name.starts_with("sys/") {
        return Err(KernelError::Manifest(format!(
            "patch cannot {op} reserved sys/* definition: {name}"
        )));
    }
    Ok(())
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
        let hash = store
            .put_node(&AirNode::Manifest(manifest))
            .expect("store manifest");
        hash.to_hex()
    }

    fn defschema(name: &str) -> AirNode {
        AirNode::Defschema(DefSchema {
            name: name.to_string(),
            ty: TypeExpr::Primitive(TypePrimitive::Bool(TypePrimitiveBool {
                bool: EmptyObject {},
            })),
        })
    }

    #[test]
    fn set_defaults_tri_state_policy_and_caps() {
        let store = MemStore::new();
        // baseline manifest with defaults populated
        let mut manifest = empty_manifest();
        manifest.defaults = Some(ManifestDefaults {
            policy: Some("policy/Old@1".into()),
            cap_grants: vec![CapGrant {
                name: "grant_old".into(),
                cap: "cap/demo@1".into(),
                params: ValueLiteral::Null(ValueNull {
                    null: EmptyObject {},
                }),
                expiry_ns: None,
            }],
        });
        let base_hash = store_manifest(&store, manifest);

        // clear policy, replace caps with empty list
        let doc = PatchDocument {
            version: "1".into(),
            base_manifest_hash: base_hash.clone(),
            patches: vec![PatchOp::SetDefaults {
                set_defaults: SetDefaults {
                    policy: Some(None),       // clear
                    cap_grants: Some(vec![]), // clear
                },
            }],
        };
        let patch = compile_patch_document(&store, doc).expect("compile");
        let defaults = patch.manifest.defaults.expect("defaults present");
        assert!(defaults.policy.is_none(), "policy cleared");
        assert!(defaults.cap_grants.is_empty(), "cap_grants cleared");

        // set policy and set cap grants
        let doc2 = PatchDocument {
            version: "1".into(),
            base_manifest_hash: base_hash,
            patches: vec![PatchOp::SetDefaults {
                set_defaults: SetDefaults {
                    policy: Some(Some("policy/New@1".into())),
                    cap_grants: Some(vec![CapGrant {
                        name: "grant_new".into(),
                        cap: "cap/demo@1".into(),
                        params: ValueLiteral::Null(ValueNull {
                            null: EmptyObject {},
                        }),
                        expiry_ns: None,
                    }]),
                },
            }],
        };
        let patch2 = compile_patch_document(&store, doc2).expect("compile");
        let defaults2 = patch2.manifest.defaults.expect("defaults present");
        assert_eq!(defaults2.policy.as_deref(), Some("policy/New@1"));
        assert_eq!(defaults2.cap_grants.len(), 1);
        assert_eq!(defaults2.cap_grants[0].name, "grant_new");
    }

    #[test]
    fn add_def_updates_manifest_refs() {
        let store = MemStore::new();
        let base_hash = store_manifest(&store, empty_manifest());
        let doc = PatchDocument {
            version: "1".into(),
            base_manifest_hash: base_hash.clone(),
            patches: vec![PatchOp::AddDef {
                add_def: AddDef {
                    kind: "defschema".into(),
                    node: defschema("demo/Foo@1"),
                },
            }],
        };
        let patch = compile_patch_document(&store, doc).expect("compile");
        assert!(
            patch
                .manifest
                .schemas
                .iter()
                .any(|nr| nr.name.as_str() == "demo/Foo@1")
        );
        assert!(
            patch
                .nodes
                .iter()
                .any(|n| matches!(n, AirNode::Defschema(s) if s.name == "demo/Foo@1"))
        );
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
            version: "1".into(),
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
        assert!(
            !patch
                .manifest
                .schemas
                .iter()
                .any(|nr| nr.name.as_str() == "demo/ToRemove@1")
        );
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
            version: "1".into(),
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
            version: "1".into(),
            base_manifest_hash: base_hash,
            patches: vec![PatchOp::SetDefaults {
                set_defaults: SetDefaults {
                    policy: Some(Some("demo/policy@1".into())),
                    cap_grants: Some(vec![CapGrant {
                        name: "g1".into(),
                        cap: "cap/demo@1".into(),
                        params: ValueLiteral::Null(ValueNull {
                            null: EmptyObject {},
                        }),
                        expiry_ns: None,
                    }]),
                },
            }],
        };
        let patch = compile_patch_document(&store, doc).expect("compile");
        let defaults = patch.manifest.defaults.expect("defaults");
        assert_eq!(defaults.policy.as_deref(), Some("demo/policy@1"));
        assert_eq!(defaults.cap_grants.len(), 1);
    }

    #[test]
    fn rejects_unknown_patch_version() {
        let store = MemStore::new();
        let base_hash = store_manifest(&store, empty_manifest());
        let doc = PatchDocument {
            version: "2".into(),
            base_manifest_hash: base_hash,
            patches: vec![],
        };
        let err = compile_patch_document(&store, doc).unwrap_err();
        assert!(
            format!("{err}").contains("unsupported patch document version"),
            "version mismatch should error"
        );
    }

    #[test]
    fn set_routing_events_replaces_block_with_pre_hash() {
        let store = MemStore::new();
        let mut manifest = empty_manifest();
        manifest.routing = Some(aos_air_types::Routing {
            events: vec![],
            inboxes: vec![],
        });
        let base_hash = store_manifest(&store, manifest.clone());
        let pre = Hash::of_cbor(&manifest.routing.as_ref().unwrap().events)
            .unwrap()
            .to_hex();
        let doc = PatchDocument {
            version: "1".into(),
            base_manifest_hash: base_hash,
            patches: vec![PatchOp::SetRoutingEvents {
                set_routing_events: SetRoutingEvents {
                    pre_hash: pre,
                    events: vec![aos_air_types::RoutingEvent {
                        event: aos_air_types::SchemaRef::new("com.acme/Evt@1").unwrap(),
                        reducer: "com.acme/Reducer@1".into(),
                        key_field: Some("id".into()),
                    }],
                },
            }],
        };
        let patch = compile_patch_document(&store, doc).expect("compile");
        let routing = patch.manifest.routing.expect("routing present");
        assert_eq!(routing.events.len(), 1);
    }

    #[test]
    fn set_module_bindings_replaces_map() {
        let store = MemStore::new();
        let base_hash = store_manifest(&store, empty_manifest());
        let pre = Hash::of_cbor(&IndexMap::<String, aos_air_types::ModuleBinding>::new())
            .unwrap()
            .to_hex();
        let mut slots = IndexMap::new();
        slots.insert("db".into(), "cap/db@1".into());
        let mut bindings = IndexMap::new();
        bindings.insert(
            "com.acme/mod@1".into(),
            aos_air_types::ModuleBinding { slots },
        );
        let doc = PatchDocument {
            version: "1".into(),
            base_manifest_hash: base_hash,
            patches: vec![PatchOp::SetModuleBindings {
                set_module_bindings: SetModuleBindings {
                    pre_hash: pre,
                    bindings,
                },
            }],
        };
        let patch = compile_patch_document(&store, doc).expect("compile");
        assert_eq!(patch.manifest.module_bindings.len(), 1);
    }

    #[test]
    fn set_secrets_allows_refs() {
        let store = MemStore::new();
        let base_hash = store_manifest(&store, empty_manifest());
        let pre = Hash::of_cbor(&Vec::<aos_air_types::SecretEntry>::new())
            .unwrap()
            .to_hex();
        let doc = PatchDocument {
            version: "1".into(),
            base_manifest_hash: base_hash,
            patches: vec![PatchOp::SetSecrets {
                set_secrets: SetSecrets {
                    pre_hash: pre,
                    secrets: vec![aos_air_types::SecretEntry::Ref(NamedRef {
                        name: "secret/api@1".into(),
                        hash: HashRef::new("sha256:1111111111111111111111111111111111111111111111111111111111111111")
                            .unwrap(),
                    })],
                },
            }],
        };
        let patch = compile_patch_document(&store, doc).expect("compile");
        assert_eq!(patch.manifest.secrets.len(), 1);
    }

    #[test]
    fn rejects_sys_add_def() {
        let store = MemStore::new();
        let base_hash = store_manifest(&store, empty_manifest());
        let doc = PatchDocument {
            version: "1".into(),
            base_manifest_hash: base_hash,
            patches: vec![PatchOp::AddDef {
                add_def: AddDef {
                    kind: "defschema".into(),
                    node: defschema("sys/TimerSetParams@1"),
                },
            }],
        };
        let err = compile_patch_document(&store, doc).unwrap_err();
        assert!(format!("{err}").contains("sys/*"));
    }

    #[test]
    fn rejects_sys_manifest_ref_updates() {
        let store = MemStore::new();
        let base_hash = store_manifest(&store, empty_manifest());
        let doc = PatchDocument {
            version: "1".into(),
            base_manifest_hash: base_hash,
            patches: vec![PatchOp::SetManifestRefs {
                set_manifest_refs: SetManifestRefs {
                    add: vec![ManifestRef {
                        kind: "defschema".into(),
                        name: "sys/TimerSetParams@1".into(),
                        hash: "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                            .into(),
                    }],
                    remove: vec![],
                },
            }],
        };
        let err = compile_patch_document(&store, doc).unwrap_err();
        assert!(format!("{err}").contains("sys/*"));
    }
    #[test]
    fn defsecret_manifest_refs_are_allowed() {
        let store = MemStore::new();
        let base_hash = store_manifest(&store, empty_manifest());
        let doc = PatchDocument {
            version: "1".into(),
            base_manifest_hash: base_hash,
            patches: vec![PatchOp::SetManifestRefs {
                set_manifest_refs: SetManifestRefs {
                    add: vec![ManifestRef {
                        kind: "defsecret".into(),
                        name: "secret/api_key@1".into(),
                        hash: "sha256:1111111111111111111111111111111111111111111111111111111111111111".into(),
                    }],
                    remove: vec![],
                },
            }],
        };
        let patch = compile_patch_document(&store, doc).expect("compile");
        assert_eq!(patch.manifest.secrets.len(), 1);
    }
}
