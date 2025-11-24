use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};

use serde_json::json;

use super::assert_json_schema;
use crate::{
    DefModule, DefPlan, EffectKind, ExprOrValue, HashRef, ModuleAbi, ModuleKind, ReducerAbi,
    SchemaRef, TypeExpr, TypeMap, TypeMapEntry, TypeMapKey, TypePrimitive, TypePrimitiveNat,
    TypePrimitiveText, TypePrimitiveUuid, TypeSet,
    builtins::builtin_schemas,
    plan_literals::{PlanLiteralError, SchemaIndex, normalize_plan_literals},
};

fn schema_index() -> SchemaIndex {
    let mut map = HashMap::new();
    for builtin in builtin_schemas() {
        map.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
    }
    map.insert(
        "com.acme/Input@1".into(),
        TypeExpr::Record(crate::TypeRecord {
            record: indexmap::IndexMap::from([(
                "id".into(),
                TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                    text: crate::EmptyObject::default(),
                })),
            )]),
        }),
    );
    map.insert(
        "com.acme/Text@1".into(),
        TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
            text: crate::EmptyObject::default(),
        })),
    );
    map.insert(
        "com.acme/Result@1".into(),
        TypeExpr::Record(crate::TypeRecord {
            record: indexmap::IndexMap::from([(
                "message".into(),
                TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                    text: crate::EmptyObject::default(),
                })),
            )]),
        }),
    );
    map.insert(
        "com.acme/Tags@1".into(),
        TypeExpr::Set(TypeSet {
            set: Box::new(TypeExpr::Primitive(TypePrimitive::Text(
                TypePrimitiveText {
                    text: crate::EmptyObject::default(),
                },
            ))),
        }),
    );
    map.insert(
        "com.acme/UuidCounter@1".into(),
        TypeExpr::Map(TypeMap {
            map: TypeMapEntry {
                key: TypeMapKey::Uuid(TypePrimitiveUuid {
                    uuid: crate::EmptyObject::default(),
                }),
                value: Box::new(TypeExpr::Primitive(TypePrimitive::Nat(TypePrimitiveNat {
                    nat: crate::EmptyObject::default(),
                }))),
            },
        }),
    );
    SchemaIndex::new(map)
}

fn reducer_modules() -> HashMap<String, DefModule> {
    let mut modules = HashMap::new();
    modules.insert(
        "com.acme/Reducer@1".into(),
        DefModule {
            name: "com.acme/Reducer@1".into(),
            module_kind: ModuleKind::Reducer,
            wasm_hash: HashRef::new(
                "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            )
            .unwrap(),
            key_schema: None,
            abi: ModuleAbi {
                reducer: Some(ReducerAbi {
                    state: SchemaRef::new("com.acme/Text@1").unwrap(),
                    event: SchemaRef::new("sys/TimerFired@1").unwrap(),
                    annotations: None,
                    effects_emitted: vec![EffectKind::http_request()],
                    cap_slots: indexmap::IndexMap::new(),
                }),
            },
        },
    );
    modules
}

#[test]
fn normalizes_all_expr_or_value_slots() {
    let plan_json = json!({
        "$kind": "defplan",
        "name": "com.acme/Plan@1",
        "input": "com.acme/Input@1",
        "output": "com.acme/Result@1",
        "locals": { "tmp": "com.acme/Text@1" },
        "steps": [
            {
                "id": "assign",
                "op": "assign",
                "expr": "hello",
                "bind": {"as": "tmp"}
            },
            {
                "id": "emit",
                "op": "emit_effect",
                "kind": "http.request",
                "params": {
                    "method": "GET",
                    "url": "https://example.com",
                    "headers": {},
                    "body_ref": null
                },
                "cap": "cap_http",
                "bind": {"effect_id_as": "req"}
            },
            {
                "id": "raise",
                "op": "raise_event",
                "reducer": "com.acme/Reducer@1",
                "event": {
                    "intent_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                    "reducer": "com.acme/Reducer@1",
                    "effect_kind": "timer.set",
                    "adapter_id": "timer",
                    "status": "ok",
                    "requested": {"deliver_at_ns": 1, "key": "remind"},
                    "receipt": {"delivered_at_ns": 1, "key": "remind"},
                    "cost_cents": 0,
                    "signature": "AA=="
                }
            },
            {
                "id": "end",
                "op": "end",
                "result": {"message": "done"}
            }
        ],
        "edges": [
            {"from": "assign", "to": "emit"},
            {"from": "emit", "to": "raise"},
            {"from": "raise", "to": "end"}
        ],
        "required_caps": ["cap_http"],
        "allowed_effects": ["http.request"]
    });

    assert_json_schema(crate::schemas::DEFPLAN, &plan_json);
    let mut plan: DefPlan = serde_json::from_value(plan_json).expect("plan json");
    normalize_plan_literals(&mut plan, &schema_index(), &reducer_modules()).expect("normalize");

    // assign literal
    if let crate::PlanStepKind::Assign(step) = &plan.steps[0].kind {
        assert!(matches!(step.expr, ExprOrValue::Literal(_)));
    } else {
        panic!("expected assign step");
    }
    if let crate::PlanStepKind::EmitEffect(step) = &plan.steps[1].kind {
        assert!(matches!(step.params, ExprOrValue::Literal(_)));
    } else {
        panic!("expected emit step");
    }
    if let crate::PlanStepKind::RaiseEvent(step) = &plan.steps[2].kind {
        assert!(matches!(step.event, ExprOrValue::Literal(_)));
    }
    if let crate::PlanStepKind::End(step) = &plan.steps[3].kind {
        assert!(matches!(step.result, Some(ExprOrValue::Literal(_))));
    }
}

#[test]
fn literal_without_local_schema_errors() {
    let plan_json = json!({
        "$kind": "defplan",
        "name": "com.acme/Plan@1",
        "input": "com.acme/Input@1",
        "steps": [
            {
                "id": "assign",
                "op": "assign",
                "expr": "hello",
                "bind": {"as": "tmp"}
            }
        ],
        "edges": [],
        "required_caps": [],
        "allowed_effects": []
    });
    assert_json_schema(crate::schemas::DEFPLAN, &plan_json);
    let mut plan: DefPlan = serde_json::from_value(plan_json).expect("plan");
    let err = normalize_plan_literals(&mut plan, &schema_index(), &reducer_modules()).unwrap_err();
    assert!(matches!(
        err,
        crate::plan_literals::PlanLiteralError::MissingSchema { .. }
    ));
}

#[test]
fn end_result_without_output_schema_errors() {
    let plan_json = json!({
        "$kind": "defplan",
        "name": "com.acme/Plan@1",
        "input": "com.acme/Input@1",
        "steps": [
            {
                "id": "end",
                "op": "end",
                "result": {"message": "done"}
            }
        ],
        "edges": [],
        "required_caps": [],
        "allowed_effects": []
    });
    assert_json_schema(crate::schemas::DEFPLAN, &plan_json);
    let mut plan: DefPlan = serde_json::from_value(plan_json).expect("plan");
    let err = normalize_plan_literals(&mut plan, &schema_index(), &reducer_modules()).unwrap_err();
    assert!(matches!(err, PlanLiteralError::MissingSchema { context } if context == "end.result"));
}

#[test]
fn emit_effect_requires_known_params_schema() {
    let schemas = SchemaIndex::new(HashMap::new());
    let plan_json = json!({
        "$kind": "defplan",
        "name": "com.acme/Plan@1",
        "input": "com.acme/Input@1",
        "steps": [
            {
                "id": "emit",
                "op": "emit_effect",
                "kind": "llm.generate",
                "params": {"prompt": "hello"},
                "cap": "cap_llm",
                "bind": {"effect_id_as": "req"}
            }
        ],
        "edges": [],
        "required_caps": ["cap_llm"],
        "allowed_effects": ["llm.generate"]
    });
    assert_json_schema(crate::schemas::DEFPLAN, &plan_json);
    let mut plan: DefPlan = serde_json::from_value(plan_json).expect("plan");
    let err = normalize_plan_literals(&mut plan, &schemas, &reducer_modules()).unwrap_err();
    assert!(
        matches!(err, PlanLiteralError::SchemaNotFound { name } if name == "sys/LlmGenerateParams@1")
    );
}

#[test]
fn set_literals_are_sorted_and_deduped() {
    let plan_json = json!({
        "$kind": "defplan",
        "name": "com.acme/SetPlan@1",
        "input": "com.acme/Input@1",
        "locals": { "tags": "com.acme/Tags@1" },
        "steps": [{
            "id": "assign",
            "op": "assign",
            "expr": ["beta", "alpha", "beta"],
            "bind": {"as": "tags"}
        }],
        "edges": [],
        "required_caps": [],
        "allowed_effects": []
    });
    assert_json_schema(crate::schemas::DEFPLAN, &plan_json);
    let mut plan: DefPlan = serde_json::from_value(plan_json).expect("plan");
    normalize_plan_literals(&mut plan, &schema_index(), &reducer_modules()).expect("normalize");
    let crate::PlanStepKind::Assign(step) = &plan.steps[0].kind else {
        panic!("expected assign step");
    };
    let ExprOrValue::Literal(crate::ValueLiteral::Set(value_set)) = &step.expr else {
        panic!("expected literal set");
    };
    let tags: Vec<&str> = value_set
        .set
        .iter()
        .map(|literal| match literal {
            crate::ValueLiteral::Text(text) => text.text.as_str(),
            other => panic!("unexpected literal {other:?}"),
        })
        .collect();
    assert_eq!(
        tags,
        vec!["beta", "alpha"],
        "set must be deduped and sorted by canonical CBOR order"
    );
}

#[test]
fn map_with_non_text_keys_rejects_object_sugar() {
    let plan_json = json!({
        "$kind": "defplan",
        "name": "com.acme/MapPlan@1",
        "input": "com.acme/Input@1",
        "locals": { "counts": "com.acme/UuidCounter@1" },
        "steps": [{
            "id": "assign",
            "op": "assign",
            "expr": {
                "123e4567-e89b-12d3-a456-426614174000": 1
            },
            "bind": {"as": "counts"}
        }],
        "edges": [],
        "required_caps": [],
        "allowed_effects": []
    });
    assert_json_schema(crate::schemas::DEFPLAN, &plan_json);
    let mut plan: DefPlan = serde_json::from_value(plan_json).expect("plan");
    let err = normalize_plan_literals(&mut plan, &schema_index(), &reducer_modules()).unwrap_err();
    match err {
        PlanLiteralError::InvalidJson(message) => {
            assert!(
                message.contains("map literals must be objects (text keys) or [[key,value]"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected invalid json error, got {other:?}"),
    }
}

#[test]
fn map_literals_with_tuple_syntax_are_sorted_and_deduped() {
    let plan_json = json!({
        "$kind": "defplan",
        "name": "com.acme/MapPlan@2",
        "input": "com.acme/Input@1",
        "locals": { "counts": "com.acme/UuidCounter@1" },
        "steps": [{
            "id": "assign",
            "op": "assign",
            "expr": [
                ["223e4567-e89b-12d3-a456-426614174000", 2],
                ["123e4567-e89b-12d3-a456-426614174000", 1],
                ["123e4567-e89b-12d3-a456-426614174000", 1]
            ],
            "bind": {"as": "counts"}
        }],
        "edges": [],
        "required_caps": [],
        "allowed_effects": []
    });
    assert_json_schema(crate::schemas::DEFPLAN, &plan_json);
    let mut plan: DefPlan = serde_json::from_value(plan_json).expect("plan");
    normalize_plan_literals(&mut plan, &schema_index(), &reducer_modules()).expect("normalize");
    let crate::PlanStepKind::Assign(step) = &plan.steps[0].kind else {
        panic!("expected assign step");
    };
    let ExprOrValue::Literal(crate::ValueLiteral::Map(map)) = &step.expr else {
        panic!("expected literal map");
    };
    let keys: Vec<String> = map
        .map
        .iter()
        .map(|entry| match &entry.key {
            crate::ValueLiteral::Uuid(uuid) => uuid.uuid.clone(),
            other => panic!("unexpected key literal {other:?}"),
        })
        .collect();
    assert_eq!(
        keys,
        vec![
            "123e4567-e89b-12d3-a456-426614174000".to_string(),
            "223e4567-e89b-12d3-a456-426614174000".to_string()
        ],
        "map entries should be sorted lexicographically and deduped"
    );
    assert_eq!(map.map.len(), 2, "duplicate keys must be collapsed");
}

#[test]
fn expr_or_value_accepts_full_expr_trees() {
    let plan_json = json!({
        "$kind": "defplan",
        "name": "com.acme/ExprPlan@1",
        "input": "sys/HttpRequestParams@1",
        "output": "com.acme/Text@1",
        "locals": { "tmp": "com.acme/Text@1" },
        "steps": [
            {
                "id": "assign",
                "op": "assign",
                "expr": {
                    "op": "concat",
                    "args": [
                        {"ref": "@plan.input.url"},
                        {"text": "?debug=true"}
                    ]
                },
                "bind": {"as": "tmp"}
            },
            {
                "id": "emit",
                "op": "emit_effect",
                "kind": "http.request",
                "params": {"ref": "@plan.input"},
                "cap": "cap_http",
                "bind": {"effect_id_as": "req"}
            },
            {
                "id": "end",
                "op": "end",
                "result": {"ref": "@var:tmp"}
            }
        ],
        "edges": [
            {"from": "assign", "to": "emit"},
            {"from": "emit", "to": "end"}
        ],
        "required_caps": ["cap_http"],
        "allowed_effects": ["http.request"]
    });
    assert_json_schema(crate::schemas::DEFPLAN, &plan_json);
    let mut plan: DefPlan = serde_json::from_value(plan_json).expect("plan");
    normalize_plan_literals(&mut plan, &schema_index(), &reducer_modules()).expect("normalize");
    let crate::PlanStepKind::Assign(assign) = &plan.steps[0].kind else {
        panic!("expected assign step");
    };
    assert!(matches!(assign.expr, ExprOrValue::Expr(_)));
    let crate::PlanStepKind::EmitEffect(emit) = &plan.steps[1].kind else {
        panic!("expected emit step");
    };
    assert!(matches!(emit.params, ExprOrValue::Expr(_)));
    let crate::PlanStepKind::End(end) = &plan.steps[2].kind else {
        panic!("expected end step");
    };
    assert!(matches!(end.result, Some(ExprOrValue::Expr(_))));
}

#[test]
fn plan_schema_accepts_all_step_kinds() {
    let plan_json = json!({
        "$kind": "defplan",
        "name": "com.acme/Plan@2",
        "input": "com.acme/Input@1",
        "output": "com.acme/Result@1",
        "locals": { "tmp": "com.acme/Text@1" },
        "steps": [
            { "id": "assign", "op": "assign", "expr": "hi", "bind": {"as": "tmp"} },
            {
                "id": "emit",
                "op": "emit_effect",
                "kind": "http.request",
                "params": {
                    "method": "POST",
                    "url": "https://example.com/notify",
                    "headers": {"x-test": "true"}
                },
                "cap": "cap_http",
                "bind": {"effect_id_as": "req"}
            },
            {
                "id": "await_receipt",
                "op": "await_receipt",
                "for": {"ref": "@step:emit"},
                "bind": {"as": "receipt"}
            },
            {
                "id": "await_event",
                "op": "await_event",
                "event": "sys/TimerFired@1",
                "where": {"ref": "@plan.input.id"},
                "bind": {"as": "fired"}
            },
            {
                "id": "raise",
                "op": "raise_event",
                "reducer": "com.acme/Reducer@1",
                "event": {"status": "ok"},
                "key": {"ref": "@plan.input.id"}
            },
            { "id": "end", "op": "end" }
        ],
        "edges": [
            {"from": "assign", "to": "emit"},
            {"from": "emit", "to": "await_receipt"},
            {"from": "await_receipt", "to": "await_event", "when": {"ref": "@var:receipt"}},
            {"from": "await_event", "to": "raise"},
            {"from": "raise", "to": "end"}
        ],
        "required_caps": ["cap_http"],
        "allowed_effects": ["http.request"],
        "invariants": [{"ref": "@plan.input.id"}]
    });
    assert_json_schema(crate::schemas::DEFPLAN, &plan_json);
    let plan: DefPlan = serde_json::from_value(plan_json).expect("plan json");
    assert_eq!(plan.steps.len(), 6);
    assert_eq!(plan.edges.len(), 5);
}

#[test]
fn await_event_without_bind_is_schema_error() {
    let plan_json = json!({
        "$kind": "defplan",
        "name": "com.acme/Plan@bad",
        "input": "com.acme/Input@1",
        "steps": [
            {
                "id": "await_event",
                "op": "await_event",
                "event": "sys/TimerFired@1"
            }
        ]
    });
    assert!(
        panic::catch_unwind(AssertUnwindSafe(|| assert_json_schema(
            crate::schemas::DEFPLAN,
            &plan_json
        )))
        .is_err()
    );
}
