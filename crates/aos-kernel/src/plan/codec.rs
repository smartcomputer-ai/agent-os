use std::collections::BTreeMap;

use aos_air_exec::{
    Env as ExprEnv, Value as ExprValue, ValueKey, ValueMap as ExecValueMap,
    ValueSet as ExecValueSet, eval_expr,
};
use aos_air_types::{
    EmptyObject, ExprOrValue, HashRef, ValueBool, ValueBytes, ValueDec128, ValueDurationNs,
    ValueHash, ValueInt, ValueList, ValueLiteral, ValueMap, ValueMapEntry, ValueNat, ValueNull,
    ValueRecord, ValueSet, ValueText, ValueTimeNs, ValueUuid, ValueVariant,
};
use aos_cbor::Hash;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use indexmap::IndexMap;
use serde_cbor::{self, Value as CborValue};

use crate::error::KernelError;

pub(crate) fn value_to_bool(value: ExprValue) -> Result<bool, KernelError> {
    match value {
        ExprValue::Bool(v) => Ok(v),
        other => Err(KernelError::Manifest(format!(
            "guard expression must return bool, got {:?}",
            other
        ))),
    }
}

pub(crate) fn eval_expr_or_value(
    expr_or_value: &ExprOrValue,
    env: &ExprEnv,
    context: &str,
) -> Result<ExprValue, KernelError> {
    match expr_or_value {
        ExprOrValue::Expr(expr) => {
            eval_expr(expr, env).map_err(|err| KernelError::Manifest(format!("{context}: {err}")))
        }
        ExprOrValue::Literal(literal) => literal_to_value(literal)
            .map_err(|err| KernelError::Manifest(format!("{context}: {err}"))),
        ExprOrValue::Json(_) => Err(KernelError::Manifest(
            "plan literals must be normalized before execution".into(),
        )),
    }
}

pub(super) fn literal_to_value(literal: &ValueLiteral) -> Result<ExprValue, String> {
    match literal {
        ValueLiteral::Null(_) => Ok(ExprValue::Null),
        ValueLiteral::Bool(v) => Ok(ExprValue::Bool(v.bool)),
        ValueLiteral::Int(v) => Ok(ExprValue::Int(v.int)),
        ValueLiteral::Nat(v) => Ok(ExprValue::Nat(v.nat)),
        ValueLiteral::Dec128(v) => Ok(ExprValue::Dec128(v.dec128.clone())),
        ValueLiteral::Bytes(v) => {
            let bytes = BASE64
                .decode(v.bytes_b64.as_bytes())
                .map_err(|err| format!("invalid bytes literal: {err}"))?;
            Ok(ExprValue::Bytes(bytes))
        }
        ValueLiteral::Text(v) => Ok(ExprValue::Text(v.text.clone())),
        ValueLiteral::TimeNs(v) => Ok(ExprValue::TimeNs(v.time_ns)),
        ValueLiteral::DurationNs(v) => Ok(ExprValue::DurationNs(v.duration_ns)),
        ValueLiteral::Hash(v) => Ok(ExprValue::Hash(v.hash.clone())),
        ValueLiteral::Uuid(v) => Ok(ExprValue::Uuid(v.uuid.clone())),
        ValueLiteral::List(list) => {
            let mut out = Vec::with_capacity(list.list.len());
            for value in &list.list {
                out.push(literal_to_value(value)?);
            }
            Ok(ExprValue::List(out))
        }
        ValueLiteral::Set(set) => {
            let mut out = ExecValueSet::new();
            for item in &set.set {
                out.insert(literal_to_value_key(item)?);
            }
            Ok(ExprValue::Set(out))
        }
        ValueLiteral::Map(map) => {
            let mut out = ExecValueMap::new();
            for entry in &map.map {
                let key = literal_to_value_key(&entry.key)?;
                let value = literal_to_value(&entry.value)?;
                out.insert(key, value);
            }
            Ok(ExprValue::Map(out))
        }
        ValueLiteral::SecretRef(secret) => {
            let mut record = IndexMap::with_capacity(2);
            record.insert("alias".into(), ExprValue::Text(secret.alias.clone()));
            record.insert("version".into(), ExprValue::Nat(secret.version));
            Ok(ExprValue::Record(record))
        }
        ValueLiteral::Record(record) => {
            let mut out = IndexMap::with_capacity(record.record.len());
            for (key, value) in &record.record {
                out.insert(key.clone(), literal_to_value(value)?);
            }
            Ok(ExprValue::Record(out))
        }
        ValueLiteral::Variant(variant) => {
            let mut record = IndexMap::with_capacity(2);
            record.insert("$tag".into(), ExprValue::Text(variant.tag.clone()));
            let value = match &variant.value {
                Some(inner) => literal_to_value(inner)?,
                None => ExprValue::Unit,
            };
            record.insert("$value".into(), value);
            Ok(ExprValue::Record(record))
        }
    }
}

fn literal_to_value_key(literal: &ValueLiteral) -> Result<ValueKey, String> {
    match literal {
        ValueLiteral::Int(v) => Ok(ValueKey::Int(v.int)),
        ValueLiteral::Nat(v) => Ok(ValueKey::Nat(v.nat)),
        ValueLiteral::Text(v) => Ok(ValueKey::Text(v.text.clone())),
        ValueLiteral::Uuid(v) => Ok(ValueKey::Uuid(v.uuid.clone())),
        ValueLiteral::Hash(v) => Ok(ValueKey::Hash(v.hash.as_str().to_string())),
        other => Err(format!(
            "map/set key must be int|nat|text|uuid|hash, got {:?}",
            other
        )),
    }
}

pub(crate) fn expr_value_to_cbor_value(value: &ExprValue) -> CborValue {
    match value {
        ExprValue::Unit | ExprValue::Null => CborValue::Null,
        ExprValue::Bool(v) => CborValue::Bool(*v),
        ExprValue::Int(v) => CborValue::Integer(*v as i128),
        ExprValue::Nat(v) => CborValue::Integer(*v as i128),
        ExprValue::Dec128(v) => CborValue::Text(v.clone()),
        ExprValue::Bytes(bytes) => CborValue::Bytes(bytes.clone()),
        ExprValue::Text(text) => CborValue::Text(text.clone()),
        ExprValue::TimeNs(v) => CborValue::Integer(*v as i128),
        ExprValue::DurationNs(v) => CborValue::Integer(*v as i128),
        ExprValue::Hash(hash) => CborValue::Text(hash.as_str().to_string()),
        ExprValue::Uuid(uuid) => CborValue::Text(uuid.clone()),
        ExprValue::List(list) => CborValue::Array(
            list.iter()
                .map(expr_value_to_cbor_value)
                .collect::<Vec<_>>(),
        ),
        ExprValue::Set(set) => CborValue::Array(
            set.iter()
                .map(expr_value_key_to_cbor_value)
                .collect::<Vec<_>>(),
        ),
        ExprValue::Map(map) => {
            let mut out = BTreeMap::new();
            for (key, value) in map {
                out.insert(
                    expr_value_key_to_cbor_value(key),
                    expr_value_to_cbor_value(value),
                );
            }
            CborValue::Map(out)
        }
        ExprValue::Record(record) => {
            if let Some(tagged) = try_convert_variant_record(record) {
                return tagged;
            }
            let mut out = BTreeMap::new();
            for (key, value) in record {
                out.insert(
                    CborValue::Text(key.clone()),
                    expr_value_to_cbor_value(value),
                );
            }
            CborValue::Map(out)
        }
    }
}

pub(super) fn idempotency_key_from_value(value: ExprValue) -> Result<[u8; 32], KernelError> {
    match value {
        ExprValue::Hash(hash) => Hash::from_hex_str(hash.as_str())
            .map(|h| *h.as_bytes())
            .map_err(|err| KernelError::IdempotencyKeyInvalid(err.to_string())),
        ExprValue::Text(text) => Hash::from_hex_str(&text)
            .map(|h| *h.as_bytes())
            .map_err(|err| KernelError::IdempotencyKeyInvalid(err.to_string())),
        ExprValue::Bytes(bytes) => Hash::from_bytes(&bytes)
            .map(|h| *h.as_bytes())
            .map_err(|err| {
                KernelError::IdempotencyKeyInvalid(format!("expected 32 bytes, got {}", err.0))
            }),
        other => Err(KernelError::IdempotencyKeyInvalid(format!(
            "expected hash or bytes, got {}",
            other.kind()
        ))),
    }
}

fn try_convert_variant_record(record: &IndexMap<String, ExprValue>) -> Option<CborValue> {
    if record.len() != 2 {
        return None;
    }
    let tag = match record.get("$tag") {
        Some(ExprValue::Text(tag)) => tag.clone(),
        _ => return None,
    };
    let value = record
        .get("$value")
        .map(expr_value_to_cbor_value)
        .unwrap_or(CborValue::Null);
    let mut out = BTreeMap::new();
    out.insert(CborValue::Text(tag), value);
    Some(CborValue::Map(out))
}

fn expr_value_key_to_cbor_value(key: &ValueKey) -> CborValue {
    match key {
        ValueKey::Int(v) => CborValue::Integer(*v as i128),
        ValueKey::Nat(v) => CborValue::Integer(*v as i128),
        ValueKey::Text(text) => CborValue::Text(text.clone()),
        ValueKey::Hash(hash) => CborValue::Text(hash.clone()),
        ValueKey::Uuid(uuid) => CborValue::Text(uuid.clone()),
    }
}

pub(super) fn decode_receipt_value(payload: &[u8]) -> ExprValue {
    if let Ok(value) = serde_cbor::from_slice::<ExprValue>(payload) {
        return value;
    }
    if let Ok(cbor) = serde_cbor::from_slice::<CborValue>(payload) {
        if let Some(value) = cbor_value_to_expr_value_loose(&cbor) {
            return value;
        }
    }
    ExprValue::Bytes(payload.to_vec())
}

fn cbor_value_to_expr_value_loose(value: &CborValue) -> Option<ExprValue> {
    match value {
        CborValue::Null => Some(ExprValue::Null),
        CborValue::Bool(v) => Some(ExprValue::Bool(*v)),
        CborValue::Integer(v) => {
            if *v >= 0 {
                u64::try_from(*v).ok().map(ExprValue::Nat)
            } else {
                i64::try_from(*v).ok().map(ExprValue::Int)
            }
        }
        CborValue::Bytes(bytes) => Some(ExprValue::Bytes(bytes.clone())),
        CborValue::Text(text) => {
            if let Ok(hash) = HashRef::new(text.clone()) {
                Some(ExprValue::Hash(hash))
            } else {
                Some(ExprValue::Text(text.clone()))
            }
        }
        CborValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(cbor_value_to_expr_value_loose(item)?);
            }
            Some(ExprValue::List(out))
        }
        CborValue::Map(entries) => {
            let all_text = entries
                .iter()
                .all(|(key, _)| matches!(key, CborValue::Text(_)));
            if all_text {
                let mut record = IndexMap::new();
                for (key, value) in entries {
                    let CborValue::Text(field) = key else {
                        continue;
                    };
                    record.insert(field.clone(), cbor_value_to_expr_value_loose(value)?);
                }
                Some(ExprValue::Record(record))
            } else {
                let mut map = ExecValueMap::new();
                for (key, value) in entries {
                    let key = cbor_key_to_value_key(key)?;
                    let value = cbor_value_to_expr_value_loose(value)?;
                    map.insert(key, value);
                }
                Some(ExprValue::Map(map))
            }
        }
        _ => None,
    }
}

fn cbor_key_to_value_key(value: &CborValue) -> Option<ValueKey> {
    match value {
        CborValue::Text(text) => Some(ValueKey::Text(text.clone())),
        CborValue::Integer(v) => {
            if *v >= 0 {
                u64::try_from(*v).ok().map(ValueKey::Nat)
            } else {
                i64::try_from(*v).ok().map(ValueKey::Int)
            }
        }
        _ => None,
    }
}

fn value_key_to_literal(key: &ValueKey) -> ValueLiteral {
    match key {
        ValueKey::Int(v) => ValueLiteral::Int(ValueInt { int: *v }),
        ValueKey::Nat(v) => ValueLiteral::Nat(ValueNat { nat: *v }),
        ValueKey::Text(v) => ValueLiteral::Text(ValueText { text: v.clone() }),
        ValueKey::Hash(v) => ValueLiteral::Hash(ValueHash {
            hash: HashRef::new(v.clone()).expect("hash literal"),
        }),
        ValueKey::Uuid(v) => ValueLiteral::Uuid(ValueUuid { uuid: v.clone() }),
    }
}

pub(super) fn expr_value_to_literal(value: &ExprValue) -> Result<ValueLiteral, String> {
    match value {
        ExprValue::Unit | ExprValue::Null => Ok(ValueLiteral::Null(ValueNull {
            null: EmptyObject::default(),
        })),
        ExprValue::Bool(v) => Ok(ValueLiteral::Bool(ValueBool { bool: *v })),
        ExprValue::Int(v) => Ok(ValueLiteral::Int(ValueInt { int: *v })),
        ExprValue::Nat(v) => Ok(ValueLiteral::Nat(ValueNat { nat: *v })),
        ExprValue::Dec128(v) => Ok(ValueLiteral::Dec128(ValueDec128 { dec128: v.clone() })),
        ExprValue::Bytes(bytes) => Ok(ValueLiteral::Bytes(ValueBytes {
            bytes_b64: BASE64.encode(bytes),
        })),
        ExprValue::Text(text) => Ok(ValueLiteral::Text(ValueText { text: text.clone() })),
        ExprValue::TimeNs(v) => Ok(ValueLiteral::TimeNs(ValueTimeNs { time_ns: *v })),
        ExprValue::DurationNs(v) => Ok(ValueLiteral::DurationNs(ValueDurationNs {
            duration_ns: *v,
        })),
        ExprValue::Hash(hash) => Ok(ValueLiteral::Hash(ValueHash { hash: hash.clone() })),
        ExprValue::Uuid(uuid) => Ok(ValueLiteral::Uuid(ValueUuid { uuid: uuid.clone() })),
        ExprValue::List(list) => {
            let mut out = Vec::with_capacity(list.len());
            for item in list {
                out.push(expr_value_to_literal(item)?);
            }
            Ok(ValueLiteral::List(ValueList { list: out }))
        }
        ExprValue::Set(set) => {
            let mut out = Vec::with_capacity(set.len());
            for key in set {
                out.push(value_key_to_literal(key));
            }
            Ok(ValueLiteral::Set(ValueSet { set: out }))
        }
        ExprValue::Map(map) => {
            let mut entries = Vec::with_capacity(map.len());
            for (key, val) in map {
                entries.push(ValueMapEntry {
                    key: value_key_to_literal(key),
                    value: expr_value_to_literal(val)?,
                });
            }
            Ok(ValueLiteral::Map(ValueMap { map: entries }))
        }
        ExprValue::Record(record) => {
            if record.len() == 2 && record.contains_key("$tag") && record.contains_key("$value") {
                let tag = match record.get("$tag").expect("tag present") {
                    ExprValue::Text(text) => text.clone(),
                    other => return Err(format!("variant $tag must be text, got {:?}", other)),
                };
                let value_literal = match record.get("$value").expect("value present") {
                    ExprValue::Unit => None,
                    ExprValue::Null => None,
                    other => Some(Box::new(expr_value_to_literal(other)?)),
                };
                Ok(ValueLiteral::Variant(ValueVariant {
                    tag,
                    value: value_literal,
                }))
            } else {
                let mut out = IndexMap::with_capacity(record.len());
                for (key, val) in record {
                    out.insert(key.clone(), expr_value_to_literal(val)?);
                }
                Ok(ValueLiteral::Record(ValueRecord { record: out }))
            }
        }
    }
}
