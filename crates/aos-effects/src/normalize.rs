use std::collections::BTreeMap;

use aos_air_types::{
    TypeExpr, TypeMapKey, TypePrimitive, catalog::EffectCatalog, plan_literals::SchemaIndex,
};
use aos_cbor::to_canonical_cbor;
use serde_cbor::Value as CborValue;
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
    let schema = schemas
        .get(schema_name)
        .ok_or_else(|| NormalizeError::SchemaNotFound(schema_name.to_string()))?;

    let value: CborValue = serde_cbor::from_slice(params_cbor)
        .map_err(|err| NormalizeError::Decode(err.to_string()))?;

    let resolved_schema = resolve_schema(schema, schemas)?;
    let normalized = canonicalize_value(value, &resolved_schema, schemas)?;

    to_canonical_cbor(&normalized).map_err(|err| NormalizeError::Encode(err.to_string()))
}

fn params_schema_name<'a>(catalog: &'a EffectCatalog, kind: &EffectKind) -> Option<&'a str> {
    catalog.params_schema(kind).map(|schema| schema.as_str())
}

fn resolve_schema<'a>(
    schema: &'a TypeExpr,
    schemas: &'a SchemaIndex,
) -> Result<TypeExpr, NormalizeError> {
    match schema {
        TypeExpr::Ref(reference) => schemas
            .get(reference.reference.as_str())
            .cloned()
            .ok_or_else(|| {
                NormalizeError::SchemaNotFound(reference.reference.as_str().to_string())
            }),
        _ => Ok(schema.clone()),
    }
}

fn canonicalize_value(
    value: CborValue,
    schema: &TypeExpr,
    schemas: &SchemaIndex,
) -> Result<CborValue, NormalizeError> {
    match schema {
        TypeExpr::Primitive(prim) => canonicalize_primitive(value, prim),
        TypeExpr::Record(record) => {
            let mut map = match value {
                CborValue::Map(m) => m,
                _ => return Err(NormalizeError::Invalid("expected record/map".into())),
            };
            let mut canon_map: BTreeMap<CborValue, CborValue> = BTreeMap::new();
            for (field, ty) in record.record.iter() {
                let field_schema = resolve_schema(ty, schemas)?;
                let raw = match map.remove(&CborValue::Text(field.clone())) {
                    Some(value) => value,
                    None if is_optional_type(&field_schema) => CborValue::Null,
                    None => {
                        return Err(NormalizeError::Invalid(format!(
                            "record missing field '{field}'"
                        )));
                    }
                };
                let canon = canonicalize_value(raw, &field_schema, schemas)?;
                canon_map.insert(CborValue::Text(field.clone()), canon);
            }
            if let Some((extra_key, _)) = map.into_iter().next() {
                if let CborValue::Text(k) = extra_key {
                    return Err(NormalizeError::Invalid(format!(
                        "unknown record field '{k}'"
                    )));
                }
            }
            Ok(CborValue::Map(canon_map))
        }
        TypeExpr::Variant(variant) => {
            let (tag, inner) = decode_variant(value)?;
            let inner_schema = variant
                .variant
                .get(&tag)
                .ok_or_else(|| NormalizeError::Invalid(format!("unknown variant tag '{tag}'")))?;
            let canonical_inner = if let Some(raw_inner) = inner {
                let resolved = resolve_schema(inner_schema, schemas)?;
                Some(Box::new(canonicalize_value(
                    *raw_inner, &resolved, schemas,
                )?))
            } else {
                None
            };
            let mut map = BTreeMap::new();
            map.insert(CborValue::Text("$tag".into()), CborValue::Text(tag));
            if let Some(inner_val) = canonical_inner {
                map.insert(CborValue::Text("$value".into()), *inner_val);
            }
            Ok(CborValue::Map(map))
        }
        TypeExpr::List(list) => {
            let items = match value {
                CborValue::Array(items) => items,
                _ => return Err(NormalizeError::Invalid("expected list".into())),
            };
            let mut canon_items = Vec::with_capacity(items.len());
            let elem_schema = resolve_schema(&list.list, schemas)?;
            for item in items {
                canon_items.push(canonicalize_value(item, &elem_schema, schemas)?);
            }
            Ok(CborValue::Array(canon_items))
        }
        TypeExpr::Set(set) => {
            let items = match value {
                CborValue::Array(items) => items,
                _ => return Err(NormalizeError::Invalid("expected set (array)".into())),
            };
            let elem_schema = resolve_schema(&set.set, schemas)?;
            let mut canon_items = Vec::with_capacity(items.len());
            for item in items {
                canon_items.push(canonicalize_value(item, &elem_schema, schemas)?);
            }
            canon_items.sort_by(|a, b| cbor_bytes(a).cmp(&cbor_bytes(b)));
            canon_items.dedup_by(|a, b| cbor_bytes(a) == cbor_bytes(b));
            Ok(CborValue::Array(canon_items))
        }
        TypeExpr::Map(map_type) => {
            let entries = match value {
                CborValue::Map(entries) => entries,
                _ => return Err(NormalizeError::Invalid("expected map".into())),
            };
            let key_schema = TypeExpr::Primitive(match &map_type.map.key {
                TypeMapKey::Int(inner) => TypePrimitive::Int(inner.clone()),
                TypeMapKey::Nat(inner) => TypePrimitive::Nat(inner.clone()),
                TypeMapKey::Text(inner) => TypePrimitive::Text(inner.clone()),
                TypeMapKey::Uuid(inner) => TypePrimitive::Uuid(inner.clone()),
                TypeMapKey::Hash(inner) => TypePrimitive::Hash(inner.clone()),
            });
            let value_schema = resolve_schema(&map_type.map.value, schemas)?;
            let mut canon_entries: Vec<(CborValue, CborValue)> = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let canon_key = canonicalize_value(k, &key_schema, schemas)?;
                let canon_val = canonicalize_value(v, &value_schema, schemas)?;
                canon_entries.push((canon_key, canon_val));
            }
            canon_entries.sort_by(|a, b| cbor_bytes(&a.0).cmp(&cbor_bytes(&b.0)));
            canon_entries.dedup_by(|a, b| cbor_bytes(&a.0) == cbor_bytes(&b.0));
            let canon_map: BTreeMap<_, _> = canon_entries.into_iter().collect();
            Ok(CborValue::Map(canon_map))
        }
        TypeExpr::Option(option) => match value {
            CborValue::Null => Ok(CborValue::Null),
            other => {
                let inner_schema = resolve_schema(&option.option, schemas)?;
                canonicalize_value(other, &inner_schema, schemas)
            }
        },
        TypeExpr::Ref(_) => unreachable!("refs should be resolved before canonicalize"),
    }
}

fn is_optional_type(schema: &TypeExpr) -> bool {
    matches!(schema, TypeExpr::Option(_))
}

fn canonicalize_primitive(
    value: CborValue,
    prim: &aos_air_types::TypePrimitive,
) -> Result<CborValue, NormalizeError> {
    use aos_air_types::TypePrimitive::*;
    match prim {
        Bool(_) => match value {
            CborValue::Bool(_) => Ok(value),
            CborValue::Text(t) => t
                .parse::<bool>()
                .map(CborValue::Bool)
                .map_err(|_| NormalizeError::Invalid("expected bool".into())),
            _ => Err(NormalizeError::Invalid("expected bool".into())),
        },
        Int(_) => match value {
            CborValue::Integer(i) => {
                let as_i64 = i128_to_i64(i)?;
                Ok(CborValue::Integer(as_i64.into()))
            }
            CborValue::Text(t) => {
                let parsed: i64 = t
                    .parse()
                    .map_err(|_| NormalizeError::Invalid("expected int".into()))?;
                Ok(CborValue::Integer(parsed.into()))
            }
            _ => Err(NormalizeError::Invalid("expected int".into())),
        },
        Nat(_) => match value {
            CborValue::Integer(i) => {
                let val = i128_to_u64(i)?;
                Ok(CborValue::Integer((val as i128).into()))
            }
            CborValue::Text(t) => {
                let parsed: u64 = t
                    .parse()
                    .map_err(|_| NormalizeError::Invalid("expected nat".into()))?;
                Ok(CborValue::Integer((parsed as i128).into()))
            }
            _ => Err(NormalizeError::Invalid("expected nat".into())),
        },
        Dec128(_) => match value {
            CborValue::Text(_) => Ok(value),
            CborValue::Integer(i) => Ok(CborValue::Text(i.to_string())),
            CborValue::Float(f) => Ok(CborValue::Text(f.to_string())),
            _ => Err(NormalizeError::Invalid("expected dec128 string".into())),
        },
        Bytes(_) => match value {
            CborValue::Bytes(_) => Ok(value),
            _ => Err(NormalizeError::Invalid("expected bytes (bstr)".into())),
        },
        Text(_) => match value {
            CborValue::Text(_) => Ok(value),
            _ => Err(NormalizeError::Invalid("expected text".into())),
        },
        Time(_) | Duration(_) | Hash(_) | Uuid(_) => match value {
            CborValue::Integer(_) | CborValue::Text(_) => Ok(value),
            _ => Err(NormalizeError::Invalid("expected scalar".into())),
        },
        Unit(_) => match value {
            CborValue::Null => Ok(CborValue::Null),
            _ => Err(NormalizeError::Invalid("expected unit/null".into())),
        },
    }
}

fn decode_variant(value: CborValue) -> Result<(String, Option<Box<CborValue>>), NormalizeError> {
    match value {
        CborValue::Map(map) => {
            if map.len() == 1 {
                if let Some((CborValue::Text(tag), inner)) = map.iter().next() {
                    return Ok((tag.clone(), Some(Box::new(inner.clone()))));
                }
            }
            let mut tag: Option<String> = None;
            let mut val: Option<CborValue> = None;
            for (k, v) in map.into_iter() {
                match k {
                    CborValue::Text(ref key) if key == "$tag" => {
                        if let CborValue::Text(t) = v {
                            tag = Some(t);
                        }
                    }
                    CborValue::Text(ref key) if key == "$value" => {
                        val = Some(v);
                    }
                    CborValue::Text(ref key) if key == "variant" => {
                        if let CborValue::Map(inner) = v {
                            let mut inner_tag = None;
                            let mut inner_val = None;
                            for (ik, iv) in inner.into_iter() {
                                match ik {
                                    CborValue::Text(ref k) if k == "tag" => {
                                        if let CborValue::Text(t) = iv {
                                            inner_tag = Some(t);
                                        }
                                    }
                                    CborValue::Text(ref k) if k == "value" => inner_val = Some(iv),
                                    _ => {}
                                }
                            }
                            tag = inner_tag.or(tag);
                            val = inner_val.or(val);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(t) = tag {
                return Ok((t, val.map(Box::new)));
            }
            Err(NormalizeError::Invalid("variant missing tag".into()))
        }
        _ => Err(NormalizeError::Invalid("expected variant map".into())),
    }
}

fn i128_to_i64(i: i128) -> Result<i64, NormalizeError> {
    i.try_into()
        .map_err(|_| NormalizeError::Invalid("int out of range".into()))
}

fn i128_to_u64(i: i128) -> Result<u64, NormalizeError> {
    if i < 0 {
        return Err(NormalizeError::Invalid(
            "nat must be >=0 and fit u64".into(),
        ));
    }
    Ok(i as u64)
}

fn cbor_bytes(v: &CborValue) -> Vec<u8> {
    to_canonical_cbor(v).expect("canonical cbor of value")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{
        builtins::builtin_effects, builtins::builtin_schemas, catalog::EffectCatalog,
    };
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
