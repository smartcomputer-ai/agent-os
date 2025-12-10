//! Shared test helpers for integration tests.
//!
//! These helpers build test manifests and utilities used across multiple integration test files.
//! Note: Each integration test compiles this module separately, so some functions may appear
//! unused in certain test contexts but are used by others.

#![allow(dead_code)]

use aos_air_types::{
    DefPlan, DefPolicy, DefSchema, EffectKind, EmptyObject, Expr, ExprConst, ExprOrValue,
    ExprRecord, ManifestDefaults, NamedRef, PlanBind, PlanBindEffect, PlanEdge, PlanStep,
    PlanStepAwaitEvent, PlanStepAwaitReceipt, PlanStepEmitEffect, PlanStepEnd, PlanStepKind,
    PlanStepRaiseEvent, ReducerAbi, TypeExpr, TypePrimitive, TypePrimitiveInt, TypePrimitiveText,
    TypeRecord, ValueLiteral, ValueMap, ValueNull, ValueRecord, ValueText,
};
use aos_effects::builtins::TimerSetParams;
#[path = "../src/fixtures/mod.rs"]
pub mod fixtures;

use aos_wasm_abi::{ReducerEffect, ReducerOutput};
use fixtures::{START_SCHEMA, TestStore, zero_hash};
use indexmap::IndexMap;
use std::sync::Arc;

/// Builds a test manifest with a plan that emits an HTTP effect, awaits its receipt,
/// and raises an event to a result reducer.
pub fn fulfillment_manifest(store: &Arc<TestStore>) -> aos_kernel::manifest::LoadedManifest {
    let mut result_module = fixtures::stub_reducer_module(
        store,
        "com.acme/ResultReducer@1",
        &ReducerOutput {
            state: Some(vec![0xEE]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    let result_state_schema = fixtures::schema("com.acme/ResultState@1");
    let result_event_schema = fixtures::schema("com.acme/ResultEvent@1");
    result_module.abi.reducer = Some(ReducerAbi {
        state: result_state_schema.clone(),
        event: result_event_schema.clone(),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: IndexMap::new(),
    });

    let plan_name = "com.acme/Fulfill@1".to_string();
    let plan = DefPlan {
        name: plan_name.clone(),
        input: fixtures::schema("com.acme/PlanIn@1"),
        output: None,
        locals: IndexMap::new(),
        steps: vec![
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: http_params_literal("https://example.com"),
                    cap: "cap_http".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "await".into(),
                kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                    for_expr: fixtures::var_expr("req"),
                    bind: PlanBind { var: "resp".into() },
                }),
            },
            PlanStep {
                id: "raise".into(),
                kind: PlanStepKind::RaiseEvent(PlanStepRaiseEvent {
                    reducer: result_module.name.clone(),
                    event: Expr::Record(ExprRecord {
                        record: IndexMap::from([(
                            "value".into(),
                            Expr::Const(ExprConst::Int { int: 9 }),
                        )]),
                    })
                    .into(),
                    key: None,
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd { result: None }),
            },
        ],
        edges: vec![
            PlanEdge {
                from: "emit".into(),
                to: "await".into(),
                when: None,
            },
            PlanEdge {
                from: "await".into(),
                to: "raise".into(),
                when: None,
            },
            PlanEdge {
                from: "raise".into(),
                to: "end".into(),
                when: None,
            },
        ],
        required_caps: vec!["cap_http".into()],
        allowed_effects: vec![EffectKind::http_request()],
        invariants: vec![],
    };

    let routing = vec![fixtures::routing_event(
        result_event_schema.as_str(),
        &result_module.name,
    )];
    let mut loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![result_module],
        routing,
    );

    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(fixtures::START_SCHEMA, vec![("id", text_type())]),
            def_text_record_schema("com.acme/PlanIn@1", vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/ResultEvent@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::from([("value".into(), int_type())]),
                }),
            },
            DefSchema {
                name: "com.acme/ResultState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );

    loaded
}

fn http_params_literal(url: &str) -> ExprOrValue {
    ExprOrValue::Literal(ValueLiteral::Record(ValueRecord {
        record: indexmap::IndexMap::from([
            (
                "method".into(),
                ValueLiteral::Text(ValueText { text: "GET".into() }),
            ),
            (
                "url".into(),
                ValueLiteral::Text(ValueText { text: url.into() }),
            ),
            (
                "headers".into(),
                ValueLiteral::Map(ValueMap { map: vec![] }),
            ),
            (
                "body_ref".into(),
                ValueLiteral::Null(ValueNull {
                    null: EmptyObject::default(),
                }),
            ),
        ]),
    }))
}

/// Builds a test manifest with a plan that awaits a domain event before proceeding.
pub fn await_event_manifest(store: &Arc<TestStore>) -> aos_kernel::manifest::LoadedManifest {
    let mut result_module = fixtures::stub_reducer_module(
        store,
        "com.acme/EventResult@1",
        &ReducerOutput {
            state: Some(vec![0xAB]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    let result_state_schema = fixtures::schema("com.acme/EventResultState@1");
    let result_event_schema = fixtures::schema("com.acme/EventDone@1");
    result_module.abi.reducer = Some(ReducerAbi {
        state: result_state_schema.clone(),
        event: result_event_schema.clone(),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: IndexMap::new(),
    });
    let unblock_event =
        fixtures::domain_event("com.acme/Unblock@1", &fixtures::plan_input_record(vec![]));
    let unblock_emitter = fixtures::stub_event_emitting_reducer(
        store,
        "com.acme/UnblockEmitter@1",
        vec![unblock_event],
    );

    let plan_name = "com.acme/WaitForEvent@1".to_string();
    let plan = DefPlan {
        name: plan_name.clone(),
        input: fixtures::schema("com.acme/PlanIn@1"),
        output: None,
        locals: IndexMap::new(),
        steps: vec![
            PlanStep {
                id: "await".into(),
                kind: PlanStepKind::AwaitEvent(PlanStepAwaitEvent {
                    event: fixtures::schema("com.acme/Unblock@1"),
                    where_clause: None,
                    bind: PlanBind { var: "evt".into() },
                }),
            },
            PlanStep {
                id: "raise".into(),
                kind: PlanStepKind::RaiseEvent(PlanStepRaiseEvent {
                    reducer: result_module.name.clone(),
                    key: None,
                    event: Expr::Record(ExprRecord {
                        record: IndexMap::from([(
                            "value".into(),
                            Expr::Const(ExprConst::Int { int: 5 }),
                        )]),
                    })
                    .into(),
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd { result: None }),
            },
        ],
        edges: vec![
            PlanEdge {
                from: "await".into(),
                to: "raise".into(),
                when: None,
            },
            PlanEdge {
                from: "raise".into(),
                to: "end".into(),
                when: None,
            },
        ],
        required_caps: vec![],
        allowed_effects: vec![],
        invariants: vec![],
    };

    let routing = vec![
        fixtures::routing_event(result_event_schema.as_str(), &result_module.name),
        fixtures::routing_event("com.acme/EmitUnblock@1", &unblock_emitter.name),
    ];
    let mut loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![result_module, unblock_emitter],
        routing,
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(fixtures::START_SCHEMA, vec![("id", text_type())]),
            def_text_record_schema("com.acme/PlanIn@1", vec![("id", text_type())]),
            def_text_record_schema("com.acme/EmitUnblock@1", vec![]),
            def_text_record_schema("com.acme/Unblock@1", vec![]),
            DefSchema {
                name: "com.acme/EventDone@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::from([("value".into(), int_type())]),
                }),
            },
            DefSchema {
                name: "com.acme/EventResultState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );
    loaded
}

/// Builds a test manifest with a reducer that emits a timer effect and another reducer
/// that handles the timer receipt event.
pub fn timer_manifest(store: &Arc<TestStore>) -> aos_kernel::manifest::LoadedManifest {
    let timer_output = ReducerOutput {
        state: Some(vec![0x01]),
        domain_events: vec![],
        effects: vec![ReducerEffect::new(
            "timer.set",
            serde_cbor::to_vec(&TimerSetParams {
                deliver_at_ns: 5,
                key: Some("retry".into()),
            })
            .unwrap(),
        )],
        ann: None,
    };
    let mut timer_emitter =
        fixtures::stub_reducer_module(store, "com.acme/TimerEmitter@1", &timer_output);

    let handler_output = ReducerOutput {
        state: Some(vec![0xCC]),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };
    let mut timer_handler =
        fixtures::stub_reducer_module(store, "com.acme/TimerHandler@1", &handler_output);

    let routing = vec![
        fixtures::routing_event(fixtures::SYS_TIMER_FIRED, &timer_handler.name),
        fixtures::routing_event(START_SCHEMA, &timer_emitter.name),
    ];
    // Provide minimal reducer ABI so routing succeeds.
    timer_emitter.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema(START_SCHEMA),
        event: fixtures::schema(START_SCHEMA),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    timer_handler.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema(fixtures::SYS_TIMER_FIRED),
        event: fixtures::schema(fixtures::SYS_TIMER_FIRED),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let mut loaded = fixtures::build_loaded_manifest(
        vec![],
        vec![],
        vec![timer_emitter, timer_handler],
        routing,
    );
    insert_test_schemas(
        &mut loaded,
        vec![def_text_record_schema(
            fixtures::START_SCHEMA,
            vec![("id", text_type())],
        )],
    );
    loaded
}

/// Builds a simple manifest with a single reducer that sets deterministic state when invoked.
pub fn simple_state_manifest(store: &Arc<TestStore>) -> aos_kernel::manifest::LoadedManifest {
    let reducer = fixtures::stub_reducer_module(
        store,
        "com.acme/Simple@1",
        &ReducerOutput {
            state: Some(vec![0xAA]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    let routing = vec![fixtures::routing_event(START_SCHEMA, &reducer.name)];
    let mut loaded = fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing);
    insert_test_schemas(
        &mut loaded,
        vec![def_text_record_schema(
            START_SCHEMA,
            vec![("id", text_type())],
        )],
    );
    loaded
}

pub fn text_type() -> TypeExpr {
    TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
        text: EmptyObject {},
    }))
}

pub fn int_type() -> TypeExpr {
    TypeExpr::Primitive(TypePrimitive::Int(TypePrimitiveInt {
        int: EmptyObject {},
    }))
}

pub fn def_text_record_schema(name: &str, fields: Vec<(&str, TypeExpr)>) -> DefSchema {
    DefSchema {
        name: name.into(),
        ty: TypeExpr::Record(TypeRecord {
            record: IndexMap::from_iter(fields.into_iter().map(|(k, ty)| (k.to_string(), ty))),
        }),
    }
}

pub fn insert_test_schemas(
    loaded: &mut aos_kernel::manifest::LoadedManifest,
    schemas: Vec<DefSchema>,
) {
    for schema in schemas {
        let name = schema.name.clone();
        loaded.schemas.insert(name.clone(), schema);
        if !loaded
            .manifest
            .schemas
            .iter()
            .any(|existing| existing.name == name)
        {
            loaded.manifest.schemas.push(NamedRef {
                name,
                hash: zero_hash(),
            });
        }
    }
}

/// Attaches a policy to the manifest defaults so it becomes the runtime policy gate.
pub fn attach_default_policy(loaded: &mut aos_kernel::manifest::LoadedManifest, policy: DefPolicy) {
    loaded.manifest.policies.push(NamedRef {
        name: policy.name.clone(),
        hash: zero_hash(),
    });
    if let Some(defaults) = loaded.manifest.defaults.as_mut() {
        defaults.policy = Some(policy.name.clone());
    } else {
        loaded.manifest.defaults = Some(ManifestDefaults {
            policy: Some(policy.name.clone()),
            cap_grants: vec![],
        });
    }
    loaded.policies.insert(policy.name.clone(), policy);
}
