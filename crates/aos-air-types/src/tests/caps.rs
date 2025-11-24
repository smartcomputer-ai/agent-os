use serde_json::json;

use super::assert_json_schema;
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
    assert_json_schema(crate::schemas::DEFCAP, &cap_json);
    let cap: DefCap = serde_json::from_value(cap_json).expect("cap json");
    assert_eq!(cap.cap_type, crate::CapType::http_out());

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

#[test]
fn supports_all_cap_types() {
    for cap_type in ["http.out", "blob", "timer", "llm.basic", "secret"] {
        let cap_json = json!({
            "$kind": "defcap",
            "name": format!("com.acme/{cap_type}@1"),
            "cap_type": cap_type,
            "schema": {"record": {}}
        });
        assert_json_schema(crate::schemas::DEFCAP, &cap_json);
        let def: DefCap = serde_json::from_value(cap_json).expect("cap json");
        assert_eq!(def.name.as_str(), format!("com.acme/{cap_type}@1"));
    }
}

#[test]
fn accepts_custom_cap_type_strings() {
    let cap_json = json!({
        "$kind": "defcap",
        "name": "com.acme/unknown@1",
        "cap_type": "email.outbound",
        "schema": {"record": {}}
    });
    assert_json_schema(crate::schemas::DEFCAP, &cap_json);
    let def: DefCap = serde_json::from_value(cap_json).expect("cap json");
    assert_eq!(def.cap_type.as_str(), "email.outbound");
}

#[test]
fn cap_grant_may_include_budget_and_expiry() {
    let grant_json = json!({
        "name": "cap_llm",
        "cap": "com.acme/llm@1",
        "params": {"record": {}},
        "expiry_ns": 99,
        "budget": {
            "tokens": 1000,
            "bytes": 2048,
            "cents": 50
        }
    });
    let grant: CapGrant = serde_json::from_value(grant_json).expect("grant json");
    let budget = grant.budget.expect("budget");
    assert_eq!(budget.tokens, Some(1000));
    assert_eq!(budget.bytes, Some(2048));
    assert_eq!(budget.cents, Some(50));
    assert_eq!(grant.expiry_ns, Some(99));
}
