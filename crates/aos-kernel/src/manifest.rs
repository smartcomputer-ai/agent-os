use std::collections::HashMap;

use aos_air_types::{AirNode, DefModule, Manifest, Name};
use aos_store::{Catalog, Store, load_manifest_from_path};

use crate::error::KernelError;

pub struct LoadedManifest {
    pub manifest: Manifest,
    pub modules: HashMap<Name, DefModule>,
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
        for (name, entry) in catalog.nodes {
            if let AirNode::Defmodule(module) = entry.node {
                modules.insert(name, module);
            }
        }
        Ok(LoadedManifest {
            manifest: catalog.manifest,
            modules,
        })
    }
}
