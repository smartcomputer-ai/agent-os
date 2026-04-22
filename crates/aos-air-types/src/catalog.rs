use indexmap::IndexMap;

use crate::{DefEffect, Name, SchemaRef};

#[derive(Debug, Clone)]
pub struct EffectCatalogEntry {
    pub effect: Name,
    pub params_schema: SchemaRef,
    pub receipt_schema: SchemaRef,
    pub impl_module: Name,
    pub impl_entrypoint: String,
}

#[derive(Debug, Clone, Default)]
pub struct EffectCatalog {
    by_effect: IndexMap<Name, EffectCatalogEntry>,
}

impl EffectCatalog {
    pub fn new() -> Self {
        Self {
            by_effect: IndexMap::new(),
        }
    }

    /// Builds a catalog from `defeffect` nodes. Duplicate names keep the first definition.
    pub fn from_effects<I>(defs: I) -> Self
    where
        I: IntoIterator<Item = DefEffect>,
    {
        let mut catalog = EffectCatalog::new();
        for def in defs {
            if catalog.by_effect.contains_key(&def.name) {
                continue;
            }
            catalog.by_effect.insert(
                def.name.clone(),
                EffectCatalogEntry {
                    effect: def.name,
                    params_schema: def.params,
                    receipt_schema: def.receipt,
                    impl_module: def.implementation.module,
                    impl_entrypoint: def.implementation.entrypoint,
                },
            );
        }
        catalog
    }

    pub fn get(&self, effect: &str) -> Option<&EffectCatalogEntry> {
        self.by_effect.get(effect)
    }

    pub fn params_schema(&self, op: &str) -> Option<&SchemaRef> {
        self.get(op).map(|e| &e.params_schema)
    }

    pub fn receipt_schema(&self, op: &str) -> Option<&SchemaRef> {
        self.get(op).map(|e| &e.receipt_schema)
    }

    pub fn ops(&self) -> impl Iterator<Item = &str> {
        self.by_effect.keys().map(String::as_str)
    }
}
