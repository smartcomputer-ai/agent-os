use serde_json::json;

use super::assert_json_schema;
use crate::{
    DefSchema, EmptyObject, TypeExpr, TypeList, TypeMap, TypeMapEntry, TypeMapKey, TypeOption,
    TypePrimitive, TypePrimitiveBool, TypePrimitiveBytes, TypePrimitiveInt,
    TypePrimitiveNat, TypePrimitiveText, TypePrimitiveUuid, TypeRecord, TypeRef, TypeSet,
    TypeVariant, ValueLiteral, ValueList, ValueMap, ValueMapEntry as ValueMapEntryLiteral,
    ValueRecord, ValueSet, ValueText, ValueVariant, ValueInt, ValueNat, validate_value_literal,
};

#[test]
fn parses_record_schema_and_validates_literal() {
    let schema_json = json!({
        "$kind": "defschema",
        "name": "com.acme/Message@1",
        "type": {
            "record": {
                "id": { "text": {} },
                "body": { "text": {} }
            }
        }
    });
    assert_json_schema(crate::schemas::DEFSCHEMA, &schema_json);
    let def: DefSchema = serde_json::from_value(schema_json).expect("parse defschema");
    assert_eq!(def.name, "com.acme/Message@1");

    let literal = ValueLiteral::Record(ValueRecord {
        record: [
            (
                "id".to_string(),
                ValueLiteral::Text(ValueText { text: "42".into() }),
            ),
            (
                "body".to_string(),
                ValueLiteral::Text(ValueText {
                    text: "hello".into(),
                }),
            ),
        ]
        .into_iter()
        .collect(),
    });

    validate_value_literal(&literal, &def.ty).expect("literal matches schema");
}

#[test]
fn rejects_invalid_map_key_type() {
    let schema_json = json!({
        "$kind": "defschema",
        "name": "com.acme/BadMap@1",
        "type": {
            "map": {
                "value": { "text": {} }
            }
        }
    });
    assert!(serde_json::from_value::<DefSchema>(schema_json).is_err());
}

#[test]
fn type_expr_round_trips_from_struct() {
    let mut record = indexmap::IndexMap::new();
    record.insert(
        "name".into(),
        TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
            text: EmptyObject::default(),
        })),
    );
    let schema = TypeExpr::Record(crate::TypeRecord { record });
    let json = serde_json::to_value(&schema).expect("serialize");
    let round_trip: TypeExpr = serde_json::from_value(json).expect("deserialize");
    assert!(matches!(round_trip, TypeExpr::Record(_)));
}

#[test]
fn accepts_all_type_expr_variants() {
    let schema_json = json!({
        "$kind": "defschema",
        "name": "com.acme/Everything@1",
        "type": {
            "record": {
                "primitive": { "bool": {} },
                "list": { "list": { "int": {} } },
                "set": { "set": { "uuid": {} } },
                "map": {
                    "map": {
                        "key": { "text": {} },
                        "value": { "hash": {} }
                    }
                },
                "option": { "option": { "bytes": {} } },
                "variant": {
                    "variant": {
                        "left": { "text": {} },
                        "right": { "ref": "com.acme/Other@1" }
                    }
                },
                "ref": { "ref": "com.acme/Other@1" }
            }
        }
    });

    assert_json_schema(crate::schemas::DEFSCHEMA, &schema_json);
    let def: DefSchema = serde_json::from_value(schema_json).expect("parse schema");
    let TypeExpr::Record(record) = def.ty else {
        panic!("expected record type");
    };
    assert_eq!(record.record.len(), 7, "all fields retained");
}

#[test]
fn validates_variant_literal_against_schema() {
    let schema_json = json!({
        "$kind": "defschema",
        "name": "com.acme/MaybeText@1",
        "type": {
            "variant": {
                "Some": { "text": {} },
                "None": { "unit": {} }
            }
        }
    });
    assert_json_schema(crate::schemas::DEFSCHEMA, &schema_json);
    let def: DefSchema = serde_json::from_value(schema_json).expect("parse schema");

    let literal = ValueLiteral::Variant(ValueVariant {
        tag: "Some".into(),
        value: Some(Box::new(ValueLiteral::Text(ValueText {
            text: "hi".into(),
        }))),
    });
    validate_value_literal(&literal, &def.ty).expect("variant literal valid");

    let bad_literal = ValueLiteral::Variant(ValueVariant {
        tag: "Other".into(),
        value: None,
    });
    assert!(validate_value_literal(&bad_literal, &def.ty).is_err());
}

#[test]
fn type_expr_struct_constructors_cover_composites() {
    let mut record_fields = indexmap::IndexMap::new();
    record_fields.insert(
        "list".into(),
        TypeExpr::List(TypeList {
            list: Box::new(TypeExpr::Primitive(TypePrimitive::Nat(TypePrimitiveNat {
                nat: EmptyObject::default(),
            }))),
        }),
    );
    record_fields.insert(
        "set".into(),
        TypeExpr::Set(TypeSet {
            set: Box::new(TypeExpr::Primitive(TypePrimitive::Int(TypePrimitiveInt {
                int: EmptyObject::default(),
            }))),
        }),
    );
    record_fields.insert(
        "map".into(),
        TypeExpr::Map(TypeMap {
            map: TypeMapEntry {
                key: TypeMapKey::Text(TypePrimitiveText {
                    text: EmptyObject::default(),
                }),
                value: Box::new(TypeExpr::Primitive(TypePrimitive::Bool(
                    TypePrimitiveBool {
                        bool: EmptyObject::default(),
                    },
                ))),
            },
        }),
    );
    record_fields.insert(
        "option".into(),
        TypeExpr::Option(TypeOption {
            option: Box::new(TypeExpr::Primitive(TypePrimitive::Bytes(
                TypePrimitiveBytes {
                    bytes: EmptyObject::default(),
                },
            ))),
        }),
    );
    record_fields.insert(
        "ref".into(),
        TypeExpr::Ref(TypeRef {
            reference: "com.acme/Other@1".parse().unwrap(),
        }),
    );
    record_fields.insert(
        "variant".into(),
        TypeExpr::Variant(TypeVariant {
            variant: indexmap::IndexMap::from([
                (
                    "left".into(),
                    TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                        text: EmptyObject::default(),
                    })),
                ),
                (
                    "right".into(),
                    TypeExpr::Primitive(TypePrimitive::Uuid(TypePrimitiveUuid {
                        uuid: EmptyObject::default(),
                    })),
                ),
            ]),
        }),
    );

    let schema = TypeExpr::Record(TypeRecord {
        record: record_fields,
    });
    let schema_payload = serde_json::to_value(&schema).expect("serialize");
    let def_json = json!({
        "$kind": "defschema",
        "name": "com.acme/Composite@1",
        "type": schema_payload
    });
    assert_json_schema(crate::schemas::DEFSCHEMA, &def_json);
    let def: DefSchema = serde_json::from_value(def_json).expect("parse defschema");
    assert!(matches!(def.ty, TypeExpr::Record(_)));
}

#[test]
fn map_literal_must_match_key_type() {
    let schema_json = json!({
        "$kind": "defschema",
        "name": "com.acme/HostMap@1",
        "type": {
            "map": {
                "key": { "text": {} },
                "value": { "nat": {} }
            }
        }
    });
    assert_json_schema(crate::schemas::DEFSCHEMA, &schema_json);
    let def: DefSchema = serde_json::from_value(schema_json).expect("parse schema");

    let literal = ValueLiteral::Map(ValueMap {
        map: vec![ValueMapEntryLiteral {
            key: ValueLiteral::Text(ValueText { text: "prod".into() }),
            value: ValueLiteral::Nat(ValueNat { nat: 1 }),
        }],
    });
    validate_value_literal(&literal, &def.ty).expect("map literal valid");

    let bad_literal = ValueLiteral::Map(ValueMap {
        map: vec![ValueMapEntryLiteral {
            key: ValueLiteral::Nat(ValueNat { nat: 0 }),
            value: ValueLiteral::Nat(ValueNat { nat: 1 }),
        }],
    });
    assert!(validate_value_literal(&bad_literal, &def.ty).is_err());
}

#[test]
fn set_literal_respects_element_shape() {
    let schema_json = json!({
        "$kind": "defschema",
        "name": "com.acme/HostSet@1",
        "type": {
            "set": { "text": {} }
        }
    });
    assert_json_schema(crate::schemas::DEFSCHEMA, &schema_json);
    let def: DefSchema = serde_json::from_value(schema_json).expect("parse schema");

    let literal = ValueLiteral::Set(ValueSet {
        set: vec![
            ValueLiteral::Text(ValueText { text: "a".into() }),
            ValueLiteral::Text(ValueText { text: "b".into() }),
        ],
    });
    validate_value_literal(&literal, &def.ty).expect("set literal valid");

    let bad_literal = ValueLiteral::Set(ValueSet {
        set: vec![ValueLiteral::Nat(ValueNat { nat: 1 })],
    });
    assert!(validate_value_literal(&bad_literal, &def.ty).is_err());
}

#[test]
fn list_literal_respects_element_shape() {
    let schema_json = json!({
        "$kind": "defschema",
        "name": "com.acme/Numbers@1",
        "type": {
            "list": { "int": {} }
        }
    });
    assert_json_schema(crate::schemas::DEFSCHEMA, &schema_json);
    let def: DefSchema = serde_json::from_value(schema_json).expect("parse schema");

    let literal = ValueLiteral::List(ValueList {
        list: vec![
            ValueLiteral::Int(ValueInt { int: -1 }),
            ValueLiteral::Int(ValueInt { int: 2 }),
        ],
    });
    validate_value_literal(&literal, &def.ty).expect("list literal valid");

    let bad_literal = ValueLiteral::List(ValueList {
        list: vec![ValueLiteral::Text(ValueText { text: "oops".into() })],
    });
    assert!(validate_value_literal(&bad_literal, &def.ty).is_err());
}
