use aos_air_exec::Value as ExprValue;
use aos_air_types::{
    DefPlan, EffectKind, Expr, ExprConst, ExprRecord, PlanBind, PlanBindEffect, PlanEdge, PlanStep,
    PlanStepAssign, PlanStepAwaitReceipt, PlanStepEmitEffect, PlanStepEnd, PlanStepKind,
    PlanStepRaiseEvent,
};
use aos_effects::builtins::{TimerSetParams, TimerSetReceipt};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_testkit::TestWorld;
use aos_testkit::fixtures::{self, START_SCHEMA};
use aos_wasm_abi::{ReducerEffect, ReducerOutput};
use indexmap::IndexMap;
use serde_cbor;

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
}

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
