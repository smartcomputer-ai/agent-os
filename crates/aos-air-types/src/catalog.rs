use indexmap::IndexMap;

use crate::{CapType, DefEffect, EffectKind, OriginScope, SchemaRef};

#[derive(Debug, Clone)]
pub struct EffectCatalogEntry {
    pub kind: EffectKind,
    pub cap_type: CapType,
    pub params_schema: SchemaRef,
    pub receipt_schema: SchemaRef,
    pub origin_scope: OriginScope,
}

#[derive(Debug, Clone, Default)]
pub struct EffectCatalog {
    by_kind: IndexMap<String, EffectCatalogEntry>,
}

impl EffectCatalog {
    pub fn new() -> Self {
        Self {
            by_kind: IndexMap::new(),
        }
    }

    /// Builds a catalog from a list of `defeffect` nodes. Duplicate kinds keep the first definition.
    pub fn from_defs<I>(defs: I) -> Self
    where
        I: IntoIterator<Item = DefEffect>,
    {
        let mut catalog = EffectCatalog::new();
        for def in defs {
            let key = def.kind.as_str().to_string();
            if catalog.by_kind.contains_key(&key) {
                continue;
            }
            catalog.by_kind.insert(
                key,
                EffectCatalogEntry {
                    kind: def.kind.clone(),
                    cap_type: def.cap_type.clone(),
                    params_schema: def.params_schema.clone(),
                    receipt_schema: def.receipt_schema.clone(),
                    origin_scope: def.origin_scope,
                },
            );
        }
        catalog
    }

    pub fn get(&self, kind: &EffectKind) -> Option<&EffectCatalogEntry> {
        self.by_kind.get(kind.as_str())
    }

    pub fn params_schema(&self, kind: &EffectKind) -> Option<&SchemaRef> {
        self.get(kind).map(|e| &e.params_schema)
    }

    pub fn receipt_schema(&self, kind: &EffectKind) -> Option<&SchemaRef> {
        self.get(kind).map(|e| &e.receipt_schema)
    }

    pub fn cap_type(&self, kind: &EffectKind) -> Option<&CapType> {
        self.get(kind).map(|e| &e.cap_type)
    }

    pub fn origin_scope(&self, kind: &EffectKind) -> Option<OriginScope> {
        self.get(kind).map(|e| e.origin_scope)
    }

    pub fn kinds(&self) -> impl Iterator<Item = &EffectKind> {
        self.by_kind.values().map(|e| &e.kind)
    }
}
