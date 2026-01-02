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

#[derive(Debug, Clone)]
pub struct GenesisImport {
    pub manifest_hash: String,
    pub manifest_bytes: Vec<u8>,
}

pub fn load_air_bundle(
    store: Arc<FsStore>,
    dir: &Path,
    _filter: BundleFilter,
) -> Result<WorldBundle> {
    let loaded = manifest_loader::load_from_assets(store, dir)
        .with_context(|| format!("load AIR bundle from {}", dir.display()))?
        .ok_or_else(|| anyhow::anyhow!("no manifest found under {}", dir.display()))?;
    Ok(WorldBundle::from_loaded(loaded))
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
    let air_dir = out_dir.join("air");
    fs::create_dir_all(&air_dir).context("create air dir")?;
    fs::create_dir_all(out_dir.join(".aos")).context("create .aos dir")?;

    write_json(
        &air_dir.join("manifest.air.json"),
        &AirNode::Manifest(bundle.manifest.clone()),
    )?;
    write_node_array(
        &air_dir.join("schemas.air.json"),
        bundle
            .schemas
            .iter()
            .cloned()
            .map(AirNode::Defschema)
            .collect(),
    )?;
    write_node_array(
        &air_dir.join("module.air.json"),
        bundle
            .modules
            .iter()
            .cloned()
            .map(AirNode::Defmodule)
            .collect(),
    )?;
    write_node_array(
        &air_dir.join("plans.air.json"),
        bundle
            .plans
            .iter()
            .cloned()
            .map(AirNode::Defplan)
            .collect(),
    )?;
    write_node_array(
        &air_dir.join("effects.air.json"),
        bundle
            .effects
            .iter()
            .cloned()
            .map(AirNode::Defeffect)
            .collect(),
    )?;
    write_node_array(
        &air_dir.join("capabilities.air.json"),
        bundle.caps.iter().cloned().map(AirNode::Defcap).collect(),
    )?;
    write_node_array(
        &air_dir.join("policies.air.json"),
        bundle
            .policies
            .iter()
            .cloned()
            .map(AirNode::Defpolicy)
            .collect(),
    )?;
    write_node_array(
        &air_dir.join("secrets.air.json"),
        bundle
            .secrets
            .iter()
            .cloned()
            .map(AirNode::Defsecret)
            .collect(),
    )?;

    fs::write(out_dir.join(".aos/manifest.air.cbor"), manifest_cbor)
        .context("write manifest.air.cbor")?;
    Ok(())
}

impl WorldBundle {
    pub fn from_loaded(loaded: LoadedManifest) -> Self {
        let mut schemas = loaded.schemas.into_values().collect::<Vec<_>>();
        let mut modules = loaded.modules.into_values().collect::<Vec<_>>();
        let mut plans = loaded.plans.into_values().collect::<Vec<_>>();
        let mut caps = loaded.caps.into_values().collect::<Vec<_>>();
        let mut policies = loaded.policies.into_values().collect::<Vec<_>>();
        let mut effects = loaded.effects.into_values().collect::<Vec<_>>();

        schemas.sort_by(|a, b| a.name.cmp(&b.name));
        modules.sort_by(|a, b| a.name.cmp(&b.name));
        plans.sort_by(|a, b| a.name.cmp(&b.name));
        caps.sort_by(|a, b| a.name.cmp(&b.name));
        policies.sort_by(|a, b| a.name.cmp(&b.name));
        effects.sort_by(|a, b| a.name.cmp(&b.name));

        WorldBundle {
            manifest: loaded.manifest,
            schemas,
            modules,
            plans,
            caps,
            policies,
            effects,
            secrets: Vec::new(),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::CURRENT_AIR_VERSION;
    use aos_store::MemStore;

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
}
