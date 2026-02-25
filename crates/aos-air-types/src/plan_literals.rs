use std::collections::HashMap;

use aos_cbor::to_canonical_cbor;
use indexmap::IndexMap;
use serde_json::Value as JsonValue;
use thiserror::Error;

use crate::{
    DefPlan, EffectKind, EmptyObject, ExprOrValue, HashRef, TypeExpr, TypeList, TypeMap,
    TypeMapEntry, TypeMapKey, TypeOption, TypePrimitive, TypePrimitiveHash, TypeRecord, TypeSet,
    TypeVariant, ValueBool, ValueBytes, ValueDec128, ValueDurationNs, ValueHash, ValueInt,
    ValueList, ValueLiteral, ValueMap, ValueMapEntry, ValueNat, ValueNull, ValueRecord, ValueSet,
    ValueText, ValueTimeNs, ValueUuid, ValueVariant,
};
use crate::{Expr, ExprConst, typecheck::validate_value_literal};

#[derive(Debug, Default, Clone)]
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
    effects: &crate::catalog::EffectCatalog,
) -> Result<(), PlanLiteralError> {
    normalize_plan_literals_with_plan_inputs(plan, schemas, effects, &HashMap::new())
}

pub fn normalize_plan_literals_with_plan_inputs(
    plan: &mut DefPlan,
    schemas: &SchemaIndex,
    effects: &crate::catalog::EffectCatalog,
    plan_input_schemas: &HashMap<String, String>,
) -> Result<(), PlanLiteralError> {
    for step in &mut plan.steps {
        match &mut step.kind {
            crate::PlanStepKind::EmitEffect(step) => {
                let schema_name = effect_params_schema(&step.kind, effects).ok_or(
                    PlanLiteralError::UnknownEffect {
                        kind: step.kind.clone(),
                    },
                )?;
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
                if let Some(key) = &mut step.idempotency_key {
                    let schema = TypeExpr::Primitive(TypePrimitive::Hash(TypePrimitiveHash {
                        hash: EmptyObject::default(),
                    }));
                    normalize_expr_or_value(
                        key,
                        &schema,
                        "idempotency_key",
                        schemas,
                        "emit_effect.idempotency_key",
                    )?;
                }
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
                normalize_raise_event_literal(step, schemas)?;
            }
            crate::PlanStepKind::SpawnPlan(step) => {
                if let Some(input_schema_name) = plan_input_schemas.get(step.plan.as_str()) {
                    let schema = schemas.get(input_schema_name.as_str()).ok_or_else(|| {
                        PlanLiteralError::SchemaNotFound {
                            name: input_schema_name.clone(),
                        }
                    })?;
                    normalize_expr_or_value(
                        &mut step.input,
                        schema,
                        input_schema_name,
                        schemas,
                        "spawn_plan.input",
                    )?;
                } else if matches!(step.input, ExprOrValue::Json(_)) {
                    return Err(PlanLiteralError::MissingSchema {
                        context: "spawn_plan.input",
                    });
                }
            }
            crate::PlanStepKind::SpawnForEach(step) => {
                if let Some(input_schema_name) = plan_input_schemas.get(step.plan.as_str()) {
                    let item_schema = schemas.get(input_schema_name.as_str()).ok_or_else(|| {
                        PlanLiteralError::SchemaNotFound {
                            name: input_schema_name.clone(),
                        }
                    })?;
                    let list_schema = TypeExpr::List(TypeList {
                        list: Box::new(item_schema.clone()),
                    });
                    normalize_expr_or_value(
                        &mut step.inputs,
                        &list_schema,
                        input_schema_name,
                        schemas,
                        "spawn_for_each.inputs",
                    )?;
                } else if matches!(step.inputs, ExprOrValue::Json(_)) {
                    return Err(PlanLiteralError::MissingSchema {
                        context: "spawn_for_each.inputs",
                    });
                }
            }
            _ => {}
        }
    }

    // Ensure derived capability/effect lists are populated and canonicalized.
    crate::validate::normalize_plan_caps_and_effects(plan);
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
        ExprOrValue::Expr(expr) => {
            normalize_parsed_expr(expr.clone(), value, schema, schema_name, schemas)
        }
        ExprOrValue::Literal(literal) => {
            canonicalize_literal(literal, schema, schemas)?;
            validate_literal(literal, schema, schema_name, schemas)
        }
        ExprOrValue::Json(json) => {
            // First, attempt to interpret the JSON as an expression. If it matches the Expr
            // shape, keep it as an Expr so dynamic params/values are allowed.
            if let Ok(expr) = serde_json::from_value::<Expr>(json.clone()) {
                return normalize_parsed_expr(expr, value, schema, schema_name, schemas);
            }

            let mut literal = parse_json_literal(json, schema, schemas)?;
            canonicalize_literal(&mut literal, schema, schemas)?;
            validate_literal(&literal, schema, schema_name, schemas)?;
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

fn normalize_parsed_expr(
    expr: Expr,
    slot: &mut ExprOrValue,
    schema: &TypeExpr,
    schema_name: &str,
    schemas: &SchemaIndex,
) -> Result<(), PlanLiteralError> {
    if let Some(mut literal) = const_expr_to_literal(&expr) {
        canonicalize_literal(&mut literal, schema, schemas)?;
        validate_literal(&literal, schema, schema_name, schemas)?;
        *slot = ExprOrValue::Literal(literal);
    } else {
        *slot = ExprOrValue::Expr(expr);
    }
    Ok(())
}

fn const_expr_to_literal(expr: &Expr) -> Option<ValueLiteral> {
    match expr {
        Expr::Const(c) => Some(expr_const_to_literal(c)),
        Expr::Record(record) => {
            let mut out = IndexMap::with_capacity(record.record.len());
            for (key, value) in &record.record {
                out.insert(key.clone(), const_expr_to_literal(value)?);
            }
            Some(ValueLiteral::Record(ValueRecord { record: out }))
        }
        Expr::List(list) => {
            let mut out = Vec::with_capacity(list.list.len());
            for item in &list.list {
                out.push(const_expr_to_literal(item)?);
            }
            Some(ValueLiteral::List(ValueList { list: out }))
        }
        Expr::Set(set) => {
            let mut out = Vec::with_capacity(set.set.len());
            for item in &set.set {
                out.push(const_expr_to_literal(item)?);
            }
            Some(ValueLiteral::Set(ValueSet { set: out }))
        }
        Expr::Map(map) => {
            let mut out = Vec::with_capacity(map.map.len());
            for entry in &map.map {
                let key = const_expr_to_literal(&entry.key)?;
                let value = const_expr_to_literal(&entry.value)?;
                out.push(ValueMapEntry { key, value });
            }
            Some(ValueLiteral::Map(ValueMap { map: out }))
        }
        Expr::Variant(variant) => {
            let value = match &variant.variant.value {
                Some(inner) => Some(Box::new(const_expr_to_literal(inner)?)),
                None => None,
            };
            Some(ValueLiteral::Variant(ValueVariant {
                tag: variant.variant.tag.clone(),
                value,
            }))
        }
        Expr::Ref(_) | Expr::Op(_) => None,
    }
}

fn expr_const_to_literal(constant: &ExprConst) -> ValueLiteral {
    match constant {
        ExprConst::Null { .. } => ValueLiteral::Null(ValueNull {
            null: crate::EmptyObject::default(),
        }),
        ExprConst::Bool { bool } => ValueLiteral::Bool(ValueBool { bool: *bool }),
        ExprConst::Int { int } => ValueLiteral::Int(ValueInt { int: *int }),
        ExprConst::Nat { nat } => ValueLiteral::Nat(ValueNat { nat: *nat }),
        ExprConst::Dec128 { dec128 } => ValueLiteral::Dec128(ValueDec128 {
            dec128: dec128.clone(),
        }),
        ExprConst::Text { text } => ValueLiteral::Text(ValueText { text: text.clone() }),
        ExprConst::Bytes { bytes_b64 } => ValueLiteral::Bytes(ValueBytes {
            bytes_b64: bytes_b64.clone(),
        }),
        ExprConst::Time { time_ns } => ValueLiteral::TimeNs(ValueTimeNs { time_ns: *time_ns }),
        ExprConst::Duration { duration_ns } => ValueLiteral::DurationNs(ValueDurationNs {
            duration_ns: *duration_ns,
        }),
        ExprConst::Hash { hash } => ValueLiteral::Hash(ValueHash { hash: hash.clone() }),
        ExprConst::Uuid { uuid } => ValueLiteral::Uuid(ValueUuid { uuid: uuid.clone() }),
    }
}

pub fn validate_literal(
    literal: &ValueLiteral,
    schema: &TypeExpr,
    schema_name: &str,
    schemas: &SchemaIndex,
) -> Result<(), PlanLiteralError> {
    let (expanded, resolved_name) = expand_schema(schema, schema_name, schemas)?;
    validate_value_literal(literal, &expanded).map_err(|err| PlanLiteralError::InvalidLiteral {
        schema: resolved_name,
        message: err.to_string(),
    })
}

fn expand_schema(
    schema: &TypeExpr,
    schema_name: &str,
    schemas: &SchemaIndex,
) -> Result<(TypeExpr, String), PlanLiteralError> {
    match schema {
        TypeExpr::Ref(reference) => {
            let target_name = reference.reference.as_str();
            let target =
                schemas
                    .get(target_name)
                    .ok_or_else(|| PlanLiteralError::SchemaNotFound {
                        name: target_name.to_string(),
                    })?;
            expand_schema(target, target_name, schemas)
        }
        TypeExpr::Primitive(_) => Ok((schema.clone(), schema_name.to_string())),
        TypeExpr::Record(record) => {
            let mut expanded = IndexMap::new();
            for (field, field_type) in &record.record {
                let (expanded_field, _) = expand_schema(field_type, schema_name, schemas)?;
                expanded.insert(field.clone(), expanded_field);
            }
            Ok((
                TypeExpr::Record(TypeRecord { record: expanded }),
                schema_name.to_string(),
            ))
        }
        TypeExpr::Variant(variant) => {
            let mut expanded = IndexMap::new();
            for (tag, ty) in &variant.variant {
                let (expanded_ty, _) = expand_schema(ty, schema_name, schemas)?;
                expanded.insert(tag.clone(), expanded_ty);
            }
            Ok((
                TypeExpr::Variant(TypeVariant { variant: expanded }),
                schema_name.to_string(),
            ))
        }
        TypeExpr::List(list) => {
            let (expanded_inner, _) = expand_schema(&list.list, schema_name, schemas)?;
            Ok((
                TypeExpr::List(TypeList {
                    list: Box::new(expanded_inner),
                }),
                schema_name.to_string(),
            ))
        }
        TypeExpr::Set(set) => {
            let (expanded_inner, _) = expand_schema(&set.set, schema_name, schemas)?;
            Ok((
                TypeExpr::Set(TypeSet {
                    set: Box::new(expanded_inner),
                }),
                schema_name.to_string(),
            ))
        }
        TypeExpr::Map(map) => {
            let (expanded_value, _) = expand_schema(&map.map.value, schema_name, schemas)?;
            Ok((
                TypeExpr::Map(TypeMap {
                    map: TypeMapEntry {
                        key: map.map.key.clone(),
                        value: Box::new(expanded_value),
                    },
                }),
                schema_name.to_string(),
            ))
        }
        TypeExpr::Option(option) => {
            let (expanded_inner, _) = expand_schema(&option.option, schema_name, schemas)?;
            Ok((
                TypeExpr::Option(TypeOption {
                    option: Box::new(expanded_inner),
                }),
                schema_name.to_string(),
            ))
        }
    }
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

pub fn canonicalize_literal(
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
                    let mut field_value =
                        value_record.record.shift_remove(field).ok_or_else(|| {
                            PlanLiteralError::InvalidJson(
                                format!("record missing field '{field}'",),
                            )
                        })?;
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

fn effect_params_schema<'a>(
    kind: &EffectKind,
    effects: &'a crate::catalog::EffectCatalog,
) -> Option<&'a str> {
    effects.params_schema(kind).map(|schema| schema.as_str())
}

fn normalize_raise_event_literal(
    step: &mut crate::PlanStepRaiseEvent,
    schemas: &SchemaIndex,
) -> Result<(), PlanLiteralError> {
    let schema_name = step.event.as_str();
    let schema = schemas
        .get(schema_name)
        .ok_or_else(|| PlanLiteralError::SchemaNotFound {
            name: schema_name.to_string(),
        })?;
    normalize_expr_or_value(
        &mut step.value,
        schema,
        schema_name,
        schemas,
        "raise_event.value",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::builtin_schemas;
    use crate::{
        TypePrimitiveBytes, TypePrimitiveDec128, TypePrimitiveDuration, TypePrimitiveHash,
        TypePrimitiveInt, TypePrimitiveNat, TypePrimitiveText, TypePrimitiveTime,
        TypePrimitiveUuid,
    };
    use aos_cbor::{Hash, to_canonical_cbor};
    use serde_json::{Value, json};

    fn schema_index() -> SchemaIndex {
        let mut map = HashMap::new();
        for builtin in builtin_schemas() {
            map.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
        }
        SchemaIndex::new(map)
    }

    fn effect_catalog() -> crate::catalog::EffectCatalog {
        crate::catalog::EffectCatalog::from_defs(
            crate::builtins::builtin_effects()
                .iter()
                .map(|e| e.effect.clone()),
        )
    }

    #[test]
    fn expr_const_null_parses() {
        let expr: Expr = serde_json::from_value(json!({ "null": {} })).expect("parse expr");
        assert!(matches!(expr, Expr::Const(ExprConst::Null { .. })));
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
                    kind: EffectKind::http_request(),
                    params: ExprOrValue::Json(json!({
                        "method": "GET",
                        "url": "https://example.com",
                        "headers": {"x-test": "ok"},
                        "body_ref": null
                    })),
                    cap: "cap".into(),
                    idempotency_key: None,
                    bind: crate::PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            }],
            edges: vec![],
            required_caps: vec!["cap".into()],
            allowed_effects: vec![EffectKind::http_request()],
            invariants: vec![],
        };
        normalize_plan_literals(&mut plan, &schema_index(), &effect_catalog()).unwrap();
        if let crate::PlanStepKind::EmitEffect(step) = &plan.steps[0].kind {
            assert!(matches!(step.params, ExprOrValue::Literal(_)));
        } else {
            panic!("expected emit_effect step");
        }
    }

    #[test]
    fn rejects_const_wrapper_null_in_expr() {
        let mut plan = DefPlan {
            name: "com.acme/Plan@1".into(),
            input: crate::SchemaRef::new("com.acme/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![crate::PlanStep {
                id: "emit".into(),
                kind: crate::PlanStepKind::EmitEffect(crate::PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: ExprOrValue::Json(json!({
                        "method": "GET",
                        "url": "https://example.com",
                        "headers": {"x-test": "ok"},
                        "body_ref": { "const": { "null": {} } }
                    })),
                    cap: "cap".into(),
                    idempotency_key: None,
                    bind: crate::PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            }],
            edges: vec![],
            required_caps: vec!["cap".into()],
            allowed_effects: vec![EffectKind::http_request()],
            invariants: vec![],
        };

        let err = normalize_plan_literals(&mut plan, &schema_index(), &effect_catalog())
            .expect_err("should reject const wrapper in expr");
        assert!(matches!(err, PlanLiteralError::InvalidJson(_)));
    }

    #[test]
    fn normalizes_llm_generate_params_literals() {
        let mut plan = DefPlan {
            name: "com.acme/Plan@1".into(),
            input: crate::SchemaRef::new("com.acme/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![crate::PlanStep {
                id: "emit".into(),
                kind: crate::PlanStepKind::EmitEffect(crate::PlanStepEmitEffect {
                    kind: EffectKind::llm_generate(),
                    params: ExprOrValue::Json(json!({
                        "correlation_id": null,
                        "provider": "openai",
                        "model": "gpt-5.2",
                        "message_refs": ["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
                        "runtime": {
                            "temperature": "0.5",
                            "top_p": null,
                            "max_tokens": 128,
                            "tool_refs": null,
                            "tool_choice": null,
                            "reasoning_effort": null,
                            "stop_sequences": null,
                            "metadata": null,
                            "provider_options_ref": null,
                            "response_format_ref": null
                        },
                        "api_key": null
                    })),
                    cap: "cap_llm".into(),
                    idempotency_key: None,
                    bind: crate::PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            }],
            edges: vec![],
            required_caps: vec!["cap_llm".into()],
            allowed_effects: vec![EffectKind::llm_generate()],
            invariants: vec![],
        };
        normalize_plan_literals(&mut plan, &schema_index(), &effect_catalog()).unwrap();
        if let crate::PlanStepKind::EmitEffect(step) = &plan.steps[0].kind {
            assert!(matches!(step.params, ExprOrValue::Literal(_)));
        } else {
            panic!("expected emit_effect step");
        }
    }

    #[test]
    fn normalizes_raise_event_literals_against_event_schema() {
        let mut plan = DefPlan {
            name: "com.acme/Plan@1".into(),
            input: crate::SchemaRef::new("com.acme/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![crate::PlanStep {
                id: "raise".into(),
                kind: crate::PlanStepKind::RaiseEvent(crate::PlanStepRaiseEvent {
                    event: crate::SchemaRef::new("sys/TimerFired@1").unwrap(),
                    value: ExprOrValue::Json(json!({
                        "intent_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                        "reducer": "com.acme/Reducer@1",
                        "effect_kind": "timer.set",
                        "adapter_id": "timer",
                        "status": "ok",
                        "requested": { "deliver_at_ns": 1, "key": "remind" },
                        "receipt": { "delivered_at_ns": 2, "key": "remind" },
                        "cost_cents": 0,
                        "signature": "AA=="
                    })),
                }),
            }],
            edges: vec![],
            required_caps: vec![],
            allowed_effects: vec![],
            invariants: vec![],
        };

        normalize_plan_literals(&mut plan, &schema_index(), &effect_catalog()).unwrap();

        if let crate::PlanStepKind::RaiseEvent(step) = &plan.steps[0].kind {
            assert!(matches!(step.value, ExprOrValue::Literal(_)));
        } else {
            panic!("expected raise_event step");
        }
    }

    #[test]
    fn raise_event_literal_without_schema_errors() {
        let mut plan = DefPlan {
            name: "com.acme/Plan@1".into(),
            input: crate::SchemaRef::new("com.acme/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![crate::PlanStep {
                id: "raise".into(),
                kind: crate::PlanStepKind::RaiseEvent(crate::PlanStepRaiseEvent {
                    event: crate::SchemaRef::new("com.acme/Missing@1").unwrap(),
                    value: ExprOrValue::Json(json!({})),
                }),
            }],
            edges: vec![],
            required_caps: vec![],
            allowed_effects: vec![],
            invariants: vec![],
        };

        let err =
            normalize_plan_literals(&mut plan, &schema_index(), &effect_catalog()).unwrap_err();
        assert!(matches!(err, PlanLiteralError::SchemaNotFound { .. }));
    }

    #[test]
    fn normalizes_spawn_plan_input_literals() {
        let mut schemas = schema_index();
        schemas.insert(
            "com.acme/ChildInput@1".into(),
            TypeExpr::Record(TypeRecord {
                record: IndexMap::from([(
                    "id".into(),
                    TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                        text: crate::EmptyObject::default(),
                    })),
                )]),
            }),
        );

        let mut plan = DefPlan {
            name: "com.acme/Parent@1".into(),
            input: crate::SchemaRef::new("com.acme/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![crate::PlanStep {
                id: "spawn".into(),
                kind: crate::PlanStepKind::SpawnPlan(crate::PlanStepSpawnPlan {
                    plan: "com.acme/Child@1".into(),
                    input: ExprOrValue::Json(json!({"id": "abc"})),
                    bind: crate::PlanBindHandle {
                        handle_as: "child".into(),
                    },
                }),
            }],
            edges: vec![],
            required_caps: vec![],
            allowed_effects: vec![],
            invariants: vec![],
        };
        let plan_inputs = HashMap::from([(
            "com.acme/Child@1".to_string(),
            "com.acme/ChildInput@1".to_string(),
        )]);
        normalize_plan_literals_with_plan_inputs(
            &mut plan,
            &schemas,
            &effect_catalog(),
            &plan_inputs,
        )
        .unwrap();
        let crate::PlanStepKind::SpawnPlan(step) = &plan.steps[0].kind else {
            panic!("expected spawn_plan");
        };
        assert!(matches!(step.input, ExprOrValue::Literal(_)));
    }

    #[test]
    fn normalizes_spawn_for_each_inputs_literals() {
        let mut schemas = schema_index();
        schemas.insert(
            "com.acme/ChildInput@1".into(),
            TypeExpr::Record(TypeRecord {
                record: IndexMap::from([(
                    "id".into(),
                    TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                        text: crate::EmptyObject::default(),
                    })),
                )]),
            }),
        );

        let mut plan = DefPlan {
            name: "com.acme/Parent@1".into(),
            input: crate::SchemaRef::new("com.acme/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![crate::PlanStep {
                id: "spawn_many".into(),
                kind: crate::PlanStepKind::SpawnForEach(crate::PlanStepSpawnForEach {
                    plan: "com.acme/Child@1".into(),
                    inputs: ExprOrValue::Json(json!([{"id": "a"}, {"id": "b"}])),
                    max_fanout: Some(10),
                    bind: crate::PlanBindHandles {
                        handles_as: "children".into(),
                    },
                }),
            }],
            edges: vec![],
            required_caps: vec![],
            allowed_effects: vec![],
            invariants: vec![],
        };
        let plan_inputs = HashMap::from([(
            "com.acme/Child@1".to_string(),
            "com.acme/ChildInput@1".to_string(),
        )]);
        normalize_plan_literals_with_plan_inputs(
            &mut plan,
            &schemas,
            &effect_catalog(),
            &plan_inputs,
        )
        .unwrap();
        let crate::PlanStepKind::SpawnForEach(step) = &plan.steps[0].kind else {
            panic!("expected spawn_for_each");
        };
        assert!(matches!(step.inputs, ExprOrValue::Literal(_)));
    }

    #[test]
    fn emit_effect_params_json_expr_is_kept_as_expr() {
        let mut plan = DefPlan {
            name: "com.acme/Plan@1".into(),
            input: crate::SchemaRef::new("com.acme/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![crate::PlanStep {
                id: "emit".into(),
                kind: crate::PlanStepKind::EmitEffect(crate::PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: ExprOrValue::Json(json!({
                        "record": {
                            "method": { "text": "GET" },
                            "url": { "op": "get", "args": [ { "ref": "@plan.input" }, { "text": "url" } ] },
                            "headers": { "map": [] },
                            "body_ref": { "null": {} }
                        }
                    })),
                    cap: "cap".into(),
                    idempotency_key: None,
                    bind: crate::PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            }],
            edges: vec![],
            required_caps: vec!["cap".into()],
            allowed_effects: vec![EffectKind::http_request()],
            invariants: vec![],
        };

        let mut schemas = schema_index();
        schemas.insert(
            "com.acme/Input@1".into(),
            TypeExpr::Record(TypeRecord {
                record: IndexMap::from([(
                    "url".into(),
                    TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                        text: crate::EmptyObject::default(),
                    })),
                )]),
            }),
        );

        normalize_plan_literals(&mut plan, &schemas, &effect_catalog()).expect("normalize");
        if let crate::PlanStepKind::EmitEffect(step) = &plan.steps[0].kind {
            assert!(
                matches!(step.params, ExprOrValue::Expr(_)),
                "params should remain expression"
            );
        } else {
            panic!("expected emit_effect step");
        }
    }

    #[test]
    fn constant_expr_is_folded_to_literal() {
        let mut plan = DefPlan {
            name: "com.acme/Plan@1".into(),
            input: crate::SchemaRef::new("com.acme/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![crate::PlanStep {
                id: "emit".into(),
                kind: crate::PlanStepKind::EmitEffect(crate::PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: ExprOrValue::Json(json!({
                        "record": {
                            "method": { "text": "POST" },
                            "url": { "text": "https://example.com" },
                            "headers": { "map": [] },
                            "body_ref": { "null": {} }
                        }
                    })),
                    cap: "cap".into(),
                    idempotency_key: None,
                    bind: crate::PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            }],
            edges: vec![],
            required_caps: vec!["cap".into()],
            allowed_effects: vec![EffectKind::http_request()],
            invariants: vec![],
        };

        normalize_plan_literals(&mut plan, &schema_index(), &effect_catalog()).expect("normalize");
        if let crate::PlanStepKind::EmitEffect(step) = &plan.steps[0].kind {
            assert!(
                matches!(step.params, ExprOrValue::Literal(_)),
                "constant expression should be folded into literal for load-time validation"
            );
        } else {
            panic!("expected emit_effect step");
        }
    }

    fn assert_sugar_and_tagged_equal(schema: TypeExpr, sugar: Value, tagged: Value) {
        let schemas = SchemaIndex::new(HashMap::new());
        let mut sugar_literal = parse_json_literal(&sugar, &schema, &schemas).expect("parse sugar");
        canonicalize_literal(&mut sugar_literal, &schema, &schemas).expect("canonicalize sugar");

        let mut tagged_literal: ValueLiteral =
            serde_json::from_value(tagged).expect("tagged literal json");
        canonicalize_literal(&mut tagged_literal, &schema, &schemas)
            .expect("canonicalize tagged literal");

        let sugar_bytes = to_canonical_cbor(&sugar_literal).expect("sugar cbor");
        let tagged_bytes = to_canonical_cbor(&tagged_literal).expect("tagged cbor");
        assert_eq!(sugar_bytes, tagged_bytes, "canonical CBOR mismatch");

        let sugar_hash = Hash::of_cbor(&sugar_literal).expect("hash sugar");
        let tagged_hash = Hash::of_cbor(&tagged_literal).expect("hash tagged");
        assert_eq!(sugar_hash, tagged_hash, "value hash mismatch");
    }

    #[test]
    fn sugar_text_literal_matches_tagged_literal() {
        let schema = TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
            text: crate::EmptyObject::default(),
        }));
        assert_sugar_and_tagged_equal(schema, json!("hello"), json!({"text": "hello"}));
    }

    #[test]
    fn sugar_stringified_int_matches_tagged_literal() {
        let schema = TypeExpr::Primitive(TypePrimitive::Int(TypePrimitiveInt {
            int: crate::EmptyObject::default(),
        }));
        assert_sugar_and_tagged_equal(schema, json!("-42"), json!({"int": -42}));
    }

    #[test]
    fn sugar_dec128_literal_matches_tagged_literal() {
        let schema = TypeExpr::Primitive(TypePrimitive::Dec128(TypePrimitiveDec128 {
            dec128: crate::EmptyObject::default(),
        }));
        assert_sugar_and_tagged_equal(schema, json!("3.14159"), json!({"dec128": "3.14159"}));
    }

    #[test]
    fn sugar_bytes_literal_matches_tagged_literal() {
        let schema = TypeExpr::Primitive(TypePrimitive::Bytes(TypePrimitiveBytes {
            bytes: crate::EmptyObject::default(),
        }));
        assert_sugar_and_tagged_equal(schema, json!("AAEC"), json!({"bytes_b64": "AAEC"}));
    }

    #[test]
    fn sugar_hash_literal_matches_tagged_literal() {
        let schema = TypeExpr::Primitive(TypePrimitive::Hash(TypePrimitiveHash {
            hash: crate::EmptyObject::default(),
        }));
        assert_sugar_and_tagged_equal(
            schema,
            json!("sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
            json!({"hash": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}),
        );
    }

    #[test]
    fn sugar_uuid_literal_matches_tagged_literal() {
        let schema = TypeExpr::Primitive(TypePrimitive::Uuid(TypePrimitiveUuid {
            uuid: crate::EmptyObject::default(),
        }));
        assert_sugar_and_tagged_equal(
            schema,
            json!("123e4567-e89b-12d3-a456-426614174000"),
            json!({"uuid": "123e4567-e89b-12d3-a456-426614174000"}),
        );
    }

    #[test]
    fn sugar_time_literal_matches_tagged_literal() {
        let schema = TypeExpr::Primitive(TypePrimitive::Time(TypePrimitiveTime {
            time: crate::EmptyObject::default(),
        }));
        assert_sugar_and_tagged_equal(
            schema,
            json!(1_700_000_000_000_000_000u64),
            json!({"time_ns": 1_700_000_000_000_000_000u64}),
        );
    }

    #[test]
    fn sugar_duration_literal_matches_tagged_literal() {
        let schema = TypeExpr::Primitive(TypePrimitive::Duration(TypePrimitiveDuration {
            duration: crate::EmptyObject::default(),
        }));
        assert_sugar_and_tagged_equal(
            schema,
            json!(-1_000_000i64),
            json!({"duration_ns": -1_000_000}),
        );
    }

    #[test]
    fn sugar_set_dedupes_and_matches_tagged_literal() {
        let schema = TypeExpr::Set(TypeSet {
            set: Box::new(TypeExpr::Primitive(TypePrimitive::Text(
                TypePrimitiveText {
                    text: crate::EmptyObject::default(),
                },
            ))),
        });
        assert_sugar_and_tagged_equal(
            schema,
            json!(["beta", "alpha", "beta"]),
            json!({"set": [{"text": "alpha"}, {"text": "beta"}]}),
        );
    }

    #[test]
    fn sugar_map_object_matches_tagged_literal() {
        let schema = TypeExpr::Map(TypeMap {
            map: TypeMapEntry {
                key: TypeMapKey::Text(TypePrimitiveText {
                    text: crate::EmptyObject::default(),
                }),
                value: Box::new(TypeExpr::Primitive(TypePrimitive::Nat(TypePrimitiveNat {
                    nat: crate::EmptyObject::default(),
                }))),
            },
        });
        assert_sugar_and_tagged_equal(
            schema,
            json!({"b": 2, "a": "1"}),
            json!({
                "map": [
                    {"key": {"text": "a"}, "value": {"nat": 1}},
                    {"key": {"text": "b"}, "value": {"nat": 2}}
                ]
            }),
        );
    }

    #[test]
    fn sugar_map_tuple_form_matches_tagged_literal() {
        let schema = TypeExpr::Map(TypeMap {
            map: TypeMapEntry {
                key: TypeMapKey::Uuid(TypePrimitiveUuid {
                    uuid: crate::EmptyObject::default(),
                }),
                value: Box::new(TypeExpr::Primitive(TypePrimitive::Nat(TypePrimitiveNat {
                    nat: crate::EmptyObject::default(),
                }))),
            },
        });
        assert_sugar_and_tagged_equal(
            schema,
            json!([
                ["123e4567-e89b-12d3-a456-426614174000", 1],
                ["223e4567-e89b-12d3-a456-426614174000", 2]
            ]),
            json!({
                "map": [
                    {
                        "key": {"uuid": "123e4567-e89b-12d3-a456-426614174000"},
                        "value": {"nat": 1}
                    },
                    {
                        "key": {"uuid": "223e4567-e89b-12d3-a456-426614174000"},
                        "value": {"nat": 2}
                    }
                ]
            }),
        );
    }

    #[test]
    fn variant_literal_with_unknown_tag_errors() {
        let schema = TypeExpr::Variant(TypeVariant {
            variant: IndexMap::from([(
                "Ok".into(),
                TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                    text: crate::EmptyObject::default(),
                })),
            )]),
        });
        let err = parse_json_literal(
            &json!({"Err": "oops"}),
            &schema,
            &SchemaIndex::new(HashMap::new()),
        )
        .unwrap_err();
        assert!(
            matches!(err, PlanLiteralError::InvalidJson(message) if message.contains("unknown variant tag"))
        );
    }

    #[test]
    fn map_tuple_form_requires_key_value_pairs() {
        let schema = TypeExpr::Map(TypeMap {
            map: TypeMapEntry {
                key: TypeMapKey::Nat(TypePrimitiveNat {
                    nat: crate::EmptyObject::default(),
                }),
                value: Box::new(TypeExpr::Primitive(TypePrimitive::Nat(TypePrimitiveNat {
                    nat: crate::EmptyObject::default(),
                }))),
            },
        });
        let err = parse_json_literal(&json!([[1]]), &schema, &SchemaIndex::new(HashMap::new()))
            .unwrap_err();
        assert!(
            matches!(err, PlanLiteralError::InvalidJson(message) if message.contains("map literals must be [[key, value]"))
        );
    }
}
