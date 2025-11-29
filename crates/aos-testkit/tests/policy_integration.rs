use aos_air_exec::Value as ExprValue;
use aos_air_types::{
    DefPolicy, EffectKind as AirEffectKind, EmptyObject, Expr, ExprConst, ExprMap, ExprOrValue,
    ExprRecord, ExprRef, OriginKind, PolicyDecision, PolicyMatch, PolicyRule, ValueLiteral,
    ValueMap, ValueNull, ValueRecord, ValueText,
};
use aos_effects::builtins::HttpRequestParams;
use aos_kernel::error::KernelError;
use aos_testkit::TestWorld;
use aos_testkit::fixtures::{self, zero_hash};
use aos_wasm_abi::ReducerEffect;
use indexmap::IndexMap;

mod helpers;
use helpers::attach_default_policy;

fn http_reducer_output() -> aos_wasm_abi::ReducerOutput {
    let mut headers = IndexMap::new();
    headers.insert("x-test".into(), "1".into());
    let params = HttpRequestParams {
        method: "POST".into(),
        url: "https://example.com".into(),
        headers,
        body_ref: Some(zero_hash()),
    };
    aos_wasm_abi::ReducerOutput {
        state: None,
        domain_events: vec![],
        effects: vec![ReducerEffect::new(
            aos_effects::EffectKind::HTTP_REQUEST,
            serde_cbor::to_vec(&params).unwrap(),
        )],
        ann: None,
    }
}

#[test]
fn reducer_http_effect_is_denied() {
    let store = fixtures::new_mem_store();
    let reducer_name = "com.acme/HttpReducer@1".to_string();
    let reducer = fixtures::stub_reducer_module(&store, &reducer_name, &http_reducer_output());

    let routing = vec![fixtures::routing_event(
        fixtures::START_SCHEMA,
        &reducer_name,
    )];
    let mut loaded = fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing);
    // Bind the reducer's default slot to the HTTP capability grant.
    if let Some(binding) = loaded.manifest.module_bindings.get_mut(&reducer_name) {
        binding.slots.insert("default".into(), "cap_http".into());
    }

    let policy = DefPolicy {
        name: "com.acme/policy@1".into(),
        rules: vec![PolicyRule {
            when: PolicyMatch {
                effect_kind: Some(AirEffectKind::http_request()),
                origin_kind: Some(OriginKind::Reducer),
                ..Default::default()
            },
            decision: PolicyDecision::Deny,
        }],
    };
    attach_default_policy(&mut loaded, policy);

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world.submit_event_value(fixtures::START_SCHEMA, &fixtures::plan_input_record(vec![]));
    let err = world.kernel.tick().unwrap_err();
    assert!(matches!(err, KernelError::UnsupportedReducerReceipt(_)), "unexpected error: {err:?}");
}

#[test]
fn plan_effect_allowed_by_policy() {
    let store = fixtures::new_mem_store();
    let plan_name = "com.acme/Plan@1".to_string();
    let plan = aos_air_types::DefPlan {
        name: plan_name.clone(),
        input: fixtures::schema("com.acme/Input@1"),
        output: None,
        locals: IndexMap::new(),
        steps: vec![aos_air_types::PlanStep {
            id: "emit".into(),
            kind: aos_air_types::PlanStepKind::EmitEffect(aos_air_types::PlanStepEmitEffect {
                kind: AirEffectKind::http_request(),
                params: http_params_literal("https://example.com"),
                cap: "cap_http".into(),
                bind: aos_air_types::PlanBindEffect {
                    effect_id_as: "req".into(),
                },
            }),
        }],
        edges: vec![],
        required_caps: vec!["cap_http".into()],
        allowed_effects: vec![AirEffectKind::http_request()],
        invariants: vec![],
    };

    let mut loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![],
        vec![],
    );

    let policy = DefPolicy {
        name: "com.acme/plan-policy@1".into(),
        rules: vec![PolicyRule {
            when: PolicyMatch {
                effect_kind: Some(AirEffectKind::http_request()),
                origin_kind: Some(OriginKind::Plan),
                ..Default::default()
            },
            decision: PolicyDecision::Allow,
        }],
    };
    attach_default_policy(&mut loaded, policy);

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    let input = fixtures::plan_input_record(vec![("foo", ExprValue::Nat(1))]);
    world.submit_event_value(fixtures::START_SCHEMA, &input);
    world.tick_n(2).unwrap();
    assert_eq!(world.drain_effects().len(), 1);
}

#[test]
fn plan_effect_expr_params_are_evaluated_and_allowed() {
    let store = fixtures::new_mem_store();
    let plan_name = "com.acme/PlanExpr@1".to_string();
    let params_expr = Expr::Record(ExprRecord {
        record: IndexMap::from([
            (
                "method".into(),
                Expr::Const(ExprConst::Text { text: "GET".into() }),
            ),
            (
                "url".into(),
                Expr::Ref(ExprRef {
                    reference: "@plan.input.url".into(),
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
    });
    let plan = aos_air_types::DefPlan {
        name: plan_name.clone(),
        input: fixtures::schema("com.acme/Input@1"),
        output: None,
        locals: IndexMap::new(),
        steps: vec![
            aos_air_types::PlanStep {
                id: "emit".into(),
                kind: aos_air_types::PlanStepKind::EmitEffect(aos_air_types::PlanStepEmitEffect {
                    kind: AirEffectKind::http_request(),
                    params: ExprOrValue::Expr(params_expr),
                    cap: "cap_http".into(),
                    bind: aos_air_types::PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            aos_air_types::PlanStep {
                id: "end".into(),
                kind: aos_air_types::PlanStepKind::End(aos_air_types::PlanStepEnd { result: None }),
            },
        ],
        edges: vec![aos_air_types::PlanEdge {
            from: "emit".into(),
            to: "end".into(),
            when: None,
        }],
        required_caps: vec!["cap_http".into()],
        allowed_effects: vec![AirEffectKind::http_request()],
        invariants: vec![],
    };

    let mut loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![],
        vec![],
    );

    let policy = DefPolicy {
        name: "com.acme/plan-policy@1".into(),
        rules: vec![PolicyRule {
            when: PolicyMatch {
                effect_kind: Some(AirEffectKind::http_request()),
                origin_kind: Some(OriginKind::Plan),
                ..Default::default()
            },
            decision: PolicyDecision::Allow,
        }],
    };
    attach_default_policy(&mut loaded, policy);

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    let input =
        fixtures::plan_input_record(vec![("url", ExprValue::Text("https://example.com".into()))]);
    world.submit_event_value(fixtures::START_SCHEMA, &input);
    world.tick_n(2).unwrap();
    let effects = world.drain_effects();
    assert_eq!(effects.len(), 1);
    let intent = &effects[0];
    assert_eq!(intent.kind.as_str(), aos_effects::EffectKind::HTTP_REQUEST);
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
