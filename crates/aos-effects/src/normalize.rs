use aos_air_types::catalog::EffectCatalog;
use aos_air_types::plan_literals::SchemaIndex;
use aos_air_types::value_normalize::{ValueNormalizeError, normalize_cbor_by_name};
use thiserror::Error;

use crate::EffectKind;

#[derive(Debug, Error)]
pub enum NormalizeError {
    #[error("unknown effect params schema for {0}")]
    UnknownEffect(String),
    #[error("schema '{0}' not found in catalog")]
    SchemaNotFound(String),
    #[error("failed to decode params CBOR: {0}")]
    Decode(String),
    #[error("params do not conform to schema: {0}")]
    Invalid(String),
    #[error("failed to encode canonical CBOR: {0}")]
    Encode(String),
}

pub fn normalize_effect_params(
    catalog: &EffectCatalog,
    schemas: &SchemaIndex,
    kind: &EffectKind,
    params_cbor: &[u8],
) -> Result<Vec<u8>, NormalizeError> {
    let schema_name = params_schema_name(catalog, kind)
        .ok_or_else(|| NormalizeError::UnknownEffect(kind.as_str().to_string()))?;
    let normalized =
        normalize_cbor_by_name(schemas, schema_name, params_cbor).map_err(map_error)?;
    Ok(normalized.bytes)
}

fn params_schema_name<'a>(catalog: &'a EffectCatalog, kind: &EffectKind) -> Option<&'a str> {
    catalog.params_schema(kind).map(|schema| schema.as_str())
}

fn map_error(err: ValueNormalizeError) -> NormalizeError {
    match err {
        ValueNormalizeError::SchemaNotFound(name) => NormalizeError::SchemaNotFound(name),
        ValueNormalizeError::Decode(msg) => NormalizeError::Decode(msg),
        ValueNormalizeError::Invalid(msg) => NormalizeError::Invalid(msg),
        ValueNormalizeError::Encode(msg) => NormalizeError::Encode(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{
        builtins::builtin_effects, builtins::builtin_schemas, catalog::EffectCatalog,
    };
    use serde_cbor::Value as CborValue;
    use std::collections::BTreeMap;
    use std::collections::HashMap;
    fn header_params(map: Vec<(&str, &str)>) -> CborValue {
        let mut headers = BTreeMap::new();
        for (k, v) in map {
            headers.insert(CborValue::Text(k.into()), CborValue::Text(v.into()));
        }
        let mut root = BTreeMap::new();
        root.insert(
            CborValue::Text("method".into()),
            CborValue::Text("GET".into()),
        );
        root.insert(
            CborValue::Text("url".into()),
            CborValue::Text("https://example.com".into()),
        );
        root.insert(CborValue::Text("headers".into()), CborValue::Map(headers));
        root.insert(CborValue::Text("body_ref".into()), CborValue::Null);
        CborValue::Map(root)
    }

    fn catalog_and_schemas() -> (EffectCatalog, SchemaIndex) {
        let catalog = EffectCatalog::from_defs(builtin_effects().iter().map(|e| e.effect.clone()));
        let mut map = HashMap::new();
        for builtin in builtin_schemas() {
            map.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
        }
        (catalog, SchemaIndex::new(map))
    }

    #[test]
    fn normalizes_header_map_order() {
        let params_a = serde_cbor::to_vec(&header_params(vec![("a", "1"), ("b", "2")])).unwrap();
        let params_b = serde_cbor::to_vec(&header_params(vec![("b", "2"), ("a", "1")])).unwrap();

        let (catalog, schemas) = catalog_and_schemas();
        let norm_a = normalize_effect_params(
            &catalog,
            &schemas,
            &EffectKind::new(crate::EffectKind::HTTP_REQUEST),
            &params_a,
        )
        .unwrap();
        let norm_b = normalize_effect_params(
            &catalog,
            &schemas,
            &EffectKind::new(crate::EffectKind::HTTP_REQUEST),
            &params_b,
        )
        .unwrap();

        assert_eq!(norm_a, norm_b, "header ordering must canonicalize");
    }

    #[test]
    fn rejects_missing_record_field() {
        let mut map = BTreeMap::new();
        map.insert(
            CborValue::Text("method".into()),
            CborValue::Text("GET".into()),
        );
        let value = CborValue::Map(map);
        let bytes = serde_cbor::to_vec(&value).unwrap();
        let (catalog, schemas) = catalog_and_schemas();
        let err = normalize_effect_params(
            &catalog,
            &schemas,
            &EffectKind::new(crate::EffectKind::HTTP_REQUEST),
            &bytes,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("missing field"));
    }

    #[test]
    fn unknown_effect_kind_returns_error() {
        let params = serde_cbor::to_vec(&CborValue::Map(BTreeMap::new())).unwrap();
        let (catalog, schemas) = catalog_and_schemas();
        let err = normalize_effect_params(
            &catalog,
            &schemas,
            &EffectKind::new("custom.effect"),
            &params,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            NormalizeError::UnknownEffect(kind) if kind == "custom.effect".to_string()
        ));
    }
}
