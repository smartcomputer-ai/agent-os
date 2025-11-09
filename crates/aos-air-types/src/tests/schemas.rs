use serde_json::json;

use crate::{
    DefSchema, EmptyObject, TypeExpr, TypePrimitive, TypePrimitiveText, ValueLiteral, ValueRecord,
    ValueText, validate_value_literal,
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
