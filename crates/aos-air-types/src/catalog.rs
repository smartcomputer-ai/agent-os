use indexmap::IndexMap;

use crate::{DefOp, Name, OpKind, SchemaRef};

#[derive(Debug, Clone)]
pub struct EffectCatalogEntry {
    pub op: Name,
    pub params_schema: SchemaRef,
    pub receipt_schema: SchemaRef,
    pub impl_module: Name,
    pub impl_entrypoint: String,
}

#[derive(Debug, Clone, Default)]
pub struct EffectCatalog {
    by_op: IndexMap<Name, EffectCatalogEntry>,
}

impl EffectCatalog {
    pub fn new() -> Self {
        Self {
            by_op: IndexMap::new(),
        }
    }

    /// Builds a catalog from `defop` effect nodes. Duplicate op names keep the first definition.
    pub fn from_defs<I>(defs: I) -> Self
    where
        I: IntoIterator<Item = DefOp>,
    {
        let mut catalog = EffectCatalog::new();
        for def in defs {
            if def.op_kind != OpKind::Effect || catalog.by_op.contains_key(&def.name) {
                continue;
            }
            let Some(effect) = def.effect.as_ref() else {
                continue;
            };
            catalog.by_op.insert(
                def.name.clone(),
                EffectCatalogEntry {
                    op: def.name,
                    params_schema: effect.params.clone(),
                    receipt_schema: effect.receipt.clone(),
                    impl_module: def.implementation.module,
                    impl_entrypoint: def.implementation.entrypoint,
                },
            );
        }
        catalog
    }

    pub fn get(&self, op: &str) -> Option<&EffectCatalogEntry> {
        self.by_op.get(op)
    }

    pub fn get_by_impl_entrypoint(&self, entrypoint: &str) -> Option<&EffectCatalogEntry> {
        self.by_op
            .values()
            .find(|entry| entry.impl_entrypoint == entrypoint)
    }

    pub fn params_schema(&self, op: &str) -> Option<&SchemaRef> {
        self.get(op).map(|e| &e.params_schema)
    }

    pub fn params_schema_for_runtime(&self, runtime_kind: &str) -> Option<&SchemaRef> {
        self.params_schema(runtime_kind)
            .or_else(|| self.params_schema(&format!("sys/{runtime_kind}@1")))
            .or_else(|| {
                self.get_by_impl_entrypoint(runtime_kind)
                    .map(|entry| &entry.params_schema)
            })
    }

    pub fn receipt_schema(&self, op: &str) -> Option<&SchemaRef> {
        self.get(op).map(|e| &e.receipt_schema)
    }

    pub fn receipt_schema_for_runtime(&self, runtime_kind: &str) -> Option<&SchemaRef> {
        self.receipt_schema(runtime_kind)
            .or_else(|| self.receipt_schema(&format!("sys/{runtime_kind}@1")))
            .or_else(|| {
                self.get_by_impl_entrypoint(runtime_kind)
                    .map(|entry| &entry.receipt_schema)
            })
    }

    pub fn ops(&self) -> impl Iterator<Item = &str> {
        self.by_op.keys().map(String::as_str)
    }
}
