use aos_air_exec::Value as ExprValue;
use aos_air_types::{
    DefModule, DefPlan, DefSchema, EffectKind, EmptyObject, Expr, ExprConst, ExprMap, ExprOp,
    ExprOpCode, ExprOrValue, ExprRecord, ExprRef, PlanBind, PlanBindEffect, PlanBindHandle,
    PlanEdge, PlanStep, PlanStepAwaitEvent, PlanStepAwaitPlan, PlanStepAwaitReceipt,
    PlanStepEmitEffect, PlanStepEnd, PlanStepKind, PlanStepRaiseEvent, PlanStepSpawnPlan,
    ReducerAbi, RoutingEvent, Trigger, TypeExpr, TypePrimitive, TypePrimitiveInt,
    TypePrimitiveNat, TypeRecord, TypeRef, TypeVariant, ValueLiteral, ValueMap, ValueNull,
    ValueRecord, ValueText,
};
use aos_effects::builtins::{
    BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt, TimerSetParams, TimerSetReceipt,
};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_host::trace::plan_run_summary;
use aos_kernel::cap_enforcer::CapCheckOutput;
use aos_kernel::error::KernelError;
use aos_kernel::journal::mem::MemJournal;
use aos_wasm_abi::{PureOutput, ReducerEffect, ReducerOutput};
use helpers::fixtures::{self, START_SCHEMA, TestStore, TestWorld, effect_params_text, fake_hash};
use indexmap::IndexMap;
use serde_cbor;
use serde_json::json;
use std::sync::Arc;

mod helpers;
use helpers::{
    def_text_record_schema, insert_test_schemas, int_type, simple_state_manifest, text_type,
    timer_manifest,
};

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

fn allow_http_enforcer(store: &Arc<TestStore>) -> DefModule {
    let allow_output = CapCheckOutput {
        constraints_ok: true,
        deny: None,
    };
    let output_bytes = serde_cbor::to_vec(&allow_output).expect("encode cap output");
    let pure_output = PureOutput {
        output: output_bytes,
    };
    fixtures::stub_pure_module(
        store,
        "sys/CapEnforceHttpOut@1",
        &pure_output,
        "sys/CapCheckInput@1",
        "sys/CapCheckOutput@1",
    )
}

fn build_loaded_manifest_with_http_enforcer(
    store: &Arc<TestStore>,
    plans: Vec<DefPlan>,
    triggers: Vec<Trigger>,
    mut modules: Vec<DefModule>,
    routing: Vec<RoutingEvent>,
) -> aos_kernel::manifest::LoadedManifest {
    if !modules
        .iter()
        .any(|module| module.name == "sys/CapEnforceHttpOut@1")
    {
        modules.push(allow_http_enforcer(store));
    }
    fixtures::build_loaded_manifest(plans, triggers, modules, routing)
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


#[test]
#[ignore = "P2: plan-runtime integration fixture retired; replace with workflow-native coverage"]
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
        context: Some(fixtures::schema("sys/ReducerContext@1")),
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
                    idempotency_key: None,
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
                    event: fixtures::schema("com.acme/ResultEvent@1"),
                    value: Expr::Record(ExprRecord {
                        record: IndexMap::from([(
                            "value".into(),
                            Expr::Const(ExprConst::Int { int: 9 }),
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
    let mut loaded = build_loaded_manifest_with_http_enforcer(
        &store,
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
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("123"))
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let mut effects = world.drain_effects().expect("drain effects");
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
        Some(vec![0xEE])
    );
}

/// Reducer micro-effects and plan-sourced effects should share the same outbox without interfering.
#[test]
#[ignore = "P2: plan-runtime integration fixture retired; replace with workflow-native coverage"]
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
    let mut reducer_module =
        fixtures::stub_reducer_module(&store, "com.acme/Reducer@1", &reducer_output);
    reducer_module.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/ReducerState@1"),
        event: fixtures::schema(START_SCHEMA),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

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
                    idempotency_key: None,
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
    let mut loaded = build_loaded_manifest_with_http_enforcer(
        &store,
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![reducer_module],
        routing,
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            def_text_record_schema("com.acme/PlanIn@1", vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/ReducerState@1".into(),
                ty: text_type(),
            },
        ],
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("123"))
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 2);
    let kinds: Vec<_> = effects.iter().map(|e| e.kind.as_str()).collect();
    assert!(kinds.contains(&aos_effects::EffectKind::TIMER_SET));
    assert!(kinds.contains(&aos_effects::EffectKind::HTTP_REQUEST));
    assert_eq!(
        world.kernel.reducer_state("com.acme/Reducer@1"),
        Some(vec![0xAA])
    );
}

/// Timer receipts emitted by reducers must route through the normal event pipeline
/// (including duplicate suppression / unknown handling) and wrap at dispatch.
#[test]
#[ignore = "P2: plan-runtime integration fixture retired; replace with workflow-native coverage"]
fn reducer_timer_receipt_routes_event_to_handler() {
    let store = fixtures::new_mem_store();
    let manifest = timer_manifest(&store);
    let mut world = TestWorld::with_store(store, manifest).unwrap();
    let start_event = fixtures::start_event("timer");
    world
        .submit_event_result(START_SCHEMA, &start_event)
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let mut effects = world.drain_effects().expect("drain effects");
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
        Some(vec![0xCC])
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
    assert!(world.drain_effects().expect("drain effects").is_empty());

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

/// Blob.put receipts should map into `sys/BlobPutResult@1`, route through the bus, and wrap at dispatch.
#[test]
#[ignore = "P2: plan-runtime integration fixture retired; replace with workflow-native coverage"]
fn blob_put_receipt_routes_event_to_handler() {
    let store = fixtures::new_mem_store();

    let mut emitter = fixtures::stub_reducer_module(
        &store,
        "com.acme/BlobPutEmitter@1",
        &ReducerOutput {
            state: None,
            domain_events: vec![],
            effects: vec![ReducerEffect::new(
                aos_effects::EffectKind::BLOB_PUT,
                serde_cbor::to_vec(&BlobPutParams {
                    bytes: Vec::new(),
                    blob_ref: None,
                    refs: None,
                })
                .unwrap(),
            )],
            ann: None,
        },
    );

    let mut handler = fixtures::stub_reducer_module(
        &store,
        "com.acme/BlobPutHandler@1",
        &ReducerOutput {
            state: Some(vec![0xDD]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );

    let event_schema = "com.acme/BlobPutEvent@1";
    emitter.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema(START_SCHEMA),
        event: fixtures::schema(event_schema),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    handler.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema(START_SCHEMA),
        event: fixtures::schema(event_schema),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    let routing = vec![
        fixtures::routing_event(event_schema, &emitter.name),
        fixtures::routing_event(event_schema, &handler.name),
    ];
    let mut loaded = build_loaded_manifest_with_http_enforcer(
        &store,
        vec![],
        vec![],
        vec![emitter, handler],
        routing,
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: event_schema.into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(START_SCHEMA),
                            }),
                        ),
                        (
                            "PutResult".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("sys/BlobPutResult@1"),
                            }),
                        ),
                    ]),
                }),
            },
        ],
    );
    if let Some(binding) = loaded
        .manifest
        .module_bindings
        .get_mut(&"com.acme/BlobPutEmitter@1".to_string())
    {
        binding.slots.insert("default".into(), "blob_cap".into());
    }

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    let start_event = json!({
        "$tag": "Start",
        "$value": fixtures::start_event("blob-put"),
    });
    world
        .submit_event_result(event_schema, &start_event)
        .expect("submit start event");
    world.tick_n(1).unwrap();

    let mut effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 1);
    let intent = effects.remove(0);
    assert_eq!(intent.kind.as_str(), aos_effects::EffectKind::BLOB_PUT);

    let receipt = EffectReceipt {
        intent_hash: intent.intent_hash,
        adapter_id: "adapter.blob".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&BlobPutReceipt {
            blob_ref: fake_hash(0x21),
            edge_ref: fake_hash(0x22),
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
        Some(vec![0xDD])
    );
}

/// Blob.get receipts should similarly map into `sys/BlobGetResult@1`, route through the bus, and wrap at dispatch.
#[test]
#[ignore = "P2: plan-runtime integration fixture retired; replace with workflow-native coverage"]
fn blob_get_receipt_routes_event_to_handler() {
    let store = fixtures::new_mem_store();

    let mut emitter = fixtures::stub_reducer_module(
        &store,
        "com.acme/BlobGetEmitter@1",
        &ReducerOutput {
            state: None,
            domain_events: vec![],
            effects: vec![ReducerEffect::new(
                aos_effects::EffectKind::BLOB_GET,
                serde_cbor::to_vec(&BlobGetParams {
                    blob_ref: fake_hash(0x10),
                })
                .unwrap(),
            )],
            ann: None,
        },
    );

    let mut handler = fixtures::stub_reducer_module(
        &store,
        "com.acme/BlobGetHandler@1",
        &ReducerOutput {
            state: Some(vec![0xEE]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );

    let event_schema = "com.acme/BlobGetEvent@1";
    emitter.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema(START_SCHEMA),
        event: fixtures::schema(event_schema),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    handler.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema(START_SCHEMA),
        event: fixtures::schema(event_schema),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    let routing = vec![
        fixtures::routing_event(event_schema, &emitter.name),
        fixtures::routing_event(event_schema, &handler.name),
    ];
    let mut loaded = build_loaded_manifest_with_http_enforcer(
        &store,
        vec![],
        vec![],
        vec![emitter, handler],
        routing,
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: event_schema.into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(START_SCHEMA),
                            }),
                        ),
                        (
                            "GetResult".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("sys/BlobGetResult@1"),
                            }),
                        ),
                    ]),
                }),
            },
        ],
    );
    if let Some(binding) = loaded
        .manifest
        .module_bindings
        .get_mut(&"com.acme/BlobGetEmitter@1".to_string())
    {
        binding.slots.insert("default".into(), "blob_cap".into());
    }

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    let start_event = json!({
        "$tag": "Start",
        "$value": fixtures::start_event("blob-get"),
    });
    world
        .submit_event_result(event_schema, &start_event)
        .expect("submit start event");
    world.tick_n(1).unwrap();

    let mut effects = world.drain_effects().expect("drain effects");
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
            bytes: vec![0; 128],
        })
        .unwrap(),
        cost_cents: None,
        signature: vec![8, 8],
    };
    world.kernel.handle_receipt(receipt).unwrap();
    world.tick_n(1).unwrap();

    assert_eq!(
        world.kernel.reducer_state("com.acme/BlobGetHandler@1"),
        Some(vec![0xEE])
    );
}

/// Complex plan scenario: emit effect → await receipt → emit → await domain event → emit. Ensures
/// interleaving of effect receipts and raised events still produces deterministic progression.
#[test]
#[ignore = "P2: plan-runtime integration fixture retired; replace with workflow-native coverage"]
fn plan_waits_for_receipt_and_event_before_progressing() {
    let store = fixtures::new_mem_store();
    let plan_name = "com.acme/TwoStage@1".to_string();

    let mut next_emitter = fixtures::stub_event_emitting_reducer(
        &store,
        "com.acme/NextEmitter@1",
        vec![fixtures::domain_event(
            "com.acme/Next@1",
            &ExprValue::Int(1),
        )],
    );
    next_emitter.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/NextEmitterState@1"),
        event: fixtures::schema("com.acme/PulseNext@1"),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

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
                    idempotency_key: None,
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
                    idempotency_key: None,
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
                    idempotency_key: None,
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
    let mut loaded = build_loaded_manifest_with_http_enforcer(
        &store,
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![next_emitter],
        routing,
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            def_text_record_schema("com.acme/PlanIn@1", vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/PulseNext@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            DefSchema {
                name: "com.acme/Next@1".into(),
                ty: TypeExpr::Primitive(TypePrimitive::Int(TypePrimitiveInt {
                    int: EmptyObject::default(),
                })),
            },
            DefSchema {
                name: "com.acme/NextEmitterState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("two-stage"))
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let mut effects = world.drain_effects().expect("drain effects");
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

    let mut after_receipt_effects = world.drain_effects().expect("drain effects");
    assert_eq!(after_receipt_effects.len(), 1);
    let second_intent = after_receipt_effects.remove(0);
    assert!(
        effect_params_text(&second_intent).ends_with("after-receipt"),
        "unexpected params: {}",
        effect_params_text(&second_intent)
    );

    world
        .submit_event_result("com.acme/PulseNext@1", &serde_json::json!({}))
        .expect("submit pulse event");
    world.kernel.tick_until_idle().unwrap();

    let mut after_event_effects = world.drain_effects().expect("drain effects");
    assert_eq!(after_event_effects.len(), 1);
    assert!(
        effect_params_text(&after_event_effects.remove(0)).ends_with("after-event"),
        "unexpected params"
    );
}

#[test]
#[ignore = "P2: plan-runtime integration fixture retired; replace with workflow-native coverage"]
fn replay_does_not_double_apply_receipt_spawned_domain_events() {
    let store = fixtures::new_mem_store();
    let producer_plan = "com.acme/ReceiptToEvent@1";
    let consumer_plan = "com.acme/DoneConsumer@1";
    let done_schema = "com.acme/Done@1";

    let build_manifest = || {
        let producer = DefPlan {
            name: producer_plan.into(),
            input: fixtures::schema("com.acme/PlanIn@1"),
            output: None,
            locals: IndexMap::new(),
            steps: vec![
                PlanStep {
                    id: "emit".into(),
                    kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                        kind: EffectKind::http_request(),
                        params: http_params_literal("receipt-gate"),
                        cap: "cap_http".into(),
                        idempotency_key: None,
                        bind: PlanBindEffect {
                            effect_id_as: "req".into(),
                        },
                    }),
                },
                PlanStep {
                    id: "await".into(),
                    kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                        for_expr: fixtures::var_expr("req"),
                        bind: PlanBind {
                            var: "receipt".into(),
                        },
                    }),
                },
                PlanStep {
                    id: "raise".into(),
                    kind: PlanStepKind::RaiseEvent(PlanStepRaiseEvent {
                        event: fixtures::schema(done_schema),
                        value: Expr::Ref(ExprRef {
                            reference: "@plan.input".into(),
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

        let consumer = DefPlan {
            name: consumer_plan.into(),
            input: fixtures::schema("com.acme/PlanIn@1"),
            output: None,
            locals: IndexMap::new(),
            steps: vec![
                PlanStep {
                    id: "emit".into(),
                    kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                        kind: EffectKind::http_request(),
                        params: http_params_literal("done-consumer"),
                        cap: "cap_http".into(),
                        idempotency_key: None,
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

        let mut loaded = build_loaded_manifest_with_http_enforcer(
            &store,
            vec![producer, consumer],
            vec![
                fixtures::start_trigger(producer_plan),
                Trigger {
                    event: fixtures::schema(done_schema),
                    plan: consumer_plan.into(),
                    correlate_by: None,
                    when: None,
                    input_expr: None,
                },
            ],
            vec![],
            vec![],
        );
        insert_test_schemas(
            &mut loaded,
            vec![
                def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
                def_text_record_schema("com.acme/PlanIn@1", vec![("id", text_type())]),
                def_text_record_schema(done_schema, vec![("id", text_type())]),
            ],
        );
        loaded
    };

    let mut world = TestWorld::with_store(store.clone(), build_manifest()).unwrap();
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("replay-once"))
        .expect("submit start event");
    world.kernel.tick_until_idle().unwrap();
    let mut initial_effects = world.drain_effects().expect("drain first effect");
    assert_eq!(initial_effects.len(), 1);
    assert!(effect_params_text(&initial_effects[0]).ends_with("receipt-gate"));

    let receipt = EffectReceipt {
        intent_hash: initial_effects.remove(0).intent_hash,
        adapter_id: "adapter.http".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&ExprValue::Text("ok".into())).unwrap(),
        cost_cents: None,
        signature: vec![],
    };
    world.kernel.handle_receipt(receipt).unwrap();
    world.kernel.tick_until_idle().unwrap();
    let mut downstream_effects = world.drain_effects().expect("drain downstream effect");
    assert_eq!(downstream_effects.len(), 1);
    assert!(effect_params_text(&downstream_effects.remove(0)).ends_with("done-consumer"));

    let journal_entries = world.kernel.dump_journal().unwrap();
    let store_for_replay = store.clone();
    let mut replay_world = TestWorld::with_store_and_journal(
        store_for_replay,
        build_manifest(),
        Box::new(MemJournal::from_entries(&journal_entries)),
    )
    .unwrap();

    let replay_effects = replay_world.drain_effects().expect("drain replay effects");
    let done_consumer_count = replay_effects
        .iter()
        .filter(|intent| effect_params_text(intent).ends_with("done-consumer"))
        .count();
    assert_eq!(
        done_consumer_count, 1,
        "done-trigger consumer should replay once"
    );
}

/// Plans blocked on `await_event` should only resume when the subscribed schema fires; different
/// schemas should remain pending even if their triggers fire later.
#[test]
#[ignore = "P2: plan-runtime integration fixture retired; replace with workflow-native coverage"]
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
                    idempotency_key: None,
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
                    idempotency_key: None,
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

    let mut ready_emitter = fixtures::stub_event_emitting_reducer(
        &store,
        "com.acme/ReadyEmitter@1",
        vec![fixtures::domain_event(
            "com.acme/Ready@1",
            &ExprValue::Nat(7),
        )],
    );
    ready_emitter.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/ReadyEmitterState@1"),
        event: fixtures::schema("com.acme/TriggerReady@1"),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let mut other_emitter = fixtures::stub_event_emitting_reducer(
        &store,
        "com.acme/OtherEmitter@1",
        vec![fixtures::domain_event(
            "com.acme/Other@1",
            &ExprValue::Nat(9),
        )],
    );
    other_emitter.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/OtherEmitterState@1"),
        event: fixtures::schema("com.acme/TriggerOther@1"),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let mut loaded = build_loaded_manifest_with_http_enforcer(
        &store,
        vec![plan_ready.clone(), plan_other.clone()],
        vec![
            fixtures::start_trigger(&plan_ready.name),
            fixtures::start_trigger(&plan_other.name),
        ],
        vec![ready_emitter, other_emitter],
        vec![
            fixtures::routing_event("com.acme/TriggerReady@1", "com.acme/ReadyEmitter@1"),
            fixtures::routing_event("com.acme/TriggerOther@1", "com.acme/OtherEmitter@1"),
        ],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            def_text_record_schema("com.acme/TriggerReady@1", vec![]),
            def_text_record_schema("com.acme/TriggerOther@1", vec![]),
            DefSchema {
                name: "com.acme/Ready@1".into(),
                ty: TypeExpr::Primitive(TypePrimitive::Nat(TypePrimitiveNat {
                    nat: EmptyObject::default(),
                })),
            },
            DefSchema {
                name: "com.acme/Other@1".into(),
                ty: TypeExpr::Primitive(TypePrimitive::Nat(TypePrimitiveNat {
                    nat: EmptyObject::default(),
                })),
            },
            DefSchema {
                name: "com.acme/ReadyEmitterState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            DefSchema {
                name: "com.acme/OtherEmitterState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("ready-other"))
        .expect("submit start event");
    world.tick_n(2).unwrap();
    assert!(world.drain_effects().expect("drain effects").is_empty());

    world
        .submit_event_result("com.acme/TriggerReady@1", &serde_json::json!({}))
        .expect("submit ready trigger");
    world.kernel.tick_until_idle().unwrap();
    let mut effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 1);
    assert!(effect_params_text(&effects.remove(0)).ends_with("ready"));

    world
        .submit_event_result("com.acme/TriggerOther@1", &serde_json::json!({}))
        .expect("submit other trigger");
    world.kernel.tick_until_idle().unwrap();
    let mut more_effects = world.drain_effects().expect("drain effects");
    assert_eq!(more_effects.len(), 1);
    assert!(effect_params_text(&more_effects.remove(0)).ends_with("other"));
}






#[test]
#[ignore = "P2: plan-runtime integration fixture retired; replace with workflow-native coverage"]
fn correlated_await_event_prevents_cross_talk_between_instances() {
    let store = fixtures::new_mem_store();
    let plan_name = "com.acme/CorrelatedWait@1";

    let plan = DefPlan {
        name: plan_name.into(),
        input: fixtures::schema("com.acme/PlanIn@1"),
        output: None,
        locals: IndexMap::new(),
        steps: vec![
            PlanStep {
                id: "await_response".into(),
                kind: PlanStepKind::AwaitEvent(PlanStepAwaitEvent {
                    event: fixtures::schema("com.acme/Response@1"),
                    where_clause: Some(Expr::Op(ExprOp {
                        op: ExprOpCode::Eq,
                        args: vec![
                            Expr::Ref(ExprRef {
                                reference: "@event.correlation_id".into(),
                            }),
                            Expr::Ref(ExprRef {
                                reference: "@var:correlation_id".into(),
                            }),
                        ],
                    })),
                    bind: PlanBind {
                        var: "response".into(),
                    },
                }),
            },
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: Expr::Record(ExprRecord {
                        record: IndexMap::from([
                            (
                                "method".into(),
                                Expr::Const(ExprConst::Text { text: "GET".into() }),
                            ),
                            (
                                "url".into(),
                                Expr::Op(ExprOp {
                                    op: ExprOpCode::Get,
                                    args: vec![
                                        Expr::Ref(ExprRef {
                                            reference: "@var:response".into(),
                                        }),
                                        Expr::Const(ExprConst::Text {
                                            text: "reply".into(),
                                        }),
                                    ],
                                }),
                            ),
                            ("headers".into(), Expr::Map(ExprMap { map: vec![] })),
                            (
                                "body_ref".into(),
                                Expr::Const(ExprConst::Null {
                                    null: EmptyObject::default(),
                                }),
                            ),
                        ]),
                    })
                    .into(),
                    cap: "cap_http".into(),
                    idempotency_key: None,
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
                from: "await_response".into(),
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

    let trigger = Trigger {
        event: fixtures::schema(START_SCHEMA),
        plan: plan_name.into(),
        correlate_by: Some("id".into()),
        when: None,
        input_expr: None,
    };
    let mut loaded =
        build_loaded_manifest_with_http_enforcer(&store, vec![plan], vec![trigger], vec![], vec![]);
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            def_text_record_schema("com.acme/PlanIn@1", vec![("id", text_type())]),
            def_text_record_schema(
                "com.acme/Response@1",
                vec![("correlation_id", text_type()), ("reply", text_type())],
            ),
        ],
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("a"))
        .expect("submit start a");
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("b"))
        .expect("submit start b");
    world.kernel.tick_until_idle().unwrap();
    assert!(world.drain_effects().expect("drain effects").is_empty());

    world
        .submit_event_result(
            "com.acme/Response@1",
            &json!({"correlation_id": "b", "reply": "reply-b"}),
        )
        .expect("submit response b");
    world.kernel.tick_until_idle().unwrap();
    let mut effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 1);
    assert_eq!(effect_params_text(&effects.remove(0)), "reply-b");

    world
        .submit_event_result(
            "com.acme/Response@1",
            &json!({"correlation_id": "a", "reply": "reply-a"}),
        )
        .expect("submit response a");
    world.kernel.tick_until_idle().unwrap();
    let mut effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 1);
    assert_eq!(effect_params_text(&effects.remove(0)), "reply-a");
}

#[test]
#[ignore = "P2: plan-runtime integration fixture retired; replace with workflow-native coverage"]
fn subplan_receipt_wait_survives_restart_and_resumes_parent() {
    let store = fixtures::new_mem_store();
    let parent_plan = "com.acme/ParentResume@1";
    let child_plan = "com.acme/ChildResume@1";

    let child = DefPlan {
        name: child_plan.into(),
        input: fixtures::schema("com.acme/PlanIn@1"),
        output: Some(fixtures::schema("com.acme/ChildOut@1")),
        locals: IndexMap::new(),
        steps: vec![
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: Expr::Record(ExprRecord {
                        record: IndexMap::from([
                            (
                                "method".into(),
                                Expr::Const(ExprConst::Text { text: "GET".into() }),
                            ),
                            (
                                "url".into(),
                                Expr::Ref(ExprRef {
                                    reference: "@plan.input.id".into(),
                                }),
                            ),
                            ("headers".into(), Expr::Map(ExprMap { map: vec![] })),
                            (
                                "body_ref".into(),
                                Expr::Const(ExprConst::Null {
                                    null: EmptyObject::default(),
                                }),
                            ),
                        ]),
                    })
                    .into(),
                    cap: "cap_http".into(),
                    idempotency_key: None,
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "await".into(),
                kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                    for_expr: Expr::Ref(ExprRef {
                        reference: "@var:req".into(),
                    }),
                    bind: PlanBind {
                        var: "receipt".into(),
                    },
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd {
                    result: Some(
                        Expr::Record(ExprRecord {
                            record: IndexMap::from([(
                                "value".into(),
                                Expr::Ref(ExprRef {
                                    reference: "@plan.input.id".into(),
                                }),
                            )]),
                        })
                        .into(),
                    ),
                }),
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
                to: "end".into(),
                when: None,
            },
        ],
        required_caps: vec!["cap_http".into()],
        allowed_effects: vec![EffectKind::http_request()],
        invariants: vec![],
    };

    let parent = DefPlan {
        name: parent_plan.into(),
        input: fixtures::schema("com.acme/PlanIn@1"),
        output: None,
        locals: IndexMap::new(),
        steps: vec![
            PlanStep {
                id: "spawn".into(),
                kind: PlanStepKind::SpawnPlan(PlanStepSpawnPlan {
                    plan: child_plan.into(),
                    input: Expr::Ref(ExprRef {
                        reference: "@plan.input".into(),
                    })
                    .into(),
                    bind: PlanBindHandle {
                        handle_as: "child".into(),
                    },
                }),
            },
            PlanStep {
                id: "await".into(),
                kind: PlanStepKind::AwaitPlan(PlanStepAwaitPlan {
                    for_expr: Expr::Ref(ExprRef {
                        reference: "@var:child".into(),
                    }),
                    bind: PlanBind {
                        var: "child_result".into(),
                    },
                }),
            },
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: Expr::Record(ExprRecord {
                        record: IndexMap::from([
                            (
                                "method".into(),
                                Expr::Const(ExprConst::Text { text: "GET".into() }),
                            ),
                            (
                                "url".into(),
                                Expr::Op(ExprOp {
                                    op: ExprOpCode::Get,
                                    args: vec![
                                        Expr::Op(ExprOp {
                                            op: ExprOpCode::Get,
                                            args: vec![
                                                Expr::Ref(ExprRef {
                                                    reference: "@var:child_result".into(),
                                                }),
                                                Expr::Const(ExprConst::Text {
                                                    text: "$value".into(),
                                                }),
                                            ],
                                        }),
                                        Expr::Const(ExprConst::Text {
                                            text: "value".into(),
                                        }),
                                    ],
                                }),
                            ),
                            ("headers".into(), Expr::Map(ExprMap { map: vec![] })),
                            (
                                "body_ref".into(),
                                Expr::Const(ExprConst::Null {
                                    null: EmptyObject::default(),
                                }),
                            ),
                        ]),
                    })
                    .into(),
                    cap: "cap_http".into(),
                    idempotency_key: None,
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
                from: "spawn".into(),
                to: "await".into(),
                when: None,
            },
            PlanEdge {
                from: "await".into(),
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

    let mut loaded = build_loaded_manifest_with_http_enforcer(
        &store,
        vec![child.clone(), parent.clone()],
        vec![fixtures::start_trigger(parent_plan)],
        vec![],
        vec![],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            def_text_record_schema("com.acme/PlanIn@1", vec![("id", text_type())]),
            def_text_record_schema("com.acme/ChildOut@1", vec![("value", text_type())]),
        ],
    );

    let mut world = TestWorld::with_store(store.clone(), loaded).unwrap();
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("resume-1"))
        .expect("submit start");
    world.kernel.tick_until_idle().unwrap();

    let mut effects = world.drain_effects().expect("drain child effect");
    assert_eq!(effects.len(), 1);
    let child_effect = effects.remove(0);
    assert_eq!(effect_params_text(&child_effect), "resume-1");

    world.kernel.create_snapshot().unwrap();
    let journal_entries = world.kernel.dump_journal().unwrap();

    let mut replay_loaded = build_loaded_manifest_with_http_enforcer(
        &store,
        vec![child, parent],
        vec![fixtures::start_trigger(parent_plan)],
        vec![],
        vec![],
    );
    insert_test_schemas(
        &mut replay_loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            def_text_record_schema("com.acme/PlanIn@1", vec![("id", text_type())]),
            def_text_record_schema("com.acme/ChildOut@1", vec![("value", text_type())]),
        ],
    );

    let mut replay_world = TestWorld::with_store_and_journal(
        store,
        replay_loaded,
        Box::new(MemJournal::from_entries(&journal_entries)),
    )
    .unwrap();
    let mut replay_queued = replay_world.drain_effects().expect("drain replay queue");
    assert_eq!(replay_queued.len(), 1);
    assert_eq!(effect_params_text(&replay_queued.remove(0)), "resume-1");

    let receipt = EffectReceipt {
        intent_hash: child_effect.intent_hash,
        adapter_id: "adapter.http".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&ExprValue::Text("done".into())).unwrap(),
        cost_cents: None,
        signature: vec![],
    };
    replay_world.kernel.handle_receipt(receipt).unwrap();
    replay_world.kernel.tick_until_idle().unwrap();

    let mut resumed_effects = replay_world.drain_effects().expect("drain parent effect");
    assert_eq!(resumed_effects.len(), 1);
    assert_eq!(effect_params_text(&resumed_effects.remove(0)), "resume-1");

    let summary = plan_run_summary(&replay_world.kernel).expect("plan summary");
    assert_eq!(summary["totals"]["runs"]["started"], 2);
    assert_eq!(summary["totals"]["runs"]["ok"], 2);
}



/// Plans that raise events should deliver them to reducers according to manifest routing.
#[test]
#[ignore = "P2: plan-runtime integration fixture retired; replace with workflow-native coverage"]
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
        context: Some(fixtures::schema("sys/ReducerContext@1")),
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
                    event: fixtures::schema("com.acme/Raised@1"),
                    value: Expr::Record(ExprRecord {
                        record: IndexMap::from([(
                            "value".into(),
                            Expr::Const(ExprConst::Int { int: 9 }),
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
        .submit_event_result(START_SCHEMA, &fixtures::start_event("raise"))
        .expect("submit start event");
    world.kernel.tick_until_idle().unwrap();

    assert_eq!(
        world.kernel.reducer_state("com.acme/Reducer@1"),
        Some(vec![0xEE])
    );
}
