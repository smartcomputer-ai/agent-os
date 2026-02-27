use std::collections::{BTreeMap, BTreeSet};

use aos_air_exec::{Value as ExprValue, ValueKey};
use aos_air_types::schema_index::SchemaIndex;
use aos_air_types::{HashRef, TypeExpr, TypeMapKey, TypePrimitive};
use serde_cbor::Value as CborValue;

use crate::error::KernelError;

pub fn cbor_to_expr_value(
    value: &CborValue,
    schema: &TypeExpr,
    schemas: &SchemaIndex,
) -> Result<ExprValue, KernelError> {
    match schema {
        TypeExpr::Primitive(prim) => primitive_to_expr(value, prim),
        TypeExpr::Record(record) => {
            let map = match value {
                CborValue::Map(map) => map,
                _ => {
                    return Err(KernelError::Manifest(format!(
                        "expected record for schema, got {:?}",
                        value
                    )));
                }
            };
            let mut out = BTreeMap::new();
            for (field, ty) in record.record.iter() {
                let field_schema = resolve_schema(ty, schemas)?;
                let raw = match map.get(&CborValue::Text(field.clone())) {
                    Some(val) => val.clone(),
                    None if matches!(field_schema, TypeExpr::Option(_)) => CborValue::Null,
                    None => {
                        return Err(KernelError::Manifest(format!(
                            "record missing field '{field}' for event"
                        )));
                    }
                };
                let expr = cbor_to_expr_value(&raw, field_schema, schemas)?;
                out.insert(field.clone(), expr);
            }
            Ok(ExprValue::Record(out.into_iter().collect()))
        }
        TypeExpr::Variant(variant) => {
            let (tag, inner) = decode_variant(value)?;
            let inner_schema = variant
                .variant
                .get(&tag)
                .ok_or_else(|| KernelError::Manifest(format!("unknown variant tag '{tag}'")))?;
            let resolved_schema = resolve_schema(inner_schema, schemas)?;
            let expr_inner = match inner {
                Some(raw) => Some(Box::new(cbor_to_expr_value(raw, resolved_schema, schemas)?)),
                None => None,
            };
            let mut record = BTreeMap::new();
            record.insert("$tag".into(), ExprValue::Text(tag));
            if let Some(inner_val) = expr_inner {
                record.insert("$value".into(), *inner_val);
            }
            Ok(ExprValue::Record(record.into_iter().collect()))
        }
        TypeExpr::List(list) => {
            let items = match value {
                CborValue::Array(items) => items,
                _ => {
                    return Err(KernelError::Manifest(format!(
                        "expected list for schema, got {:?}",
                        value
                    )));
                }
            };
            let mut out = Vec::with_capacity(items.len());
            let elem_schema = resolve_schema(&list.list, schemas)?;
            for item in items {
                out.push(cbor_to_expr_value(item, elem_schema, schemas)?);
            }
            Ok(ExprValue::List(out))
        }
        TypeExpr::Set(set) => {
            let items = match value {
                CborValue::Array(items) => items,
                _ => {
                    return Err(KernelError::Manifest(format!(
                        "expected set for schema, got {:?}",
                        value
                    )));
                }
            };
            let elem_schema = resolve_schema(&set.set, schemas)?;
            let mut out = BTreeSet::new();
            for item in items {
                let key = value_key_from_cbor(item, elem_schema)?;
                out.insert(key);
            }
            Ok(ExprValue::Set(out))
        }
        TypeExpr::Map(map_type) => {
            let entries = match value {
                CborValue::Map(entries) => entries,
                _ => {
                    return Err(KernelError::Manifest(format!(
                        "expected map for schema, got {:?}",
                        value
                    )));
                }
            };
            let value_schema = resolve_schema(&map_type.map.value, schemas)?;
            let mut out = BTreeMap::new();
            for (k, v) in entries {
                let key = value_key_from_map_key(k, &map_type.map.key)?;
                let val = cbor_to_expr_value(v, value_schema, schemas)?;
                out.insert(key, val);
            }
            Ok(ExprValue::Map(out))
        }
        TypeExpr::Option(option) => match value {
            CborValue::Null => Ok(ExprValue::Null),
            other => cbor_to_expr_value(other, resolve_schema(&option.option, schemas)?, schemas),
        },
        TypeExpr::Ref(reference) => {
            let target = schemas.get(reference.reference.as_str()).ok_or_else(|| {
                KernelError::Manifest(format!(
                    "schema '{}' not found for ref",
                    reference.reference
                ))
            })?;
            cbor_to_expr_value(value, target, schemas)
        }
    }
}

fn primitive_to_expr(value: &CborValue, prim: &TypePrimitive) -> Result<ExprValue, KernelError> {
    use TypePrimitive::*;
    match prim {
        Bool(_) => match value {
            CborValue::Bool(b) => Ok(ExprValue::Bool(*b)),
            _ => Err(KernelError::Manifest("expected bool".into())),
        },
        Int(_) => match value {
            CborValue::Integer(i) => {
                let val: i64 = (*i)
                    .try_into()
                    .map_err(|_| KernelError::Manifest("int out of range".into()))?;
                Ok(ExprValue::Int(val))
            }
            _ => Err(KernelError::Manifest("expected int".into())),
        },
        Nat(_) => match value {
            CborValue::Integer(i) if *i >= 0 => {
                let val: u64 = (*i)
                    .try_into()
                    .map_err(|_| KernelError::Manifest("nat out of range".into()))?;
                Ok(ExprValue::Nat(val))
            }
            _ => Err(KernelError::Manifest("expected nat".into())),
        },
        Dec128(_) => match value {
            CborValue::Text(t) => Ok(ExprValue::Dec128(t.clone())),
            _ => Err(KernelError::Manifest("expected dec128 text".into())),
        },
        Bytes(_) => match value {
            CborValue::Bytes(b) => Ok(ExprValue::Bytes(b.clone())),
            _ => Err(KernelError::Manifest("expected bytes".into())),
        },
        Text(_) => match value {
            CborValue::Text(t) => Ok(ExprValue::Text(t.clone())),
            _ => Err(KernelError::Manifest("expected text".into())),
        },
        Time(_) => match value {
            CborValue::Integer(i) if *i >= 0 => Ok(ExprValue::TimeNs(*i as u64)),
            _ => Err(KernelError::Manifest("expected time_ns integer".into())),
        },
        Duration(_) => match value {
            CborValue::Integer(i) => Ok(ExprValue::DurationNs(*i as i64)),
            _ => Err(KernelError::Manifest("expected duration_ns integer".into())),
        },
        Hash(_) => match value {
            CborValue::Text(t) => HashRef::new(t.clone())
                .map(ExprValue::Hash)
                .map_err(|_| KernelError::Manifest("expected hash text".into())),
            _ => Err(KernelError::Manifest("expected hash text".into())),
        },
        Uuid(_) => match value {
            CborValue::Text(t) => Ok(ExprValue::Uuid(t.clone())),
            _ => Err(KernelError::Manifest("expected uuid text".into())),
        },
        Unit(_) => Ok(ExprValue::Unit),
    }
}

fn value_key_from_cbor(value: &CborValue, elem_schema: &TypeExpr) -> Result<ValueKey, KernelError> {
    match elem_schema {
        TypeExpr::Primitive(TypePrimitive::Int(_)) => match value {
            CborValue::Integer(i) => (*i)
                .try_into()
                .map(ValueKey::Int)
                .map_err(|_| KernelError::Manifest("int key out of range".into())),
            _ => Err(KernelError::Manifest("set key must be int".into())),
        },
        TypeExpr::Primitive(TypePrimitive::Nat(_)) => match value {
            CborValue::Integer(i) if *i >= 0 => (*i)
                .try_into()
                .map(ValueKey::Nat)
                .map_err(|_| KernelError::Manifest("nat key out of range".into())),
            _ => Err(KernelError::Manifest("set key must be nat".into())),
        },
        TypeExpr::Primitive(TypePrimitive::Text(_)) => match value {
            CborValue::Text(t) => Ok(ValueKey::Text(t.clone())),
            _ => Err(KernelError::Manifest("set key must be text".into())),
        },
        TypeExpr::Primitive(TypePrimitive::Uuid(_)) => match value {
            CborValue::Text(t) => Ok(ValueKey::Uuid(t.clone())),
            _ => Err(KernelError::Manifest("set key must be uuid".into())),
        },
        TypeExpr::Primitive(TypePrimitive::Hash(_)) => match value {
            CborValue::Text(t) => Ok(ValueKey::Hash(t.clone())),
            _ => Err(KernelError::Manifest("set key must be hash".into())),
        },
        _ => Err(KernelError::Manifest(
            "set keys must be comparable primitives (int|nat|text|uuid|hash)".into(),
        )),
    }
}

fn value_key_from_map_key(
    value: &CborValue,
    key_type: &TypeMapKey,
) -> Result<ValueKey, KernelError> {
    match key_type {
        TypeMapKey::Int(_) => match value {
            CborValue::Integer(i) => (*i)
                .try_into()
                .map(ValueKey::Int)
                .map_err(|_| KernelError::Manifest("map key int out of range".into())),
            _ => Err(KernelError::Manifest("map key must be int".into())),
        },
        TypeMapKey::Nat(_) => match value {
            CborValue::Integer(i) if *i >= 0 => (*i)
                .try_into()
                .map(ValueKey::Nat)
                .map_err(|_| KernelError::Manifest("map key nat out of range".into())),
            _ => Err(KernelError::Manifest("map key must be nat".into())),
        },
        TypeMapKey::Text(_) => match value {
            CborValue::Text(t) => Ok(ValueKey::Text(t.clone())),
            _ => Err(KernelError::Manifest("map key must be text".into())),
        },
        TypeMapKey::Uuid(_) => match value {
            CborValue::Text(t) => Ok(ValueKey::Uuid(t.clone())),
            _ => Err(KernelError::Manifest("map key must be uuid".into())),
        },
        TypeMapKey::Hash(_) => match value {
            CborValue::Text(t) => Ok(ValueKey::Hash(t.clone())),
            _ => Err(KernelError::Manifest("map key must be hash".into())),
        },
    }
}

fn resolve_schema<'a>(
    schema: &'a TypeExpr,
    schemas: &'a SchemaIndex,
) -> Result<&'a TypeExpr, KernelError> {
    match schema {
        TypeExpr::Ref(reference) => schemas.get(reference.reference.as_str()).ok_or_else(|| {
            KernelError::Manifest(format!(
                "schema '{}' not found for ref",
                reference.reference
            ))
        }),
        _ => Ok(schema),
    }
}

fn decode_variant(value: &CborValue) -> Result<(String, Option<&CborValue>), KernelError> {
    match value {
        CborValue::Map(map) => {
            let tag = match map.get(&CborValue::Text("$tag".into())) {
                Some(CborValue::Text(tag)) => tag.clone(),
                Some(_) => return Err(KernelError::Manifest("variant $tag must be text".into())),
                None => {
                    if map.len() == 1 {
                        if let Some((CborValue::Text(tag), inner)) = map.iter().next() {
                            return Ok((tag.clone(), Some(inner)));
                        }
                    }
                    return Err(KernelError::Manifest("variant missing $tag field".into()));
                }
            };
            let inner = map.get(&CborValue::Text("$value".into()));
            Ok((tag, inner))
        }
        _ => Err(KernelError::Manifest(
            "expected variant map with $tag".into(),
        )),
    }
}
