use std::collections::BTreeMap;

use aos_cbor::to_canonical_cbor;
use serde_cbor::Value as CborValue;
use thiserror::Error;

use crate::{TypeExpr, TypeMapKey, TypePrimitive, schema_index::SchemaIndex};

#[derive(Debug, Error)]
pub enum ValueNormalizeError {
    #[error("schema '{0}' not found")]
    SchemaNotFound(String),
    #[error("failed to decode CBOR: {0}")]
    Decode(String),
    #[error("value does not conform to schema: {0}")]
    Invalid(String),
    #[error("failed to encode canonical CBOR: {0}")]
    Encode(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedValue {
    pub value: CborValue,
    pub bytes: Vec<u8>,
}

pub fn normalize_cbor_by_name(
    schemas: &SchemaIndex,
    schema_name: &str,
    payload: &[u8],
) -> Result<NormalizedValue, ValueNormalizeError> {
    let schema = schemas
        .get(schema_name)
        .ok_or_else(|| ValueNormalizeError::SchemaNotFound(schema_name.to_string()))?;
    normalize_cbor_with_schema(schemas, schema, payload)
}

pub fn normalize_cbor_with_schema(
    schemas: &SchemaIndex,
    schema: &TypeExpr,
    payload: &[u8],
) -> Result<NormalizedValue, ValueNormalizeError> {
    let value: CborValue = serde_cbor::from_slice(payload)
        .map_err(|err| ValueNormalizeError::Decode(err.to_string()))?;
    normalize_value_with_schema(value, schema, schemas)
}

pub fn normalize_value_with_schema(
    value: CborValue,
    schema: &TypeExpr,
    schemas: &SchemaIndex,
) -> Result<NormalizedValue, ValueNormalizeError> {
    let resolved_schema = resolve_schema(schema, schemas)?;
    let canonical_value = canonicalize_value(value, &resolved_schema, schemas)?;
    let bytes = to_canonical_cbor(&canonical_value)
        .map_err(|err| ValueNormalizeError::Encode(err.to_string()))?;
    Ok(NormalizedValue {
        value: canonical_value,
        bytes,
    })
}

fn resolve_schema<'a>(
    schema: &'a TypeExpr,
    schemas: &'a SchemaIndex,
) -> Result<TypeExpr, ValueNormalizeError> {
    match schema {
        TypeExpr::Ref(reference) => schemas
            .get(reference.reference.as_str())
            .cloned()
            .ok_or_else(|| {
                ValueNormalizeError::SchemaNotFound(reference.reference.as_str().to_string())
            }),
        _ => Ok(schema.clone()),
    }
}

fn canonicalize_value(
    value: CborValue,
    schema: &TypeExpr,
    schemas: &SchemaIndex,
) -> Result<CborValue, ValueNormalizeError> {
    match schema {
        TypeExpr::Primitive(prim) => canonicalize_primitive(value, prim),
        TypeExpr::Record(record) => {
            let mut map = match value {
                CborValue::Map(m) => m,
                _ => return Err(ValueNormalizeError::Invalid("expected record/map".into())),
            };
            let mut canon_map: BTreeMap<CborValue, CborValue> = BTreeMap::new();
            for (field, ty) in record.record.iter() {
                let field_schema = resolve_schema(ty, schemas)?;
                let raw = match map.remove(&CborValue::Text(field.clone())) {
                    Some(value) => value,
                    None if is_optional_type(&field_schema) => CborValue::Null,
                    None => {
                        return Err(ValueNormalizeError::Invalid(format!(
                            "record missing field '{field}'"
                        )));
                    }
                };
                let canon = canonicalize_value(raw, &field_schema, schemas)?;
                canon_map.insert(CborValue::Text(field.clone()), canon);
            }
            if let Some((extra_key, _)) = map.into_iter().next() {
                if let CborValue::Text(k) = extra_key {
                    return Err(ValueNormalizeError::Invalid(format!(
                        "unknown record field '{k}'"
                    )));
                }
            }
            Ok(CborValue::Map(canon_map))
        }
        TypeExpr::Variant(variant) => {
            let (tag, inner) = decode_variant(value)?;
            let inner_schema = variant.variant.get(&tag).ok_or_else(|| {
                ValueNormalizeError::Invalid(format!("unknown variant tag '{tag}'"))
            })?;
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
                _ => return Err(ValueNormalizeError::Invalid("expected list".into())),
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
                _ => return Err(ValueNormalizeError::Invalid("expected set (array)".into())),
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
                _ => return Err(ValueNormalizeError::Invalid("expected map".into())),
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

fn cbor_bytes(value: &CborValue) -> Vec<u8> {
    to_canonical_cbor(value).unwrap_or_default()
}

fn is_optional_type(schema: &TypeExpr) -> bool {
    matches!(schema, TypeExpr::Option(_))
}

fn canonicalize_primitive(
    value: CborValue,
    prim: &TypePrimitive,
) -> Result<CborValue, ValueNormalizeError> {
    use crate::TypePrimitive::*;
    match prim {
        Bool(_) => match value {
            CborValue::Bool(_) => Ok(value),
            CborValue::Text(t) => t
                .parse::<bool>()
                .map(CborValue::Bool)
                .map_err(|_| ValueNormalizeError::Invalid("expected bool".into())),
            _ => Err(ValueNormalizeError::Invalid("expected bool".into())),
        },
        Int(_) => match value {
            CborValue::Integer(_) => Ok(value),
            _ => Err(ValueNormalizeError::Invalid("expected int".into())),
        },
        Nat(_) => match value {
            CborValue::Integer(i) if i >= 0 => Ok(value),
            _ => Err(ValueNormalizeError::Invalid("expected nat".into())),
        },
        Dec128(_) => match value {
            CborValue::Text(_) => Ok(value),
            _ => Err(ValueNormalizeError::Invalid(
                "expected dec128 string".into(),
            )),
        },
        Bytes(_) => match value {
            CborValue::Bytes(_) => Ok(value),
            CborValue::Text(t) => Ok(CborValue::Bytes(t.into_bytes())),
            _ => Err(ValueNormalizeError::Invalid("expected bytes".into())),
        },
        Text(_) => match value {
            CborValue::Text(_) => Ok(value),
            _ => Err(ValueNormalizeError::Invalid("expected text".into())),
        },
        Time(_) => match value {
            CborValue::Integer(_) => Ok(value),
            _ => Err(ValueNormalizeError::Invalid(
                "expected time_ns as integer".into(),
            )),
        },
        Duration(_) => match value {
            CborValue::Integer(_) => Ok(value),
            _ => Err(ValueNormalizeError::Invalid(
                "expected duration_ns as integer".into(),
            )),
        },
        Hash(_) => match value {
            CborValue::Text(_) => Ok(value),
            _ => Err(ValueNormalizeError::Invalid("expected hash text".into())),
        },
        Uuid(_) => match value {
            CborValue::Text(_) => Ok(value),
            _ => Err(ValueNormalizeError::Invalid("expected uuid text".into())),
        },
        Unit(_) => Ok(CborValue::Null),
    }
}

fn decode_variant(
    value: CborValue,
) -> Result<(String, Option<Box<CborValue>>), ValueNormalizeError> {
    match value {
        CborValue::Map(mut map) => {
            let tag = match map.remove(&CborValue::Text("$tag".into())) {
                Some(CborValue::Text(tag)) => tag,
                Some(_) => {
                    return Err(ValueNormalizeError::Invalid(
                        "variant $tag must be text".into(),
                    ));
                }
                None => {
                    // Also allow {Tag: value} form
                    if map.len() == 1 {
                        if let Some((CborValue::Text(tag), inner)) = map.into_iter().next() {
                            return Ok((tag, Some(Box::new(inner))));
                        }
                    }
                    return Err(ValueNormalizeError::Invalid("variant missing $tag".into()));
                }
            };
            let value = map.remove(&CborValue::Text("$value".into())).map(Box::new);
            Ok((tag, value))
        }
        _ => Err(ValueNormalizeError::Invalid("expected variant map".into())),
    }
}
