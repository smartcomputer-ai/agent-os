use aos_air_exec::Value as ExprValue;
use aos_air_types::{
    DefPlan, DefSchema, EffectKind, EmptyObject, Expr, ExprConst, ExprOp, ExprOpCode, ExprOrValue,
    ExprRecord, ExprRef, PlanBind, PlanBindEffect, PlanEdge, PlanStep, PlanStepAssign,
    PlanStepAwaitEvent, PlanStepAwaitReceipt, PlanStepEmitEffect, PlanStepEnd, PlanStepKind,
    PlanStepRaiseEvent, ReducerAbi, TypeExpr, TypePrimitive, TypePrimitiveText, TypeRecord,
    ValueLiteral, ValueMap, ValueNull, ValueRecord, ValueText,
    builtins::builtin_schemas,
    plan_literals::{SchemaIndex, normalize_plan_literals},
};
use aos_effects::builtins::{
    BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt, TimerSetParams, TimerSetReceipt,
};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_host::fixtures::{self, START_SCHEMA, TestWorld, effect_params_text, fake_hash};
use aos_kernel::error::KernelError;
use aos_kernel::journal::{JournalKind, JournalRecord, PlanEndStatus, mem::MemJournal};
use aos_wasm_abi::{ReducerEffect, ReducerOutput};
use indexmap::IndexMap;
use serde_cbor;
use serde_json::json;
use std::collections::HashMap;

mod helpers;
use helpers::{
    def_text_record_schema, insert_test_schemas, int_type, simple_state_manifest, text_type,
    timer_manifest,
};

fn builtin_schema_index_with_custom_types() -> SchemaIndex {
    let mut map = HashMap::new();
    for builtin in builtin_schemas() {
        map.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
    }
    let message_field = TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
        text: EmptyObject::default(),
    }));
    map.insert(
        "com.acme/Result@1".into(),
        TypeExpr::Record(TypeRecord {
            record: IndexMap::from([("message".into(), message_field.clone())]),
        }),
    );
    map.insert(
        "com.acme/ResultEvent@1".into(),
        TypeExpr::Record(TypeRecord {
            record: IndexMap::from([("message".into(), message_field)]),
        }),
    );
    SchemaIndex::new(map)
}

fn http_params_literal(tag: &str) -> ExprOrValue {
    ExprOrValue::Literal(ValueLiteral::Record(ValueRecord {
        record: IndexMap::from([
            (
                "method".into(),
                ValueLiteral::Text(ValueText { text: "GET".into() }),
            ),
            (
                "url".into(),
                ValueLiteral::Text(ValueText {
                    text: format!("https://example.com/{tag}"),
                }),
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

#[test]
fn rejects_event_payload_that_violates_schema() {
    let store = fixtures::new_mem_store();
    let loaded = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store, loaded).unwrap();

    let bad_payload = ExprValue::Record(IndexMap::new());
    let err = world
        .submit_event_value_result(START_SCHEMA, &bad_payload)
        .expect_err("event should fail schema validation");
    match err {
        KernelError::Manifest(msg) => {
            assert!(msg.contains("payload failed validation"));
            assert!(msg.contains("record missing field 'id'"));
        }
        other => panic!("unexpected error: {:?}", other),
    }
}

/// Happy-path end-to-end: reducer emits an intent, plan does work, receipt feeds a result event
/// back into the reducer. Mirrors the “single plan orchestration” pattern in the spec.
#[test]
fn sugar_literal_plan_executes_http_flow() {
    let store = fixtures::new_mem_store();
    let mut result_module = fixtures::stub_reducer_module(
        &store,
        "com.acme/ResultReducer@1",
        &ReducerOutput {
            state: Some(vec![0xEE]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    result_module.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/Result@1"),
        event: fixtures::schema("com.acme/ResultEvent@1"),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: IndexMap::new(),
    });

    let plan_name = "com.acme/SugarPlan@1";
    let plan_json = json!({
        "$kind": "defplan",
        "name": plan_name,
        "input": "com.acme/PlanIn@1",
        "output": "com.acme/Result@1",
        "locals": { "resp": "sys/HttpRequestReceipt@1" },
        "steps": [
            {
                "id": "emit",
                "op": "emit_effect",
                "kind": "http.request",
                "params": {
                    "method": "POST",
                    "url": "https://example.com",
                    "headers": { "content-type": "application/json" },
                    "body_ref": null
                },
                "cap": "cap_http",
                "bind": { "effect_id_as": "req" }
            },
            {
                "id": "await",
                "op": "await_receipt",
                "for": { "ref": "@var:req" },
                "bind": { "as": "resp" }
            },
            {
                "id": "raise",
                "op": "raise_event",
                "reducer": "com.acme/ResultReducer@1",
                "event": { "message": "done" }
            },
            {
                "id": "end",
                "op": "end",
                "result": { "message": "done" }
            }
        ],
        "edges": [
            { "from": "emit", "to": "await" },
            { "from": "await", "to": "raise" },
            { "from": "raise", "to": "end" }
        ],
        "required_caps": ["cap_http"],
        "allowed_effects": ["http.request"]
    });
    let mut plan: DefPlan = serde_json::from_value(plan_json).expect("plan json");
    if let Some(step) = plan.steps.iter_mut().find(|step| step.id == "raise") {
        if let PlanStepKind::RaiseEvent(raise) = &mut step.kind {
            raise.event = Expr::Record(ExprRecord {
                record: IndexMap::from([("message".into(), fixtures::text_expr("done"))]),
            })
            .into();
        }
    }
    let mut modules = HashMap::new();
    modules.insert(result_module.name.clone(), result_module.clone());
    let effect_catalog = aos_air_types::catalog::EffectCatalog::from_defs(
        aos_air_types::builtins::builtin_effects()
            .iter()
            .map(|e| e.effect.clone()),
    );
    normalize_plan_literals(
        &mut plan,
        &builtin_schema_index_with_custom_types(),
        &modules,
        &effect_catalog,
    )
    .expect("normalize literals");

    let routing = vec![fixtures::routing_event(
        "com.acme/ResultEvent@1",
        &result_module.name,
    )];
    let mut loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(plan_name)],
        vec![result_module.clone()],
        routing,
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            def_text_record_schema("com.acme/PlanIn@1", vec![("id", text_type())]),
            def_text_record_schema("com.acme/Result@1", vec![("message", text_type())]),
            def_text_record_schema("com.acme/ResultEvent@1", vec![("message", text_type())]),
        ],
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    let input = fixtures::plan_input_record(vec![("id", ExprValue::Text("123".into()))]);
    world
        .submit_event_value_result(START_SCHEMA, &input)
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let mut effects = world.drain_effects();
    assert_eq!(effects.len(), 1);
    let effect = effects.remove(0);

    let receipt_payload = serde_cbor::to_vec(&ExprValue::Text("ok".into())).unwrap();
    let receipt = EffectReceipt {
        intent_hash: effect.intent_hash,
        adapter_id: "adapter.http".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: receipt_payload,
        cost_cents: None,
        signature: vec![],
    };
    world.kernel.handle_receipt(receipt).unwrap();
    world.kernel.tick_until_idle().unwrap();

    assert_eq!(
        world.kernel.reducer_state("com.acme/ResultReducer@1"),
        Some(&vec![0xEE])
    );
}

#[test]
fn single_plan_orchestration_completes_after_receipt() {
    let store = fixtures::new_mem_store();

    let mut result_module = fixtures::stub_reducer_module(
        &store,
        "com.acme/ResultReducer@1",
        &ReducerOutput {
            state: Some(vec![0xEE]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    result_module.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/Result@1"),
        event: fixtures::schema("com.acme/ResultEvent@1"),
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
                    params: http_params_literal("body"),
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
        "com.acme/ResultEvent@1",
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
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            def_text_record_schema("com.acme/PlanIn@1", vec![("id", text_type())]),
            def_text_record_schema("com.acme/Result@1", vec![("message", text_type())]),
            DefSchema {
                name: "com.acme/ResultEvent@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::from([("value".into(), int_type())]),
                }),
            },
        ],
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    let input = fixtures::plan_input_record(vec![("id", ExprValue::Text("123".into()))]);
    world
        .submit_event_value_result(START_SCHEMA, &input)
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let mut effects = world.drain_effects();
    assert_eq!(effects.len(), 1);
    let effect = effects.remove(0);

    let receipt_payload = serde_cbor::to_vec(&ExprValue::Text("done".into())).unwrap();
    let receipt = EffectReceipt {
        intent_hash: effect.intent_hash,
        adapter_id: "adapter.http".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: receipt_payload,
        cost_cents: None,
        signature: vec![],
    };
    world.kernel.handle_receipt(receipt).unwrap();
    world.kernel.tick_until_idle().unwrap();

    assert_eq!(
        world.kernel.reducer_state("com.acme/ResultReducer@1"),
        Some(&vec![0xEE])
    );
}

/// Reducer micro-effects and plan-sourced effects should share the same outbox without interfering.
#[test]
fn reducer_and_plan_effects_are_enqueued() {
    let store = fixtures::new_mem_store();

    let reducer_output = ReducerOutput {
        state: Some(vec![0xAA]),
        domain_events: vec![],
        effects: vec![ReducerEffect::new(
            "timer.set",
            serde_cbor::to_vec(&TimerSetParams {
                deliver_at_ns: 10,
                key: Some("retry".into()),
            })
            .unwrap(),
        )],
        ann: None,
    };
    let reducer_module =
        fixtures::stub_reducer_module(&store, "com.acme/Reducer@1", &reducer_output);

    let plan_name = "com.acme/EmitOnly@1".to_string();
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
                    params: http_params_literal("plan"),
                    cap: "cap_http".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd { result: None }),
            },
        ],
        edges: vec![PlanEdge {
            from: "emit".into(),
            to: "end".into(),
            when: None,
        }],
        required_caps: vec!["cap_http".into()],
        allowed_effects: vec![EffectKind::http_request()],
        invariants: vec![],
    };

    let routing = vec![fixtures::routing_event(START_SCHEMA, &reducer_module.name)];
    let loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![reducer_module],
        routing,
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    let input = fixtures::plan_input_record(vec![("id", ExprValue::Text("123".into()))]);
    world
        .submit_event_value_result(START_SCHEMA, &input)
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let effects = world.drain_effects();
    assert_eq!(effects.len(), 2);
    let kinds: Vec<_> = effects.iter().map(|e| e.kind.as_str()).collect();
    assert!(kinds.contains(&aos_effects::EffectKind::TIMER_SET));
    assert!(kinds.contains(&aos_effects::EffectKind::HTTP_REQUEST));
    assert_eq!(
        world.kernel.reducer_state("com.acme/Reducer@1"),
        Some(&vec![0xAA])
    );
}

/// Timer receipts emitted by reducers must be translated into `sys/TimerFired@1` and routed
/// through the normal event pipeline (including duplicate suppression / unknown handling).
#[test]
fn reducer_timer_receipt_routes_event_to_handler() {
    let store = fixtures::new_mem_store();
    let manifest = timer_manifest(&store);
    let mut world = TestWorld::with_store(store, manifest).unwrap();
    world
        .submit_event_value_result(START_SCHEMA, &fixtures::plan_input_record(vec![]))
        .expect("submit start event");
    world.tick_n(1).unwrap();

    let mut effects = world.drain_effects();
    assert_eq!(effects.len(), 1);
    let effect = effects.remove(0);

    let receipt = EffectReceipt {
        intent_hash: effect.intent_hash,
        adapter_id: "adapter.timer".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
            delivered_at_ns: 10,
            key: Some("retry".into()),
        })
        .unwrap(),
        cost_cents: Some(1),
        signature: vec![1, 2, 3],
    };
    world.kernel.handle_receipt(receipt).unwrap();
    world.tick_n(1).unwrap();

    assert_eq!(
        world.kernel.reducer_state("com.acme/TimerHandler@1"),
        Some(&vec![0xCC])
    );

    let duplicate = EffectReceipt {
        intent_hash: effect.intent_hash,
        adapter_id: "adapter.timer".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
            delivered_at_ns: 10,
            key: Some("retry".into()),
        })
        .unwrap(),
        cost_cents: Some(1),
        signature: vec![1, 2, 3],
    };
    world.kernel.handle_receipt(duplicate).unwrap();
    world.tick_n(1).unwrap();
    assert!(world.drain_effects().is_empty());

    let unknown = EffectReceipt {
        intent_hash: [9u8; 32],
        adapter_id: "adapter.timer".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
            delivered_at_ns: 42,
            key: None,
        })
        .unwrap(),
        cost_cents: None,
        signature: vec![],
    };
    let err = world.kernel.handle_receipt(unknown).unwrap_err();
    assert!(matches!(err, KernelError::UnknownReceipt(_)));
}

/// Guards on plan edges should gate side-effects and completion state.
#[test]
fn guarded_plan_branches_control_effects() {
    let plan_name = "com.acme/Guarded@1".to_string();
    let plan = DefPlan {
        name: plan_name.clone(),
        input: fixtures::schema("com.acme/FlagIntent@1"),
        output: None,
        locals: IndexMap::new(),
        steps: vec![
            PlanStep {
                id: "assign".into(),
                kind: PlanStepKind::Assign(PlanStepAssign {
                    expr: fixtures::plan_input_expr("flag").into(),
                    bind: PlanBind { var: "flag".into() },
                }),
            },
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: http_params_literal("do-it"),
                    cap: "cap_http".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd { result: None }),
            },
        ],
        edges: vec![
            PlanEdge {
                from: "assign".into(),
                to: "emit".into(),
                when: Some(fixtures::var_expr("flag")),
            },
            PlanEdge {
                from: "emit".into(),
                to: "end".into(),
                when: None,
            },
        ],
        required_caps: vec!["cap_http".into()],
        allowed_effects: vec![EffectKind::http_request()],
        invariants: vec![],
    };

    let loaded = fixtures::build_loaded_manifest(
        vec![plan.clone()],
        vec![fixtures::start_trigger(&plan_name)],
        vec![],
        vec![],
    );

    let mut world = TestWorld::new(loaded).unwrap();
    let true_input = fixtures::plan_input_record(vec![("flag", ExprValue::Bool(true))]);
    world
        .submit_event_value_result(START_SCHEMA, &true_input)
        .expect("submit start event");
    world.tick_n(2).unwrap();
    assert_eq!(world.drain_effects().len(), 1);

    let loaded_false = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![],
        vec![],
    );
    let mut world_false = TestWorld::new(loaded_false).unwrap();
    let false_input = fixtures::plan_input_record(vec![("flag", ExprValue::Bool(false))]);
    world_false
        .submit_event_value_result(START_SCHEMA, &false_input)
        .expect("submit start event");
    world_false.tick_n(2).unwrap();
    assert_eq!(world_false.drain_effects().len(), 0);
}
/// Blob.put receipts should be mapped into `sys/BlobPutResult@1` and delivered to reducers.
#[test]
fn blob_put_receipt_routes_event_to_handler() {
    let store = fixtures::new_mem_store();

    let emitter = fixtures::stub_reducer_module(
        &store,
        "com.acme/BlobPutEmitter@1",
        &ReducerOutput {
            state: None,
            domain_events: vec![],
            effects: vec![ReducerEffect::new(
                aos_effects::EffectKind::BLOB_PUT,
                serde_cbor::to_vec(&BlobPutParams {
                    namespace: "docs".into(),
                    blob_ref: fake_hash(0x20),
                })
                .unwrap(),
            )],
            ann: None,
        },
    );

    let handler = fixtures::stub_reducer_module(
        &store,
        "com.acme/BlobPutHandler@1",
        &ReducerOutput {
            state: Some(vec![0xDD]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );

    let routing = vec![
        fixtures::routing_event(START_SCHEMA, &emitter.name),
        fixtures::routing_event("sys/BlobPutResult@1", &handler.name),
    ];
    let mut loaded =
        fixtures::build_loaded_manifest(vec![], vec![], vec![emitter, handler], routing);
    if let Some(binding) = loaded
        .manifest
        .module_bindings
        .get_mut(&"com.acme/BlobPutEmitter@1".to_string())
    {
        binding.slots.insert("default".into(), "blob_cap".into());
    }

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_value_result(START_SCHEMA, &fixtures::plan_input_record(vec![]))
        .expect("submit start event");
    world.tick_n(1).unwrap();

    let mut effects = world.drain_effects();
    assert_eq!(effects.len(), 1);
    let intent = effects.remove(0);
    assert_eq!(intent.kind.as_str(), aos_effects::EffectKind::BLOB_PUT);

    let receipt = EffectReceipt {
        intent_hash: intent.intent_hash,
        adapter_id: "adapter.blob".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&BlobPutReceipt {
            blob_ref: fake_hash(0x21),
            size: 64,
        })
        .unwrap(),
        cost_cents: Some(2),
        signature: vec![7, 7],
    };
    world.kernel.handle_receipt(receipt).unwrap();
    world.tick_n(1).unwrap();

    assert_eq!(
        world.kernel.reducer_state("com.acme/BlobPutHandler@1"),
        Some(&vec![0xDD])
    );
}

/// Blob.get receipts should similarly map into `sys/BlobGetResult@1` and wake reducers.
#[test]
fn blob_get_receipt_routes_event_to_handler() {
    let store = fixtures::new_mem_store();

    let emitter = fixtures::stub_reducer_module(
        &store,
        "com.acme/BlobGetEmitter@1",
        &ReducerOutput {
            state: None,
            domain_events: vec![],
            effects: vec![ReducerEffect::new(
                aos_effects::EffectKind::BLOB_GET,
                serde_cbor::to_vec(&BlobGetParams {
                    namespace: "docs".into(),
                    key: "readme".into(),
                })
                .unwrap(),
            )],
            ann: None,
        },
    );

    let handler = fixtures::stub_reducer_module(
        &store,
        "com.acme/BlobGetHandler@1",
        &ReducerOutput {
            state: Some(vec![0xEE]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );

    let routing = vec![
        fixtures::routing_event(START_SCHEMA, &emitter.name),
        fixtures::routing_event("sys/BlobGetResult@1", &handler.name),
    ];
    let mut loaded =
        fixtures::build_loaded_manifest(vec![], vec![], vec![emitter, handler], routing);
    if let Some(binding) = loaded
        .manifest
        .module_bindings
        .get_mut(&"com.acme/BlobGetEmitter@1".to_string())
    {
        binding.slots.insert("default".into(), "blob_cap".into());
    }

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_value_result(START_SCHEMA, &fixtures::plan_input_record(vec![]))
        .expect("submit start event");
    world.tick_n(1).unwrap();

    let mut effects = world.drain_effects();
    assert_eq!(effects.len(), 1);
    let intent = effects.remove(0);
    assert_eq!(intent.kind.as_str(), aos_effects::EffectKind::BLOB_GET);

    let receipt = EffectReceipt {
        intent_hash: intent.intent_hash,
        adapter_id: "adapter.blob".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&BlobGetReceipt {
            blob_ref: fake_hash(0x22),
            size: 128,
        })
        .unwrap(),
        cost_cents: None,
        signature: vec![8, 8],
    };
    world.kernel.handle_receipt(receipt).unwrap();
    world.tick_n(1).unwrap();

    assert_eq!(
        world.kernel.reducer_state("com.acme/BlobGetHandler@1"),
        Some(&vec![0xEE])
    );
}

/// Complex plan scenario: emit effect → await receipt → emit → await domain event → emit. Ensures
/// interleaving of effect receipts and raised events still produces deterministic progression.
#[test]
fn plan_waits_for_receipt_and_event_before_progressing() {
    let store = fixtures::new_mem_store();
    let plan_name = "com.acme/TwoStage@1".to_string();

    let next_emitter = fixtures::stub_event_emitting_reducer(
        &store,
        "com.acme/NextEmitter@1",
        vec![fixtures::domain_event(
            "com.acme/Next@1",
            &ExprValue::Int(1),
        )],
    );

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
                    params: http_params_literal("first"),
                    cap: "cap_http".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "wait_receipt".into(),
                kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                    for_expr: fixtures::var_expr("req"),
                    bind: PlanBind { var: "resp".into() },
                }),
            },
            PlanStep {
                id: "after_receipt".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: http_params_literal("after-receipt"),
                    cap: "cap_http".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "second".into(),
                    },
                }),
            },
            PlanStep {
                id: "wait_event".into(),
                kind: PlanStepKind::AwaitEvent(PlanStepAwaitEvent {
                    event: fixtures::schema("com.acme/Next@1"),
                    where_clause: None,
                    bind: PlanBind { var: "evt".into() },
                }),
            },
            PlanStep {
                id: "after_event".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: http_params_literal("after-event"),
                    cap: "cap_http".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "third".into(),
                    },
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
                to: "wait_receipt".into(),
                when: None,
            },
            PlanEdge {
                from: "wait_receipt".into(),
                to: "after_receipt".into(),
                when: None,
            },
            PlanEdge {
                from: "after_receipt".into(),
                to: "wait_event".into(),
                when: None,
            },
            PlanEdge {
                from: "wait_event".into(),
                to: "after_event".into(),
                when: None,
            },
            PlanEdge {
                from: "after_event".into(),
                to: "end".into(),
                when: None,
            },
        ],
        required_caps: vec!["cap_http".into()],
        allowed_effects: vec![EffectKind::http_request()],
        invariants: vec![],
    };

    let routing = vec![fixtures::routing_event(
        "com.acme/PulseNext@1",
        &next_emitter.name,
    )];
    let loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![next_emitter],
        routing,
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_value_result(START_SCHEMA, &fixtures::plan_input_record(vec![]))
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let mut effects = world.drain_effects();
    assert_eq!(effects.len(), 1);
    let first_intent = effects.remove(0);

    let receipt = EffectReceipt {
        intent_hash: first_intent.intent_hash,
        adapter_id: "adapter.http".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&ExprValue::Text("done".into())).unwrap(),
        cost_cents: None,
        signature: vec![],
    };
    world.kernel.handle_receipt(receipt).unwrap();
    world.tick_n(2).unwrap();

    let mut after_receipt_effects = world.drain_effects();
    assert_eq!(after_receipt_effects.len(), 1);
    let second_intent = after_receipt_effects.remove(0);
    assert!(
        effect_params_text(&second_intent).ends_with("after-receipt"),
        "unexpected params: {}",
        effect_params_text(&second_intent)
    );

    world
        .submit_event_value_result("com.acme/PulseNext@1", &fixtures::plan_input_record(vec![]))
        .expect("submit pulse event");
    world.kernel.tick_until_idle().unwrap();

    let mut after_event_effects = world.drain_effects();
    assert_eq!(after_event_effects.len(), 1);
    assert!(
        effect_params_text(&after_event_effects.remove(0)).ends_with("after-event"),
        "unexpected params"
    );
}

/// Plans blocked on `await_event` should only resume when the subscribed schema fires; different
/// schemas should remain pending even if their triggers fire later.
#[test]
fn plan_event_wakeup_only_resumes_matching_schema() {
    let store = fixtures::new_mem_store();
    let plan_ready = DefPlan {
        name: "com.acme/WaitReady@1".into(),
        input: fixtures::schema("com.acme/PlanIn@1"),
        output: None,
        locals: IndexMap::new(),
        steps: vec![
            PlanStep {
                id: "wait".into(),
                kind: PlanStepKind::AwaitEvent(PlanStepAwaitEvent {
                    event: fixtures::schema("com.acme/Ready@1"),
                    where_clause: None,
                    bind: PlanBind { var: "evt".into() },
                }),
            },
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: http_params_literal("ready"),
                    cap: "cap_http".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd { result: None }),
            },
        ],
        edges: vec![
            PlanEdge {
                from: "wait".into(),
                to: "emit".into(),
                when: None,
            },
            PlanEdge {
                from: "emit".into(),
                to: "end".into(),
                when: None,
            },
        ],
        required_caps: vec!["cap_http".into()],
        allowed_effects: vec![EffectKind::http_request()],
        invariants: vec![],
    };

    let plan_other = DefPlan {
        name: "com.acme/WaitOther@1".into(),
        input: fixtures::schema("com.acme/PlanIn@1"),
        output: None,
        locals: IndexMap::new(),
        steps: vec![
            PlanStep {
                id: "wait".into(),
                kind: PlanStepKind::AwaitEvent(PlanStepAwaitEvent {
                    event: fixtures::schema("com.acme/Other@1"),
                    where_clause: None,
                    bind: PlanBind { var: "evt".into() },
                }),
            },
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: http_params_literal("other"),
                    cap: "cap_http".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd { result: None }),
            },
        ],
        edges: vec![
            PlanEdge {
                from: "wait".into(),
                to: "emit".into(),
                when: None,
            },
            PlanEdge {
                from: "emit".into(),
                to: "end".into(),
                when: None,
            },
        ],
        required_caps: vec!["cap_http".into()],
        allowed_effects: vec![EffectKind::http_request()],
        invariants: vec![],
    };

    let loaded = fixtures::build_loaded_manifest(
        vec![plan_ready.clone(), plan_other.clone()],
        vec![
            fixtures::start_trigger(&plan_ready.name),
            fixtures::start_trigger(&plan_other.name),
        ],
        vec![
            fixtures::stub_event_emitting_reducer(
                &store,
                "com.acme/ReadyEmitter@1",
                vec![fixtures::domain_event(
                    "com.acme/Ready@1",
                    &ExprValue::Nat(7),
                )],
            ),
            fixtures::stub_event_emitting_reducer(
                &store,
                "com.acme/OtherEmitter@1",
                vec![fixtures::domain_event(
                    "com.acme/Other@1",
                    &ExprValue::Nat(9),
                )],
            ),
        ],
        vec![
            fixtures::routing_event("com.acme/TriggerReady@1", "com.acme/ReadyEmitter@1"),
            fixtures::routing_event("com.acme/TriggerOther@1", "com.acme/OtherEmitter@1"),
        ],
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_value_result(START_SCHEMA, &fixtures::plan_input_record(vec![]))
        .expect("submit start event");
    world.tick_n(2).unwrap();
    assert!(world.drain_effects().is_empty());

    world
        .submit_event_value_result(
            "com.acme/TriggerReady@1",
            &fixtures::plan_input_record(vec![]),
        )
        .expect("submit ready trigger");
    world.kernel.tick_until_idle().unwrap();
    let mut effects = world.drain_effects();
    assert_eq!(effects.len(), 1);
    assert!(effect_params_text(&effects.remove(0)).ends_with("ready"));

    world
        .submit_event_value_result(
            "com.acme/TriggerOther@1",
            &fixtures::plan_input_record(vec![]),
        )
        .expect("submit other trigger");
    world.kernel.tick_until_idle().unwrap();
    let mut more_effects = world.drain_effects();
    assert_eq!(more_effects.len(), 1);
    assert!(effect_params_text(&more_effects.remove(0)).ends_with("other"));
}

#[test]
fn plan_outputs_are_journaled_and_replayed() {
    let store = fixtures::new_mem_store();
    let plan_name = "com.acme/OutputPlan@1";
    let output_schema = "com.acme/PlanOut@1";

    let build_manifest = || {
        let plan = DefPlan {
            name: plan_name.to_string(),
            input: fixtures::schema("com.acme/PlanIn@1"),
            output: Some(fixtures::schema(output_schema)),
            locals: IndexMap::new(),
            steps: vec![PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd {
                    result: Some(
                        Expr::Record(ExprRecord {
                            record: IndexMap::from([(
                                "message".into(),
                                Expr::Const(ExprConst::Text {
                                    text: "done".into(),
                                }),
                            )]),
                        })
                        .into(),
                    ),
                }),
            }],
            edges: vec![],
            required_caps: vec![],
            allowed_effects: vec![],
            invariants: vec![],
        };

        let mut loaded = fixtures::build_loaded_manifest(
            vec![plan],
            vec![fixtures::start_trigger(plan_name)],
            vec![],
            vec![],
        );
        insert_test_schemas(
            &mut loaded,
            vec![
                def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
                def_text_record_schema("com.acme/PlanIn@1", vec![("id", text_type())]),
                DefSchema {
                    name: output_schema.into(),
                    ty: TypeExpr::Record(TypeRecord {
                        record: IndexMap::from([("message".into(), text_type())]),
                    }),
                },
            ],
        );
        loaded
    };

    let manifest = build_manifest();
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();
    world
        .submit_event_value_result(
            START_SCHEMA,
            &fixtures::plan_input_record(vec![("id", ExprValue::Text("123".into()))]),
        )
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let results = world.kernel.recent_plan_results();
    assert_eq!(results.len(), 1);
    let entry = &results[0];
    assert_eq!(entry.plan_name, plan_name);
    assert_eq!(entry.output_schema, output_schema);
    let value: ExprValue = serde_cbor::from_slice(&entry.value_cbor).unwrap();
    assert_eq!(
        value,
        ExprValue::Record(IndexMap::from([(
            "message".into(),
            ExprValue::Text("done".into()),
        )]))
    );

    let journal_entries = world.kernel.dump_journal().unwrap();
    assert!(
        journal_entries
            .iter()
            .any(|entry| entry.kind == JournalKind::PlanResult)
    );

    let replay_manifest = build_manifest();
    let replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        replay_manifest,
        Box::new(MemJournal::from_entries(&journal_entries)),
    )
    .unwrap();
    let replay_results = replay_world.kernel.recent_plan_results();
    assert_eq!(replay_results.len(), 1);
    let replay_value: ExprValue = serde_cbor::from_slice(&replay_results[0].value_cbor).unwrap();
    assert_eq!(replay_value, value);
}

#[test]
fn invariant_failure_records_plan_ended_error() {
    let store = fixtures::new_mem_store();
    let plan_name = "com.acme/InvariantPlan@1";

    let build_manifest = || {
        let plan = DefPlan {
            name: plan_name.to_string(),
            input: fixtures::schema("com.acme/PlanIn@1"),
            output: None,
            locals: IndexMap::new(),
            steps: vec![PlanStep {
                id: "set".into(),
                kind: PlanStepKind::Assign(PlanStepAssign {
                    expr: Expr::Const(ExprConst::Int { int: 5 }).into(),
                    bind: PlanBind { var: "val".into() },
                }),
            }],
            edges: vec![],
            required_caps: vec![],
            allowed_effects: vec![],
            invariants: vec![Expr::Op(ExprOp {
                op: ExprOpCode::Lt,
                args: vec![
                    Expr::Ref(ExprRef {
                        reference: "@var:val".into(),
                    }),
                    Expr::Const(ExprConst::Int { int: 1 }),
                ],
            })],
        };

        let mut loaded = fixtures::build_loaded_manifest(
            vec![plan],
            vec![fixtures::start_trigger(plan_name)],
            vec![],
            vec![],
        );
        insert_test_schemas(
            &mut loaded,
            vec![
                def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
                def_text_record_schema("com.acme/PlanIn@1", vec![("id", text_type())]),
            ],
        );
        loaded
    };

    let manifest = build_manifest();
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();
    world
        .submit_event_value_result(
            START_SCHEMA,
            &fixtures::plan_input_record(vec![("id", ExprValue::Text("123".into()))]),
        )
        .expect("submit start event");
    world.kernel.tick_until_idle().unwrap();

    let journal_entries = world.kernel.dump_journal().unwrap();
    let mut ended_entries = journal_entries
        .iter()
        .filter(|entry| entry.kind == JournalKind::PlanEnded);
    let ended = ended_entries.next().expect("plan ended entry");
    assert!(ended_entries.next().is_none(), "only one plan ended entry");

    let record: JournalRecord = serde_cbor::from_slice(&ended.payload).unwrap();
    match record {
        JournalRecord::PlanEnded(rec) => {
            assert_eq!(rec.plan_name, plan_name);
            assert_eq!(rec.status, PlanEndStatus::Error);
            assert_eq!(rec.error_code.as_deref(), Some("invariant_violation"));
        }
        other => panic!("unexpected record {:?}", other),
    }

    assert!(
        journal_entries
            .iter()
            .all(|entry| entry.kind != JournalKind::PlanResult),
        "no plan result recorded on invariant failure"
    );
}

/// Plans that raise events should deliver them to reducers according to manifest routing.
#[test]
fn raised_events_are_routed_to_reducers() {
    let store = fixtures::new_mem_store();

    let reducer_output = ReducerOutput {
        state: Some(vec![0xEE]),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };
    let mut reducer_module =
        fixtures::stub_reducer_module(&store, "com.acme/Reducer@1", &reducer_output);
    reducer_module.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/RaisedState@1"),
        event: fixtures::schema("com.acme/Raised@1"),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: IndexMap::new(),
    });
    let reducer_name = reducer_module.name.clone();

    let plan = DefPlan {
        name: "com.acme/Raise@1".into(),
        input: fixtures::schema("com.acme/PlanIn@1"),
        output: None,
        locals: IndexMap::new(),
        steps: vec![
            PlanStep {
                id: "raise".into(),
                kind: PlanStepKind::RaiseEvent(PlanStepRaiseEvent {
                    reducer: reducer_name.clone(),
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
        edges: vec![PlanEdge {
            from: "raise".into(),
            to: "end".into(),
            when: None,
        }],
        required_caps: vec![],
        allowed_effects: vec![],
        invariants: vec![],
    };

    let routing = vec![fixtures::routing_event("com.acme/Raised@1", &reducer_name)];
    let mut loaded = fixtures::build_loaded_manifest(
        vec![plan.clone()],
        vec![fixtures::start_trigger(&plan.name)],
        vec![reducer_module],
        routing,
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            def_text_record_schema("com.acme/PlanIn@1", vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/Raised@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::from([("value".into(), int_type())]),
                }),
            },
            DefSchema {
                name: "com.acme/RaisedState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );
    assert!(
        loaded
            .modules
            .get(&reducer_name)
            .and_then(|module| module.abi.reducer.as_ref())
            .is_some(),
        "Reducer ABI missing"
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_value_result(START_SCHEMA, &fixtures::plan_input_record(vec![]))
        .expect("submit start event");
    world.kernel.tick_until_idle().unwrap();

    assert_eq!(
        world.kernel.reducer_state("com.acme/Reducer@1"),
        Some(&vec![0xEE])
    );
}
