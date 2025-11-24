use indexmap::IndexMap;
use once_cell::sync::Lazy;

use crate::{
    CapType, EffectKind,
    builtins::{BuiltinSchema, find_builtin_schema},
};

#[derive(Debug, Clone)]
pub struct EffectCatalogEntry {
    pub kind: EffectKind,
    pub cap_type: CapType,
    pub params_schema: Option<&'static BuiltinSchema>,
    pub receipt_schema: Option<&'static BuiltinSchema>,
}

static EFFECT_CATALOG: Lazy<IndexMap<String, EffectCatalogEntry>> = Lazy::new(|| {
    let entries = vec![
        (
            EffectKind::HTTP_REQUEST,
            CapType::HTTP_OUT,
            Some("sys/HttpRequestParams@1"),
            Some("sys/HttpRequestReceipt@1"),
        ),
        (
            EffectKind::BLOB_PUT,
            CapType::BLOB,
            Some("sys/BlobPutParams@1"),
            Some("sys/BlobPutReceipt@1"),
        ),
        (
            EffectKind::BLOB_GET,
            CapType::BLOB,
            Some("sys/BlobGetParams@1"),
            Some("sys/BlobGetReceipt@1"),
        ),
        (
            EffectKind::TIMER_SET,
            CapType::TIMER,
            Some("sys/TimerSetParams@1"),
            Some("sys/TimerSetReceipt@1"),
        ),
        (
            EffectKind::LLM_GENERATE,
            CapType::LLM_BASIC,
            Some("sys/LlmGenerateParams@1"),
            Some("sys/LlmGenerateReceipt@1"),
        ),
        (
            EffectKind::VAULT_PUT,
            CapType::SECRET,
            Some("sys/VaultPutParams@1"),
            Some("sys/VaultPutReceipt@1"),
        ),
        (
            EffectKind::VAULT_ROTATE,
            CapType::SECRET,
            Some("sys/VaultRotateParams@1"),
            Some("sys/VaultRotateReceipt@1"),
        ),
    ];

    entries
        .into_iter()
        .map(|(kind, cap_type, params_schema, receipt_schema)| {
            let params_schema = params_schema.map(|name| {
                find_builtin_schema(name)
                    .unwrap_or_else(|| panic!("builtin schema '{name}' must exist"))
            });
            let receipt_schema = receipt_schema.map(|name| {
                find_builtin_schema(name)
                    .unwrap_or_else(|| panic!("builtin schema '{name}' must exist"))
            });

            (
                kind.to_string(),
                EffectCatalogEntry {
                    kind: EffectKind::new(kind),
                    cap_type: CapType::new(cap_type),
                    params_schema,
                    receipt_schema,
                },
            )
        })
        .collect()
});

pub fn effect_catalog() -> &'static IndexMap<String, EffectCatalogEntry> {
    &EFFECT_CATALOG
}

pub fn find_effect(kind: &EffectKind) -> Option<&'static EffectCatalogEntry> {
    EFFECT_CATALOG.get(kind.as_str())
}

pub fn effect_params_schema(kind: &EffectKind) -> Option<&'static BuiltinSchema> {
    find_effect(kind)?.params_schema
}

pub fn effect_receipt_schema(kind: &EffectKind) -> Option<&'static BuiltinSchema> {
    find_effect(kind)?.receipt_schema
}
