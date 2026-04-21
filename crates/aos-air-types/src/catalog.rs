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
            let cap_type = if def.cap_type.as_str() == CapType::INTERNAL_LEGACY {
                legacy_cap_type_for_kind(&def.kind).unwrap_or_else(|| def.cap_type.clone())
            } else {
                def.cap_type.clone()
            };
            catalog.by_kind.insert(
                key,
                EffectCatalogEntry {
                    kind: def.kind.clone(),
                    cap_type,
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

fn legacy_cap_type_for_kind(kind: &EffectKind) -> Option<CapType> {
    let cap_type = match kind.as_str() {
        EffectKind::HTTP_REQUEST => CapType::http_out(),
        EffectKind::BLOB_PUT | EffectKind::BLOB_GET => CapType::blob(),
        EffectKind::TIMER_SET => CapType::timer(),
        EffectKind::PORTAL_SEND => CapType::portal(),
        EffectKind::HOST_SESSION_OPEN
        | EffectKind::HOST_EXEC
        | EffectKind::HOST_SESSION_SIGNAL
        | EffectKind::HOST_FS_READ_FILE
        | EffectKind::HOST_FS_WRITE_FILE
        | EffectKind::HOST_FS_EDIT_FILE
        | EffectKind::HOST_FS_APPLY_PATCH
        | EffectKind::HOST_FS_GREP
        | EffectKind::HOST_FS_GLOB
        | EffectKind::HOST_FS_STAT
        | EffectKind::HOST_FS_EXISTS
        | EffectKind::HOST_FS_LIST_DIR => CapType::host(),
        EffectKind::LLM_GENERATE => CapType::llm_basic(),
        EffectKind::VAULT_PUT | EffectKind::VAULT_ROTATE => CapType::secret(),
        EffectKind::WORKSPACE_RESOLVE
        | EffectKind::WORKSPACE_EMPTY_ROOT
        | EffectKind::WORKSPACE_LIST
        | EffectKind::WORKSPACE_READ_REF
        | EffectKind::WORKSPACE_READ_BYTES
        | EffectKind::WORKSPACE_WRITE_BYTES
        | EffectKind::WORKSPACE_WRITE_REF
        | EffectKind::WORKSPACE_REMOVE
        | EffectKind::WORKSPACE_DIFF
        | EffectKind::WORKSPACE_ANNOTATIONS_GET
        | EffectKind::WORKSPACE_ANNOTATIONS_SET => CapType::workspace(),
        EffectKind::INTROSPECT_MANIFEST
        | EffectKind::INTROSPECT_WORKFLOW_STATE
        | EffectKind::INTROSPECT_JOURNAL_HEAD
        | EffectKind::INTROSPECT_LIST_CELLS => CapType::query(),
        "governance.propose" | "governance.shadow" | "governance.approve" | "governance.apply" => {
            CapType::new("governance")
        }
        _ => return None,
    };
    Some(cap_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_legacy_builtin_cap_type_when_defeffect_omits_it() {
        let def = DefEffect {
            name: "sys/http.request@1".into(),
            kind: EffectKind::http_request(),
            params_schema: SchemaRef::new("sys/HttpRequestParams@1").unwrap(),
            receipt_schema: SchemaRef::new("sys/HttpRequestReceipt@1").unwrap(),
            cap_type: CapType::new(CapType::INTERNAL_LEGACY),
            origin_scope: OriginScope::Both,
        };

        let catalog = EffectCatalog::from_defs([def]);
        assert_eq!(
            catalog.cap_type(&EffectKind::http_request()),
            Some(&CapType::http_out())
        );
    }
}
