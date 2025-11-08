use std::collections::HashMap;

use aos_air_types::{AirNode, DefModule, DefPlan, Manifest, Name};
use aos_store::{Catalog, Store, load_manifest_from_path};

use crate::error::KernelError;

pub struct LoadedManifest {
    pub manifest: Manifest,
    pub modules: HashMap<Name, DefModule>,
    pub plans: HashMap<Name, DefPlan>,
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
        for (name, entry) in catalog.nodes {
            match entry.node {
                AirNode::Defmodule(module) => {
                    modules.insert(name, module);
                }
                AirNode::Defplan(plan) => {
                    plans.insert(name, plan);
                }
                _ => {}
            }
        }
        Ok(LoadedManifest {
            manifest: catalog.manifest,
            modules,
            plans,
        })
    }
}
