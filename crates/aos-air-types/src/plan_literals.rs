use std::collections::HashMap;

use aos_cbor::to_canonical_cbor;
use indexmap::IndexMap;
use serde_json::Value as JsonValue;
use thiserror::Error;

use crate::typecheck::validate_value_literal;
use crate::{
    DefModule, DefPlan, EffectKind, ExprOrValue, HashRef, TypeExpr, TypeList, TypeMap,
    TypeMapKey, TypeOption, TypePrimitive, TypeRecord, TypeSet, TypeVariant, ValueBytes,
    ValueDec128, ValueDurationNs, ValueHash, ValueInt, ValueList, ValueLiteral, ValueMap,
    ValueMapEntry, ValueNat, ValueNull, ValueRecord, ValueSet, ValueText, ValueTimeNs, ValueUuid,
    ValueVariant,
};

#[derive(Debug, Default)]
pub struct SchemaIndex {
    schemas: HashMap<String, TypeExpr>,
}

impl SchemaIndex {
    pub fn new(schemas: HashMap<String, TypeExpr>) -> Self {
        Self { schemas }
    }

    pub fn insert(&mut self, name: String, ty: TypeExpr) {
        self.schemas.insert(name, ty);
    }

    pub fn get(&self, name: &str) -> Option<&TypeExpr> {
        self.schemas.get(name)
    }
}

#[derive(Debug, Error)]
pub enum PlanLiteralError {
    #[error("schema '{name}' not found")]
    SchemaNotFound { name: String },
    #[error("reducer '{name}' not found")]
    ReducerNotFound { name: String },
    #[error("reducer '{name}' lacks reducer ABI")]
    ReducerMissingAbi { name: String },
    #[error("effect {:?} does not have a parameters schema", kind)]
    UnknownEffect { kind: EffectKind },
    #[error("literal requires schema for {context}")]
    MissingSchema { context: &'static str },
    #[error("invalid literal for schema {schema}: {message}")]
    InvalidLiteral { schema: String, message: String },
    #[error("invalid JSON literal: {0}")]
    InvalidJson(String),
}

pub fn normalize_plan_literals(
    plan: &mut DefPlan,
    schemas: &SchemaIndex,
    _modules: &HashMap<String, DefModule>,
) -> Result<(), PlanLiteralError> {
    for step in &mut plan.steps {
        match &mut step.kind {
            crate::PlanStepKind::EmitEffect(step) => {
                let schema_name =
                    effect_params_schema(&step.kind).ok_or(PlanLiteralError::UnknownEffect {
                        kind: step.kind.clone(),
                    })?;
                let schema =
                    schemas
                        .get(schema_name)
                        .ok_or_else(|| PlanLiteralError::SchemaNotFound {
                            name: schema_name.to_string(),
                        })?;
                normalize_expr_or_value(
                    &mut step.params,
                    schema,
                    schema_name,
                    schemas,
                    "emit_effect.params",
                )?;
            }
            crate::PlanStepKind::Assign(step) => {
                if let Some(schema_ref) = plan.locals.get(&step.bind.var) {
                    let schema = schemas.get(schema_ref.as_str()).ok_or_else(|| {
                        PlanLiteralError::SchemaNotFound {
                            name: schema_ref.as_str().to_string(),
                        }
                    })?;
                    normalize_expr_or_value(
                        &mut step.expr,
                        schema,
                        schema_ref.as_str(),
                        schemas,
                        "assign.expr",
                    )?;
                } else if matches!(step.expr, ExprOrValue::Json(_)) {
                    return Err(PlanLiteralError::MissingSchema {
                        context: "assign.expr",
                    });
                }
            }
            crate::PlanStepKind::End(step) => {
                if let Some(result) = &mut step.result {
                    let output_schema =
                        plan.output
                            .as_ref()
                            .ok_or(PlanLiteralError::MissingSchema {
                                context: "end.result",
                            })?;
                    let schema = schemas.get(output_schema.as_str()).ok_or_else(|| {
                        PlanLiteralError::SchemaNotFound {
                            name: output_schema.as_str().to_string(),
                        }
                    })?;
                    normalize_expr_or_value(
                        result,
                        schema,
                        output_schema.as_str(),
                        schemas,
                        "end.result",
                    )?;
                }
            }
            crate::PlanStepKind::RaiseEvent(step) => {
                if matches!(step.event, ExprOrValue::Json(_)) {
                    return Err(PlanLiteralError::MissingSchema {
                        context: "raise_event.event",
                    });
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn normalize_expr_or_value(
    value: &mut ExprOrValue,
    schema: &TypeExpr,
    schema_name: &str,
    schemas: &SchemaIndex,
    context: &'static str,
) -> Result<(), PlanLiteralError> {
    match value {
        ExprOrValue::Expr(_) => Ok(()),
        ExprOrValue::Literal(literal) => {
            canonicalize_literal(literal, schema, schemas)?;
            validate_literal(literal, schema_name, schema)
        }
        ExprOrValue::Json(json) => {
            let mut literal = parse_json_literal(json, schema, schemas)?;
            canonicalize_literal(&mut literal, schema, schemas)?;
            validate_literal(&literal, schema_name, schema)?;
            *value = ExprOrValue::Literal(literal);
            Ok(())
        }
    }
    .map_err(|err| match err {
        PlanLiteralError::InvalidJson(message) => {
            PlanLiteralError::InvalidJson(format!("{context}: {message}"))
        }
        other => other,
    })
}

fn validate_literal(
    literal: &ValueLiteral,
    schema_name: &str,
    schema: &TypeExpr,
) -> Result<(), PlanLiteralError> {
    validate_value_literal(literal, schema).map_err(|err| PlanLiteralError::InvalidLiteral {
        schema: schema_name.to_string(),
        message: err.to_string(),
    })
}

fn parse_json_literal(
    json: &JsonValue,
    schema: &TypeExpr,
    schemas: &SchemaIndex,
) -> Result<ValueLiteral, PlanLiteralError> {
    match schema {
        TypeExpr::Primitive(primitive) => parse_primitive(json, primitive),
        TypeExpr::Record(record) => parse_record(json, record, schemas),
        TypeExpr::Variant(variant) => parse_variant(json, variant, schemas),
        TypeExpr::List(list) => parse_list(json, list, schemas),
        TypeExpr::Set(set) => parse_set(json, set, schemas),
        TypeExpr::Map(map) => parse_map(json, map, schemas),
        TypeExpr::Option(option) => parse_option(json, option, schemas),
        TypeExpr::Ref(reference) => {
            let target = schemas.get(reference.reference.as_str()).ok_or_else(|| {
                PlanLiteralError::SchemaNotFound {
                    name: reference.reference.as_str().to_string(),
                }
            })?;
            parse_json_literal(json, target, schemas)
        }
    }
}

fn parse_primitive(
    json: &JsonValue,
    primitive: &TypePrimitive,
) -> Result<ValueLiteral, PlanLiteralError> {
    match primitive {
        TypePrimitive::Bool(_) => match json {
            JsonValue::Bool(value) => Ok(ValueLiteral::Bool(crate::ValueBool { bool: *value })),
            _ => Err(PlanLiteralError::InvalidJson(
                "expected boolean literal".into(),
            )),
        },
        TypePrimitive::Int(_) => match json {
            JsonValue::Number(num) => num
                .as_i64()
                .map(|int| ValueLiteral::Int(ValueInt { int }))
                .ok_or_else(|| PlanLiteralError::InvalidJson("invalid int literal".into())),
            JsonValue::String(s) => s
                .parse::<i64>()
                .map(|int| ValueLiteral::Int(ValueInt { int }))
                .map_err(|_| PlanLiteralError::InvalidJson("invalid int literal".into())),
            _ => Err(PlanLiteralError::InvalidJson("expected int literal".into())),
        },
        TypePrimitive::Nat(_) => match json {
            JsonValue::Number(num) => num
                .as_u64()
                .map(|nat| ValueLiteral::Nat(ValueNat { nat }))
                .ok_or_else(|| PlanLiteralError::InvalidJson("invalid nat literal".into())),
            JsonValue::String(s) => s
                .parse::<u64>()
                .map(|nat| ValueLiteral::Nat(ValueNat { nat }))
                .map_err(|_| PlanLiteralError::InvalidJson("invalid nat literal".into())),
            _ => Err(PlanLiteralError::InvalidJson("expected nat literal".into())),
        },
        TypePrimitive::Dec128(_) => match json {
            JsonValue::String(text) => Ok(ValueLiteral::Dec128(ValueDec128 {
                dec128: text.clone(),
            })),
            _ => Err(PlanLiteralError::InvalidJson(
                "decimal128 literals must be strings".into(),
            )),
        },
        TypePrimitive::Bytes(_) => match json {
            JsonValue::String(text) => Ok(ValueLiteral::Bytes(ValueBytes {
                bytes_b64: text.clone(),
            })),
            _ => Err(PlanLiteralError::InvalidJson(
                "bytes literals must be base64 strings".into(),
            )),
        },
        TypePrimitive::Text(_) => match json {
            JsonValue::String(text) => Ok(ValueLiteral::Text(ValueText { text: text.clone() })),
            _ => Err(PlanLiteralError::InvalidJson(
                "expected string literal".into(),
            )),
        },
        TypePrimitive::Time(_) => match json {
            JsonValue::Number(num) => num
                .as_u64()
                .map(|time_ns| ValueLiteral::TimeNs(ValueTimeNs { time_ns }))
                .ok_or_else(|| PlanLiteralError::InvalidJson("invalid time literal".into())),
            _ => Err(PlanLiteralError::InvalidJson(
                "time literals must be integers (ns) in v1".into(),
            )),
        },
        TypePrimitive::Duration(_) => match json {
            JsonValue::Number(num) => num
                .as_i64()
                .map(|duration_ns| ValueLiteral::DurationNs(ValueDurationNs { duration_ns }))
                .ok_or_else(|| PlanLiteralError::InvalidJson("invalid duration literal".into())),
            _ => Err(PlanLiteralError::InvalidJson(
                "duration literals must be integers (ns) in v1".into(),
            )),
        },
        TypePrimitive::Hash(_) => match json {
            JsonValue::String(text) => Ok(ValueLiteral::Hash(ValueHash {
                hash: HashRef::new(text.clone())
                    .map_err(|_| PlanLiteralError::InvalidJson("invalid hash literal".into()))?,
            })),
            _ => Err(PlanLiteralError::InvalidJson(
                "expected hash literal".into(),
            )),
        },
        TypePrimitive::Uuid(_) => match json {
            JsonValue::String(text) => Ok(ValueLiteral::Uuid(ValueUuid { uuid: text.clone() })),
            _ => Err(PlanLiteralError::InvalidJson(
                "expected uuid literal".into(),
            )),
        },
        TypePrimitive::Unit(_) => Ok(ValueLiteral::Null(ValueNull {
            null: crate::EmptyObject::default(),
        })),
    }
}

fn parse_record(
    json: &JsonValue,
    record: &TypeRecord,
    schemas: &SchemaIndex,
) -> Result<ValueLiteral, PlanLiteralError> {
    let obj = json.as_object().ok_or_else(|| {
        PlanLiteralError::InvalidJson("record literals must be JSON objects".into())
    })?;
    let mut map = IndexMap::new();
    for (field, field_type) in &record.record {
        let field_value = obj.get(field).ok_or_else(|| {
            PlanLiteralError::InvalidJson(format!("record missing field '{field}'"))
        })?;
        let literal = parse_json_literal(field_value, field_type, schemas)?;
        map.insert(field.clone(), literal);
    }
    Ok(ValueLiteral::Record(ValueRecord { record: map }))
}

fn parse_variant(
    json: &JsonValue,
    variant: &TypeVariant,
    schemas: &SchemaIndex,
) -> Result<ValueLiteral, PlanLiteralError> {
    let obj = json.as_object().ok_or_else(|| {
        PlanLiteralError::InvalidJson("variant literals must be JSON objects".into())
    })?;
    if obj.len() != 1 {
        return Err(PlanLiteralError::InvalidJson(
            "variant literals must have exactly one tag".into(),
        ));
    }
    let (tag, value) = obj.iter().next().unwrap();
    let ty = variant
        .variant
        .get(tag)
        .ok_or_else(|| PlanLiteralError::InvalidJson(format!("unknown variant tag '{tag}'")))?;
    let literal = if value.is_null() {
        None
    } else {
        Some(Box::new(parse_json_literal(value, ty, schemas)?))
    };
    Ok(ValueLiteral::Variant(ValueVariant {
        tag: tag.clone(),
        value: literal,
    }))
}

fn parse_list(
    json: &JsonValue,
    list: &TypeList,
    schemas: &SchemaIndex,
) -> Result<ValueLiteral, PlanLiteralError> {
    let arr = json
        .as_array()
        .ok_or_else(|| PlanLiteralError::InvalidJson("list literals must be arrays".into()))?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(parse_json_literal(item, &list.list, schemas)?);
    }
    Ok(ValueLiteral::List(ValueList { list: out }))
}

fn parse_set(
    json: &JsonValue,
    set: &TypeSet,
    schemas: &SchemaIndex,
) -> Result<ValueLiteral, PlanLiteralError> {
    let arr = json
        .as_array()
        .ok_or_else(|| PlanLiteralError::InvalidJson("set literals must be arrays".into()))?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(parse_json_literal(item, &set.set, schemas)?);
    }
    Ok(ValueLiteral::Set(ValueSet { set: out }))
}

fn parse_map(
    json: &JsonValue,
    map_type: &TypeMap,
    schemas: &SchemaIndex,
) -> Result<ValueLiteral, PlanLiteralError> {
    let mut entries = Vec::new();
    match (&map_type.map.key, json) {
        (TypeMapKey::Text(_), JsonValue::Object(obj)) => {
            for (key, value) in obj {
                let key_literal = ValueLiteral::Text(ValueText { text: key.clone() });
                let value_literal = parse_json_literal(value, &map_type.map.value, schemas)?;
                entries.push(ValueMapEntry {
                    key: key_literal,
                    value: value_literal,
                });
            }
        }
        (_, JsonValue::Array(items)) => {
            for item in items {
                let pair = item
                    .as_array()
                    .filter(|arr| arr.len() == 2)
                    .ok_or_else(|| {
                        PlanLiteralError::InvalidJson(
                            "map literals must be [[key, value], ...]".into(),
                        )
                    })?;
                let key_schema = TypeExpr::Primitive(match &map_type.map.key {
                    TypeMapKey::Int(inner) => TypePrimitive::Int(inner.clone()),
                    TypeMapKey::Nat(inner) => TypePrimitive::Nat(inner.clone()),
                    TypeMapKey::Text(inner) => TypePrimitive::Text(inner.clone()),
                    TypeMapKey::Uuid(inner) => TypePrimitive::Uuid(inner.clone()),
                    TypeMapKey::Hash(inner) => TypePrimitive::Hash(inner.clone()),
                });
                let key_literal = parse_json_literal(&pair[0], &key_schema, schemas)?;
                let value_literal = parse_json_literal(&pair[1], &map_type.map.value, schemas)?;
                entries.push(ValueMapEntry {
                    key: key_literal,
                    value: value_literal,
                });
            }
        }
        _ => {
            return Err(PlanLiteralError::InvalidJson(
                "map literals must be objects (text keys) or [[key,value],…]".into(),
            ));
        }
    }
    Ok(ValueLiteral::Map(ValueMap { map: entries }))
}

fn parse_option(
    json: &JsonValue,
    option: &TypeOption,
    schemas: &SchemaIndex,
) -> Result<ValueLiteral, PlanLiteralError> {
    if json.is_null() {
        Ok(ValueLiteral::Null(ValueNull {
            null: crate::EmptyObject::default(),
        }))
    } else {
        parse_json_literal(json, &option.option, schemas)
    }
}

fn canonicalize_literal(
    literal: &mut ValueLiteral,
    schema: &TypeExpr,
    schemas: &SchemaIndex,
) -> Result<(), PlanLiteralError> {
    match schema {
        TypeExpr::Primitive(_) => {}
        TypeExpr::Record(record) => {
            if let ValueLiteral::Record(value_record) = literal {
                let extras: Vec<String> = value_record
                    .record
                    .keys()
                    .filter(|key| !record.record.contains_key(*key))
                    .cloned()
                    .collect();
                if let Some(field) = extras.first() {
                    return Err(PlanLiteralError::InvalidJson(format!(
                        "unknown record field '{field}'",
                    )));
                }
                let mut ordered = IndexMap::new();
                for (field, field_type) in &record.record {
                    let mut field_value = value_record
                        .record
                        .shift_remove(field)
                        .ok_or_else(|| PlanLiteralError::InvalidJson(format!(
                            "record missing field '{field}'",
                        )))?;
                    canonicalize_literal(&mut field_value, field_type, schemas)?;
                    ordered.insert(field.clone(), field_value);
                }
                value_record.record = ordered;
            }
        }
        TypeExpr::Variant(variant) => {
            if let ValueLiteral::Variant(value_variant) = literal {
                if let Some(inner) = value_variant.value.as_mut() {
                    if let Some(inner_schema) = variant.variant.get(&value_variant.tag) {
                        canonicalize_literal(inner, inner_schema, schemas)?;
                    }
                }
            }
        }
        TypeExpr::List(list) => {
            if let ValueLiteral::List(value_list) = literal {
                for item in &mut value_list.list {
                    canonicalize_literal(item, &list.list, schemas)?;
                }
            }
        }
        TypeExpr::Set(set) => {
            if let ValueLiteral::Set(value_set) = literal {
                for item in &mut value_set.set {
                    canonicalize_literal(item, &set.set, schemas)?;
                }
                sort_and_dedup(&mut value_set.set)?;
            }
        }
        TypeExpr::Map(map_type) => {
            if let ValueLiteral::Map(value_map) = literal {
                let key_schema = TypeExpr::Primitive(match &map_type.map.key {
                    TypeMapKey::Int(inner) => TypePrimitive::Int(inner.clone()),
                    TypeMapKey::Nat(inner) => TypePrimitive::Nat(inner.clone()),
                    TypeMapKey::Text(inner) => TypePrimitive::Text(inner.clone()),
                    TypeMapKey::Uuid(inner) => TypePrimitive::Uuid(inner.clone()),
                    TypeMapKey::Hash(inner) => TypePrimitive::Hash(inner.clone()),
                });
                for entry in &mut value_map.map {
                    canonicalize_literal(&mut entry.key, &key_schema, schemas)?;
                    canonicalize_literal(&mut entry.value, &map_type.map.value, schemas)?;
                }
                sort_map_entries(&mut value_map.map)?;
            }
        }
        TypeExpr::Option(option) => {
            if !matches!(literal, ValueLiteral::Null(_)) {
                canonicalize_literal(literal, &option.option, schemas)?;
            }
        }
        TypeExpr::Ref(reference) => {
            if let Some(target) = schemas.get(reference.reference.as_str()) {
                canonicalize_literal(literal, target, schemas)?;
            }
        }
    }
    Ok(())
}

fn sort_and_dedup(values: &mut Vec<ValueLiteral>) -> Result<(), PlanLiteralError> {
    let mut with_bytes = Vec::with_capacity(values.len());
    for value in values.drain(..) {
        let bytes = canonical_bytes(&value)?;
        with_bytes.push((bytes, value));
    }
    with_bytes.sort_by(|a, b| a.0.cmp(&b.0));
    with_bytes.dedup_by(|a, b| a.0 == b.0);
    values.extend(with_bytes.into_iter().map(|(_, value)| value));
    Ok(())
}

fn sort_map_entries(entries: &mut Vec<ValueMapEntry>) -> Result<(), PlanLiteralError> {
    let mut with_bytes = Vec::with_capacity(entries.len());
    for entry in entries.drain(..) {
        let bytes = canonical_bytes(&entry.key)?;
        with_bytes.push((bytes, entry));
    }
    with_bytes.sort_by(|a, b| a.0.cmp(&b.0));
    with_bytes.dedup_by(|a, b| a.0 == b.0);
    entries.extend(with_bytes.into_iter().map(|(_, entry)| entry));
    Ok(())
}

fn canonical_bytes(value: &ValueLiteral) -> Result<Vec<u8>, PlanLiteralError> {
    to_canonical_cbor(value).map_err(|err| PlanLiteralError::InvalidJson(err.to_string()))
}

fn effect_params_schema(kind: &EffectKind) -> Option<&'static str> {
    match kind {
        EffectKind::HttpRequest => Some("sys/HttpRequestParams@1"),
        EffectKind::BlobPut => Some("sys/BlobPutParams@1"),
        EffectKind::BlobGet => Some("sys/BlobGetParams@1"),
        EffectKind::TimerSet => Some("sys/TimerSetParams@1"),
        EffectKind::LlmGenerate => Some("sys/LlmGenerateParams@1"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::builtin_schemas;
    use serde_json::json;

    fn schema_index() -> SchemaIndex {
        let mut map = HashMap::new();
        for builtin in builtin_schemas() {
            map.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
        }
        SchemaIndex::new(map)
    }

    #[test]
    fn normalizes_http_params_literals() {
        let mut plan = DefPlan {
            name: "com.acme/Plan@1".into(),
            input: crate::SchemaRef::new("com.acme/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![crate::PlanStep {
                id: "emit".into(),
                kind: crate::PlanStepKind::EmitEffect(crate::PlanStepEmitEffect {
                    kind: EffectKind::HttpRequest,
                    params: ExprOrValue::Json(json!({
                        "method": "GET",
                        "url": "https://example.com",
                        "headers": {"x-test": "ok"},
                        "body_ref": null
                    })),
                    cap: "cap".into(),
                    bind: crate::PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            }],
            edges: vec![],
            required_caps: vec!["cap".into()],
            allowed_effects: vec![EffectKind::HttpRequest],
            invariants: vec![],
        };
        normalize_plan_literals(&mut plan, &schema_index(), &HashMap::new()).unwrap();
        if let crate::PlanStepKind::EmitEffect(step) = &plan.steps[0].kind {
            assert!(matches!(step.params, ExprOrValue::Literal(_)));
        } else {
            panic!("expected emit_effect step");
        }
    }
}
