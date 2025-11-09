use serde_json::json;

use crate::{
    CapGrant, DefCap, TypeExpr, TypePrimitive, TypePrimitiveText, TypeRecord, TypeSet,
    validate_value_literal,
};

fn cap_schema() -> TypeExpr {
    TypeExpr::Record(TypeRecord {
        record: indexmap::IndexMap::from([(
            "hosts".into(),
            TypeExpr::Set(TypeSet {
                set: Box::new(TypeExpr::Primitive(TypePrimitive::Text(
                    TypePrimitiveText {
                        text: crate::EmptyObject::default(),
                    },
                ))),
            }),
        )]),
    })
}

#[test]
fn parses_cap_definition_and_grant() {
    let cap_json = json!({
        "$kind": "defcap",
        "name": "com.acme/http@1",
        "cap_type": "http.out",
        "schema": {
            "record": {
                "hosts": {"set": {"text": {}}}
            }
        }
    });
    let cap: DefCap = serde_json::from_value(cap_json).expect("cap json");
    assert_eq!(cap.cap_type, crate::CapType::HttpOut);

    let grant_json = json!({
        "name": "cap_http",
        "cap": "com.acme/http@1",
        "params": {
            "record": {
                "hosts": {"set": [{"text": "example.com"}]}
            }
        }
    });
    let grant: CapGrant = serde_json::from_value(grant_json).expect("grant json");
    validate_value_literal(&grant.params, &cap.schema).expect("grant params match schema");
}

#[test]
fn rejects_grant_with_wrong_shape() {
    let grant_json = json!({
        "name": "cap_http",
        "cap": "com.acme/http@1",
        "params": {"text": "not a record"}
    });
    let grant: CapGrant = serde_json::from_value(grant_json).expect("grant json");
    assert!(validate_value_literal(&grant.params, &cap_schema()).is_err());
}
