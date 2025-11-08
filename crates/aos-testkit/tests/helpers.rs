//! Shared test helpers for integration tests.
//!
//! These helpers build test manifests and utilities used across multiple integration test files.
//! Note: Each integration test compiles this module separately, so some functions may appear
//! unused in certain test contexts but are used by others.

#![allow(dead_code)]

use aos_air_types::{
    DefPlan, EffectKind, Expr, ExprConst, ExprRecord, PlanBind, PlanBindEffect, PlanEdge,
    PlanStep, PlanStepAwaitEvent, PlanStepAwaitReceipt, PlanStepEmitEffect,
    PlanStepEnd, PlanStepKind, PlanStepRaiseEvent,
};
use aos_effects::builtins::TimerSetParams;
use aos_testkit::fixtures::{self, START_SCHEMA};
use aos_testkit::TestStore;
use aos_wasm_abi::{ReducerEffect, ReducerOutput};
use indexmap::IndexMap;
use std::sync::Arc;

/// Builds a test manifest with a plan that emits an HTTP effect, awaits its receipt,
/// and raises an event to a result reducer.
pub fn fulfillment_manifest(store: &Arc<TestStore>) -> aos_kernel::manifest::LoadedManifest {
    let result_module = fixtures::stub_reducer_module(
        store,
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
    fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![result_module],
        routing,
    )
}

/// Builds a test manifest with a plan that awaits a domain event before proceeding.
pub fn await_event_manifest(store: &Arc<TestStore>) -> aos_kernel::manifest::LoadedManifest {
    let result_module = fixtures::stub_reducer_module(
        store,
        "com.acme/EventResult@1",
        &ReducerOutput {
            state: Some(vec![0xAB]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    let unblock_event = fixtures::domain_event(
        "com.acme/Unblock@1",
        &fixtures::plan_input_record(vec![]),
    );
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
                        record: IndexMap::from([
                            (
                                "$schema".into(),
                                fixtures::text_expr("com.acme/EventDone@1"),
                            ),
                            ("value".into(), Expr::Const(ExprConst::Int { int: 5 })),
                        ]),
                    }),
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
        fixtures::routing_event("com.acme/EventDone@1", &result_module.name),
        fixtures::routing_event("com.acme/EmitUnblock@1", &unblock_emitter.name),
    ];
    fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![result_module, unblock_emitter],
        routing,
    )
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
    let timer_emitter =
        fixtures::stub_reducer_module(store, "com.acme/TimerEmitter@1", &timer_output);

    let handler_output = ReducerOutput {
        state: Some(vec![0xCC]),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };
    let timer_handler =
        fixtures::stub_reducer_module(store, "com.acme/TimerHandler@1", &handler_output);

    let routing = vec![
        fixtures::routing_event(fixtures::SYS_TIMER_FIRED, &timer_handler.name),
        fixtures::routing_event(START_SCHEMA, &timer_emitter.name),
    ];
    fixtures::build_loaded_manifest(vec![], vec![], vec![timer_emitter, timer_handler], routing)
}
