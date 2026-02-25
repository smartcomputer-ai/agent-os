use std::collections::HashMap;

use aos_cbor::to_canonical_cbor;
use thiserror::Error;

use crate::{
    TypeExpr, TypeMapKey, TypePrimitive, ValueLiteral, ValueMapEntry, typecheck::validate_value_literal,
};

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
    #[error("invalid literal for schema {schema}: {message}")]
    InvalidLiteral { schema: String, message: String },
    #[error("invalid literal encoding: {0}")]
    InvalidJson(String),
}

// Transitional no-op normalizers retained for legacy plan runtime call sites.
pub fn normalize_plan_literals(
    _plan: &mut crate::DefPlan,
    _schemas: &SchemaIndex,
    _effects: &crate::catalog::EffectCatalog,
) -> Result<(), PlanLiteralError> {
    Ok(())
}

pub fn normalize_plan_literals_with_plan_inputs(
    _plan: &mut crate::DefPlan,
    _schemas: &SchemaIndex,
    _effects: &crate::catalog::EffectCatalog,
    _plan_input_schemas: &HashMap<String, String>,
) -> Result<(), PlanLiteralError> {
    Ok(())
}

pub fn validate_literal(
    literal: &ValueLiteral,
    schema: &TypeExpr,
    schema_name: &str,
    schemas: &SchemaIndex,
) -> Result<(), PlanLiteralError> {
    let expanded = expand_schema(schema, schemas)?;
    validate_value_literal(literal, &expanded).map_err(|err| PlanLiteralError::InvalidLiteral {
        schema: schema_name.to_string(),
        message: err.to_string(),
    })
}

pub fn canonicalize_literal(
    literal: &mut ValueLiteral,
    schema: &TypeExpr,
    schemas: &SchemaIndex,
) -> Result<(), PlanLiteralError> {
    match resolve_type(schema, schemas)? {
        TypeExpr::Record(record) => {
            if let ValueLiteral::Record(value_record) = literal {
                for (field_name, field_type) in &record.record {
                    if let Some(field_value) = value_record.record.get_mut(field_name) {
                        canonicalize_literal(field_value, field_type, schemas)?;
                    }
                }
            }
        }
        TypeExpr::Variant(variant) => {
            if let ValueLiteral::Variant(value_variant) = literal
                && let Some(value) = value_variant.value.as_mut()
                && let Some(inner_schema) = variant.variant.get(value_variant.tag.as_str())
            {
                canonicalize_literal(value, inner_schema, schemas)?;
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
                let key_schema = match &map_type.map.key {
                    TypeMapKey::Int(v) => TypeExpr::Primitive(TypePrimitive::Int(v.clone())),
                    TypeMapKey::Nat(v) => TypeExpr::Primitive(TypePrimitive::Nat(v.clone())),
                    TypeMapKey::Text(v) => TypeExpr::Primitive(TypePrimitive::Text(v.clone())),
                    TypeMapKey::Uuid(v) => TypeExpr::Primitive(TypePrimitive::Uuid(v.clone())),
                    TypeMapKey::Hash(v) => TypeExpr::Primitive(TypePrimitive::Hash(v.clone())),
                };
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
        TypeExpr::Primitive(_) => {}
    }
    Ok(())
}

fn resolve_type(schema: &TypeExpr, schemas: &SchemaIndex) -> Result<TypeExpr, PlanLiteralError> {
    match schema {
        TypeExpr::Ref(reference) => schemas
            .get(reference.reference.as_str())
            .cloned()
            .ok_or_else(|| PlanLiteralError::SchemaNotFound {
                name: reference.reference.as_str().to_string(),
            }),
        _ => Ok(schema.clone()),
    }
}

fn expand_schema(schema: &TypeExpr, schemas: &SchemaIndex) -> Result<TypeExpr, PlanLiteralError> {
    match schema {
        TypeExpr::Ref(reference) => {
            let inner = schemas.get(reference.reference.as_str()).ok_or_else(|| {
                PlanLiteralError::SchemaNotFound {
                    name: reference.reference.as_str().to_string(),
                }
            })?;
            expand_schema(inner, schemas)
        }
        TypeExpr::Record(record) => {
            let mut expanded = indexmap::IndexMap::new();
            for (k, v) in &record.record {
                expanded.insert(k.clone(), expand_schema(v, schemas)?);
            }
            Ok(TypeExpr::Record(crate::TypeRecord { record: expanded }))
        }
        TypeExpr::Variant(variant) => {
            let mut expanded = indexmap::IndexMap::new();
            for (k, v) in &variant.variant {
                expanded.insert(k.clone(), expand_schema(v, schemas)?);
            }
            Ok(TypeExpr::Variant(crate::TypeVariant { variant: expanded }))
        }
        TypeExpr::List(list) => Ok(TypeExpr::List(crate::TypeList {
            list: Box::new(expand_schema(&list.list, schemas)?),
        })),
        TypeExpr::Set(set) => Ok(TypeExpr::Set(crate::TypeSet {
            set: Box::new(expand_schema(&set.set, schemas)?),
        })),
        TypeExpr::Map(map) => {
            let key = map.map.key.clone();
            let value = Box::new(expand_schema(&map.map.value, schemas)?);
            Ok(TypeExpr::Map(crate::TypeMap {
                map: crate::TypeMapEntry { key, value },
            }))
        }
        TypeExpr::Option(opt) => Ok(TypeExpr::Option(crate::TypeOption {
            option: Box::new(expand_schema(&opt.option, schemas)?),
        })),
        primitive => Ok(primitive.clone()),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EmptyObject, TypePrimitive, TypePrimitiveText, ValueRecord, ValueSet, ValueText,
    };
    use indexmap::IndexMap;

    #[test]
    fn canonicalize_set_is_sorted_and_deduped() {
        let schema = TypeExpr::Set(crate::TypeSet {
            set: Box::new(TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                text: EmptyObject::default(),
            }))),
        });
        let mut literal = ValueLiteral::Set(ValueSet {
            set: vec![
                ValueLiteral::Text(ValueText { text: "b".into() }),
                ValueLiteral::Text(ValueText { text: "a".into() }),
                ValueLiteral::Text(ValueText { text: "a".into() }),
            ],
        });

        canonicalize_literal(&mut literal, &schema, &SchemaIndex::default()).unwrap();

        let ValueLiteral::Set(values) = literal else {
            panic!("expected set");
        };
        assert_eq!(values.set.len(), 2);
    }

    #[test]
    fn validate_literal_expands_schema_refs() {
        let mut index = SchemaIndex::default();
        index.insert(
            "com.acme/Inner@1".into(),
            TypeExpr::Record(crate::TypeRecord {
                record: IndexMap::from([(
                    "name".into(),
                    TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                        text: EmptyObject::default(),
                    })),
                )]),
            }),
        );

        let schema = TypeExpr::Ref(crate::TypeRef {
            reference: crate::SchemaRef::new("com.acme/Inner@1").unwrap(),
        });
        let literal = ValueLiteral::Record(ValueRecord {
            record: IndexMap::from([(
                "name".into(),
                ValueLiteral::Text(ValueText {
                    text: "demo".into(),
                }),
            )]),
        });

        validate_literal(&literal, &schema, "com.acme/Inner@1", &index).unwrap();
    }
}
