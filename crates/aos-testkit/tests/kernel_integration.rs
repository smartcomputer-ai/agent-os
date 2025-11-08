use aos_air_exec::Value as ExprValue;
use aos_air_types::{
    DefPlan, EffectKind, Expr, ExprConst, ExprRecord, PlanBind, PlanBindEffect, PlanEdge, PlanStep,
    PlanStepAssign, PlanStepAwaitEvent, PlanStepAwaitReceipt, PlanStepEmitEffect, PlanStepEnd,
    PlanStepKind, PlanStepRaiseEvent,
};
use aos_effects::builtins::{TimerSetParams, TimerSetReceipt};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_kernel::error::KernelError;
use aos_testkit::fixtures::{self, START_SCHEMA};
use aos_testkit::{TestWorld, effect_params_text};
use aos_wasm_abi::{ReducerEffect, ReducerOutput};
use indexmap::IndexMap;
use serde_cbor;

/// Happy-path end-to-end: reducer emits an intent, plan does work, receipt feeds a result event
/// back into the reducer. Mirrors the “single plan orchestration” pattern in the spec.
#[test]
fn single_plan_orchestration_completes_after_receipt() {
    let store = fixtures::new_mem_store();

    let result_module = fixtures::stub_reducer_module(
        &store,
        "com.acme/ResultReducer@1",
        &ReducerOutput {
            state: Some(vec![0xEE]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );

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
                    kind: EffectKind::HttpRequest,
                    params: fixtures::text_expr("body"),
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
                        record: IndexMap::from([
                            ("$schema".into(), fixtures::text_expr("com.acme/Result@1")),
                            ("value".into(), Expr::Const(ExprConst::Int { int: 9 })),
                        ]),
                    }),
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
        allowed_effects: vec![EffectKind::HttpRequest],
        invariants: vec![],
    };

    let routing = vec![fixtures::routing_event(
        "com.acme/Result@1",
        &result_module.name,
    )];
    let loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![result_module],
        routing,
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    let input = fixtures::plan_input_record(vec![("id", ExprValue::Text("123".into()))]);
    world.submit_event_value(START_SCHEMA, &input);
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
    world.tick_n(3).unwrap();

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
                    kind: EffectKind::HttpRequest,
                    params: fixtures::text_expr("plan"),
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
        allowed_effects: vec![EffectKind::HttpRequest],
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
    world.submit_event_value(START_SCHEMA, &input);
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
    let timer_emitter =
        fixtures::stub_reducer_module(&store, "com.acme/TimerEmitter@1", &timer_output);

    let handler_output = ReducerOutput {
        state: Some(vec![0xCC]),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };
    let timer_handler =
        fixtures::stub_reducer_module(&store, "com.acme/TimerHandler@1", &handler_output);

    let routing = vec![
        fixtures::routing_event(fixtures::SYS_TIMER_FIRED, &timer_handler.name),
        fixtures::routing_event(START_SCHEMA, &timer_emitter.name),
    ];
    let loaded = fixtures::build_loaded_manifest(
        vec![],
        vec![],
        vec![timer_emitter, timer_handler],
        routing,
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world.submit_event_value(START_SCHEMA, &fixtures::plan_input_record(vec![]));
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
                    expr: fixtures::plan_input_expr("flag"),
                    bind: PlanBind { var: "flag".into() },
                }),
            },
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::HttpRequest,
                    params: fixtures::text_expr("do it"),
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
        allowed_effects: vec![EffectKind::HttpRequest],
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
    world.submit_event_value(START_SCHEMA, &true_input);
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
    world_false.submit_event_value(START_SCHEMA, &false_input);
    world_false.tick_n(2).unwrap();
    assert_eq!(world_false.drain_effects().len(), 0);
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
                    kind: EffectKind::HttpRequest,
                    params: fixtures::text_expr("first"),
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
                    kind: EffectKind::HttpRequest,
                    params: fixtures::text_expr("after_receipt"),
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
                    kind: EffectKind::HttpRequest,
                    params: fixtures::text_expr("after_event"),
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
        allowed_effects: vec![EffectKind::HttpRequest],
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
    world.submit_event_value(START_SCHEMA, &fixtures::plan_input_record(vec![]));
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
    assert_eq!(effect_params_text(&second_intent), "after_receipt");

    world.submit_event_value("com.acme/PulseNext@1", &fixtures::plan_input_record(vec![]));
    world.tick_n(3).unwrap();

    let mut after_event_effects = world.drain_effects();
    assert_eq!(after_event_effects.len(), 1);
    assert_eq!(
        effect_params_text(&after_event_effects.remove(0)),
        "after_event"
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
                    kind: EffectKind::HttpRequest,
                    params: fixtures::text_expr("ready"),
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
        allowed_effects: vec![EffectKind::HttpRequest],
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
                    kind: EffectKind::HttpRequest,
                    params: fixtures::text_expr("other"),
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
        allowed_effects: vec![EffectKind::HttpRequest],
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
    world.submit_event_value(START_SCHEMA, &fixtures::plan_input_record(vec![]));
    world.tick_n(2).unwrap();
    assert!(world.drain_effects().is_empty());

    world.submit_event_value(
        "com.acme/TriggerReady@1",
        &fixtures::plan_input_record(vec![]),
    );
    world.tick_n(3).unwrap();
    let mut effects = world.drain_effects();
    assert_eq!(effects.len(), 1);
    assert_eq!(effect_params_text(&effects.remove(0)), "ready");

    world.submit_event_value(
        "com.acme/TriggerOther@1",
        &fixtures::plan_input_record(vec![]),
    );
    world.tick_n(3).unwrap();
    let mut more_effects = world.drain_effects();
    assert_eq!(more_effects.len(), 1);
    assert_eq!(effect_params_text(&more_effects.remove(0)), "other");
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
    let reducer_module =
        fixtures::stub_reducer_module(&store, "com.acme/Reducer@1", &reducer_output);

    let plan = DefPlan {
        name: "com.acme/Raise@1".into(),
        input: fixtures::schema("com.acme/PlanIn@1"),
        output: None,
        locals: IndexMap::new(),
        steps: vec![
            PlanStep {
                id: "raise".into(),
                kind: PlanStepKind::RaiseEvent(PlanStepRaiseEvent {
                    reducer: reducer_module.name.clone(),
                    event: Expr::Record(ExprRecord {
                        record: IndexMap::from([
                            ("$schema".into(), fixtures::text_expr("com.acme/Raised@1")),
                            ("value".into(), Expr::Const(ExprConst::Int { int: 9 })),
                        ]),
                    }),
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

    let routing = vec![fixtures::routing_event(
        "com.acme/Raised@1",
        &reducer_module.name,
    )];
    let loaded = fixtures::build_loaded_manifest(
        vec![plan.clone()],
        vec![fixtures::start_trigger(&plan.name)],
        vec![reducer_module],
        routing,
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world.submit_event_value(START_SCHEMA, &fixtures::plan_input_record(vec![]));
    world.tick_n(3).unwrap();

    assert_eq!(
        world.kernel.reducer_state("com.acme/Reducer@1"),
        Some(&vec![0xEE])
    );
}
