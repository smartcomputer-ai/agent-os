use std::collections::HashMap;

use aos_air_types::{
    AirNode, DefCap, DefModule, DefPlan, Manifest, Name, builtins::builtin_schemas,
};
use aos_store::{Catalog, Store, load_manifest_from_path};

use crate::error::KernelError;

pub struct LoadedManifest {
    pub manifest: Manifest,
    pub modules: HashMap<Name, DefModule>,
    pub plans: HashMap<Name, DefPlan>,
    pub caps: HashMap<Name, DefCap>,
}

pub struct ManifestLoader;

impl ManifestLoader {
    pub fn load_from_path<S: Store>(
        store: &S,
        path: impl AsRef<std::path::Path>,
    ) -> Result<LoadedManifest, KernelError> {
        let catalog = load_manifest_from_path(store, path)?;
        Self::from_catalog(catalog)
    }

    fn from_catalog(catalog: Catalog) -> Result<LoadedManifest, KernelError> {
        let mut modules = HashMap::new();
        let mut plans = HashMap::new();
        let mut caps = HashMap::new();
        for (name, entry) in catalog.nodes {
            match entry.node {
                AirNode::Defmodule(module) => {
                    modules.insert(name, module);
                }
                AirNode::Defplan(plan) => {
                    plans.insert(name, plan);
                }
                AirNode::Defcap(cap) => {
                    caps.insert(name, cap);
                }
                _ => {}
            }
        }
        Ok(LoadedManifest {
            manifest: attach_builtin_schemas(catalog.manifest),
            modules,
            plans,
            caps,
        })
    }
}

fn attach_builtin_schemas(mut manifest: Manifest) -> Manifest {
    for builtin in builtin_schemas() {
        let exists = manifest
            .schemas
            .iter()
            .any(|named| named.name == builtin.schema.name);
        if !exists {
            manifest.schemas.push(aos_air_types::NamedRef {
                name: builtin.schema.name.clone(),
                hash: builtin.hash_ref.clone(),
            });
        }
    }
    manifest
}
