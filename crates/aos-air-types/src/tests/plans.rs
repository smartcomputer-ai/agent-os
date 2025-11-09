use std::collections::HashMap;

use serde_json::json;

use super::assert_json_schema;
use crate::{
    DefModule, DefPlan, EffectKind, ExprOrValue, HashRef, ModuleAbi, ModuleKind, ReducerAbi,
    SchemaRef, TypeExpr, TypePrimitive, TypePrimitiveText,
    builtins::builtin_schemas,
    plan_literals::{SchemaIndex, normalize_plan_literals},
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
                    effects_emitted: vec![EffectKind::HttpRequest],
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
