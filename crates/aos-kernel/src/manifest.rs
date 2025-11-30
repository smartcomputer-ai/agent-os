use std::collections::HashMap;

use aos_air_types::{
    AirNode, DefCap, DefEffect, DefModule, DefPlan, DefPolicy, DefSchema, Manifest, Name,
    SecretDecl, catalog::EffectCatalog,
};
use aos_store::{Catalog, Store, load_manifest_from_path};

use crate::error::KernelError;

pub struct LoadedManifest {
    pub manifest: Manifest,
    pub secrets: Vec<SecretDecl>,
    pub modules: HashMap<Name, DefModule>,
    pub plans: HashMap<Name, DefPlan>,
    pub effects: HashMap<Name, DefEffect>,
    pub caps: HashMap<Name, DefCap>,
    pub policies: HashMap<Name, DefPolicy>,
    pub schemas: HashMap<Name, DefSchema>,
    pub effect_catalog: EffectCatalog,
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
        let mut effects = HashMap::new();
        let mut caps = HashMap::new();
        let mut policies = HashMap::new();
        let mut schemas = HashMap::new();
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
        let effect_catalog = EffectCatalog::from_defs(effects.values().cloned());
        Ok(LoadedManifest {
            manifest,
            secrets: catalog.resolved_secrets,
            modules,
            plans,
            effects,
            caps,
            policies,
            schemas,
            effect_catalog,
        })
    }
}
