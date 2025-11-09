use std::collections::HashSet;

use thiserror::Error;

use crate::{
    TypeExpr, TypeList, TypeMap, TypeMapKey, TypeOption, TypePrimitive, TypeRecord, TypeSet,
    TypeVariant, ValueLiteral, ValueMapEntry, ValueVariant,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValueTypeError {
    #[error("expected {expected}, found {found}")]
    TypeMismatch {
        expected: &'static str,
        found: &'static str,
    },
    #[error("missing record field '{field}'")]
    MissingField { field: String },
    #[error("unexpected record field '{field}'")]
    UnexpectedField { field: String },
    #[error("variant tag '{tag}' not defined")]
    UnknownVariant { tag: String },
    #[error("variant '{tag}' requires a value")]
    VariantMissingValue { tag: String },
    #[error("map key must be {expected}")]
    InvalidMapKey { expected: &'static str },
    #[error("type references are not supported without a schema resolver: {reference}")]
    UnsupportedTypeRef { reference: String },
}

pub fn validate_value_literal(
    value: &ValueLiteral,
    schema: &TypeExpr,
) -> Result<(), ValueTypeError> {
    match schema {
        TypeExpr::Primitive(p) => validate_primitive(value, p),
        TypeExpr::Record(r) => validate_record(value, r),
        TypeExpr::Variant(v) => validate_variant(value, v),
        TypeExpr::List(list) => validate_list(value, list),
        TypeExpr::Set(set) => validate_set(value, set),
        TypeExpr::Map(map) => validate_map(value, map),
        TypeExpr::Option(opt) => validate_option(value, opt),
        TypeExpr::Ref(r) => Err(ValueTypeError::UnsupportedTypeRef {
            reference: r.reference.to_string(),
        }),
    }
}

fn validate_primitive(
    value: &ValueLiteral,
    primitive: &TypePrimitive,
) -> Result<(), ValueTypeError> {
    let matches = match primitive {
        TypePrimitive::Bool(_) => matches!(value, ValueLiteral::Bool(_)),
        TypePrimitive::Int(_) => matches!(value, ValueLiteral::Int(_)),
        TypePrimitive::Nat(_) => matches!(value, ValueLiteral::Nat(_)),
        TypePrimitive::Dec128(_) => matches!(value, ValueLiteral::Dec128(_)),
        TypePrimitive::Bytes(_) => matches!(value, ValueLiteral::Bytes(_)),
        TypePrimitive::Text(_) => matches!(value, ValueLiteral::Text(_)),
        TypePrimitive::Time(_) => matches!(value, ValueLiteral::TimeNs(_)),
        TypePrimitive::Duration(_) => matches!(value, ValueLiteral::DurationNs(_)),
        TypePrimitive::Hash(_) => matches!(value, ValueLiteral::Hash(_)),
        TypePrimitive::Uuid(_) => matches!(value, ValueLiteral::Uuid(_)),
        TypePrimitive::Unit(_) => {
            matches!(value, ValueLiteral::Record(rec) if rec.record.is_empty())
                || matches!(value, ValueLiteral::Null(_))
        }
    };
    if matches {
        Ok(())
    } else {
        Err(ValueTypeError::TypeMismatch {
            expected: primitive_name(primitive),
            found: value_kind(value),
        })
    }
}

fn validate_record(value: &ValueLiteral, record: &TypeRecord) -> Result<(), ValueTypeError> {
    let fields = match value {
        ValueLiteral::Record(record_value) => &record_value.record,
        other => {
            return Err(ValueTypeError::TypeMismatch {
                expected: "record",
                found: value_kind(other),
            });
        }
    };

    let mut seen = HashSet::new();
    for (name, field_schema) in record.record.iter() {
        if let Some(field_value) = fields.get(name) {
            validate_value_literal(field_value, field_schema)?;
            seen.insert(name);
        } else if !is_optional_type(field_schema) {
            return Err(ValueTypeError::MissingField {
                field: name.clone(),
            });
        }
    }

    for name in fields.keys() {
        if !seen.contains(name) && !record.record.contains_key(name) {
            return Err(ValueTypeError::UnexpectedField {
                field: name.clone(),
            });
        }
    }

    Ok(())
}

fn validate_variant(value: &ValueLiteral, variant: &TypeVariant) -> Result<(), ValueTypeError> {
    let ValueLiteral::Variant(ValueVariant { tag, value }) = value else {
        return Err(ValueTypeError::TypeMismatch {
            expected: "variant",
            found: value_kind(value),
        });
    };
    let Some(expected_type) = variant.variant.get(tag) else {
        return Err(ValueTypeError::UnknownVariant { tag: tag.clone() });
    };
    match value {
        Some(inner) => validate_value_literal(inner, expected_type),
        None => {
            if is_unit_type(expected_type) {
                Ok(())
            } else {
                Err(ValueTypeError::VariantMissingValue { tag: tag.clone() })
            }
        }
    }
}

fn validate_list(value: &ValueLiteral, schema: &TypeList) -> Result<(), ValueTypeError> {
    let ValueLiteral::List(list_value) = value else {
        return Err(ValueTypeError::TypeMismatch {
            expected: "list",
            found: value_kind(value),
        });
    };
    for item in &list_value.list {
        validate_value_literal(item, &schema.list)?;
    }
    Ok(())
}

fn validate_set(value: &ValueLiteral, schema: &TypeSet) -> Result<(), ValueTypeError> {
    let ValueLiteral::Set(set_value) = value else {
        return Err(ValueTypeError::TypeMismatch {
            expected: "set",
            found: value_kind(value),
        });
    };
    for item in &set_value.set {
        validate_value_literal(item, &schema.set)?;
    }
    Ok(())
}

fn validate_map(value: &ValueLiteral, map_type: &TypeMap) -> Result<(), ValueTypeError> {
    let ValueLiteral::Map(map_value) = value else {
        return Err(ValueTypeError::TypeMismatch {
            expected: "map",
            found: value_kind(value),
        });
    };
    for ValueMapEntry { key, value } in &map_value.map {
        validate_map_key(key, &map_type.map.key)?;
        validate_value_literal(value, &map_type.map.value)?;
    }
    Ok(())
}

fn validate_option(value: &ValueLiteral, opt: &TypeOption) -> Result<(), ValueTypeError> {
    if matches!(value, ValueLiteral::Null(_)) {
        Ok(())
    } else {
        validate_value_literal(value, &opt.option)
    }
}

fn validate_map_key(value: &ValueLiteral, key: &TypeMapKey) -> Result<(), ValueTypeError> {
    let matches = match key {
        TypeMapKey::Int(_) => matches!(value, ValueLiteral::Int(_)),
        TypeMapKey::Nat(_) => matches!(value, ValueLiteral::Nat(_)),
        TypeMapKey::Text(_) => matches!(value, ValueLiteral::Text(_)),
        TypeMapKey::Uuid(_) => matches!(value, ValueLiteral::Uuid(_)),
        TypeMapKey::Hash(_) => matches!(value, ValueLiteral::Hash(_)),
    };
    if matches {
        Ok(())
    } else {
        Err(ValueTypeError::InvalidMapKey {
            expected: map_key_name(key),
        })
    }
}

fn is_optional_type(schema: &TypeExpr) -> bool {
    matches!(schema, TypeExpr::Option(_))
}

fn is_unit_type(schema: &TypeExpr) -> bool {
    matches!(schema, TypeExpr::Primitive(TypePrimitive::Unit(_)))
}

fn primitive_name(p: &TypePrimitive) -> &'static str {
    match p {
        TypePrimitive::Bool(_) => "bool",
        TypePrimitive::Int(_) => "int",
        TypePrimitive::Nat(_) => "nat",
        TypePrimitive::Dec128(_) => "dec128",
        TypePrimitive::Bytes(_) => "bytes",
        TypePrimitive::Text(_) => "text",
        TypePrimitive::Time(_) => "time",
        TypePrimitive::Duration(_) => "duration",
        TypePrimitive::Hash(_) => "hash",
        TypePrimitive::Uuid(_) => "uuid",
        TypePrimitive::Unit(_) => "unit",
    }
}

fn map_key_name(key: &TypeMapKey) -> &'static str {
    match key {
        TypeMapKey::Int(_) => "int",
        TypeMapKey::Nat(_) => "nat",
        TypeMapKey::Text(_) => "text",
        TypeMapKey::Uuid(_) => "uuid",
        TypeMapKey::Hash(_) => "hash",
    }
}

fn value_kind(value: &ValueLiteral) -> &'static str {
    match value {
        ValueLiteral::Null(_) => "null",
        ValueLiteral::Bool(_) => "bool",
        ValueLiteral::Int(_) => "int",
        ValueLiteral::Nat(_) => "nat",
        ValueLiteral::Dec128(_) => "dec128",
        ValueLiteral::Bytes(_) => "bytes",
        ValueLiteral::Text(_) => "text",
        ValueLiteral::TimeNs(_) => "time",
        ValueLiteral::DurationNs(_) => "duration",
        ValueLiteral::Hash(_) => "hash",
        ValueLiteral::Uuid(_) => "uuid",
        ValueLiteral::List(_) => "list",
        ValueLiteral::Set(_) => "set",
        ValueLiteral::Map(_) => "map",
        ValueLiteral::Record(_) => "record",
        ValueLiteral::Variant(_) => "variant",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TypeList, TypeOption, TypeRecord, TypeSet, ValueList, ValueRecord, ValueSet};
    use indexmap::IndexMap;

    fn text_literal(text: &str) -> ValueLiteral {
        ValueLiteral::Text(crate::ValueText { text: text.into() })
    }

    #[test]
    fn record_missing_field_is_error() {
        let schema = TypeExpr::Record(TypeRecord {
            record: IndexMap::from([(
                "hosts".into(),
                TypeExpr::Set(TypeSet {
                    set: Box::new(TypeExpr::Primitive(TypePrimitive::Text(
                        crate::TypePrimitiveText {
                            text: crate::EmptyObject {},
                        },
                    ))),
                }),
            )]),
        });
        let value = ValueLiteral::Record(ValueRecord {
            record: IndexMap::new(),
        });
        let err = validate_value_literal(&value, &schema).unwrap_err();
        assert!(matches!(err, ValueTypeError::MissingField { field } if field == "hosts"));
    }

    #[test]
    fn option_allows_null() {
        let schema = TypeExpr::Option(TypeOption {
            option: Box::new(TypeExpr::Primitive(TypePrimitive::Text(
                crate::TypePrimitiveText {
                    text: crate::EmptyObject {},
                },
            ))),
        });
        let value = ValueLiteral::Null(crate::ValueNull {
            null: crate::EmptyObject {},
        });
        assert!(validate_value_literal(&value, &schema).is_ok());
    }

    #[test]
    fn list_items_checked() {
        let schema = TypeExpr::List(TypeList {
            list: Box::new(TypeExpr::Primitive(TypePrimitive::Text(
                crate::TypePrimitiveText {
                    text: crate::EmptyObject {},
                },
            ))),
        });
        let value = ValueLiteral::List(ValueList {
            list: vec![text_literal("a"), text_literal("b")],
        });
        assert!(validate_value_literal(&value, &schema).is_ok());
    }

    #[test]
    fn set_item_type_mismatch() {
        let schema = TypeExpr::Set(TypeSet {
            set: Box::new(TypeExpr::Primitive(TypePrimitive::Text(
                crate::TypePrimitiveText {
                    text: crate::EmptyObject {},
                },
            ))),
        });
        let value = ValueLiteral::Set(ValueSet {
            set: vec![ValueLiteral::Nat(crate::ValueNat { nat: 1 })],
        });
        let err = validate_value_literal(&value, &schema).unwrap_err();
        assert!(matches!(err, ValueTypeError::TypeMismatch { .. }));
    }
}
