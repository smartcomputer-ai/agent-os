use aos_air_types::{
    DefPolicy, DefSchema, EffectKind as AirEffectKind, EmptyObject, Expr, ExprConst, ExprMap,
    ExprOrValue, ExprRecord, ExprRef, OriginKind, PolicyDecision, PolicyMatch, PolicyRule,
    ReducerAbi, ValueLiteral, ValueMap, ValueNull, ValueRecord, ValueText,
};
use aos_effects::builtins::HttpRequestParams;
use aos_kernel::cap_enforcer::CapCheckOutput;
use aos_kernel::error::KernelError;
use aos_wasm_abi::{PureOutput, ReducerEffect};
use helpers::fixtures::{self, TestStore, TestWorld, zero_hash};
use indexmap::IndexMap;

mod helpers;
use helpers::{attach_default_policy, def_text_record_schema, text_type};

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
    let mut reducer = fixtures::stub_reducer_module(&store, &reducer_name, &http_reducer_output());
    reducer.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/HttpState@1"),
        event: fixtures::schema(fixtures::START_SCHEMA),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::HTTP_REQUEST.into()],
        cap_slots: Default::default(),
    });

    let routing = vec![fixtures::routing_event(
        fixtures::START_SCHEMA,
        &reducer_name,
    )];
    let mut loaded = fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing);
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(fixtures::START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/HttpState@1".into(),
                ty: text_type(),
            },
        ],
    );
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
    world
        .submit_event_result(
            fixtures::START_SCHEMA,
            &serde_json::json!({ "id": "start" }),
        )
        .expect("submit start event");
    let err = world.kernel.tick().unwrap_err();
    assert!(
        matches!(err, KernelError::UnsupportedReducerReceipt(_)),
        "unexpected error: {err:?}"
    );
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
                idempotency_key: None,
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

    let enforcer = allow_http_enforcer(&store);
    let mut loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![enforcer],
        vec![],
    );
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(
                fixtures::START_SCHEMA,
                vec![("id", text_type()), ("url", text_type())],
            ),
            def_text_record_schema(
                "com.acme/Input@1",
                vec![("id", text_type()), ("url", text_type())],
            ),
        ],
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
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(fixtures::START_SCHEMA, vec![("id", text_type())]),
            def_text_record_schema("com.acme/Input@1", vec![("id", text_type())]),
        ],
    );
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(
                fixtures::START_SCHEMA,
                vec![("id", text_type()), ("url", text_type())],
            ),
            def_text_record_schema(
                "com.acme/Input@1",
                vec![("id", text_type()), ("url", text_type())],
            ),
        ],
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    let input = serde_json::json!({ "id": "123", "url": "https://example.com" });
    world
        .submit_event_result(fixtures::START_SCHEMA, &input)
        .expect("submit start event");
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
                    idempotency_key: None,
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

    let enforcer = allow_http_enforcer(&store);
    let mut loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![enforcer],
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
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(
                fixtures::START_SCHEMA,
                vec![("id", text_type()), ("url", text_type())],
            ),
            def_text_record_schema(
                "com.acme/Input@1",
                vec![("id", text_type()), ("url", text_type())],
            ),
        ],
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    let input = serde_json::json!({ "id": "expr", "url": "https://example.com" });
    world
        .submit_event_result(fixtures::START_SCHEMA, &input)
        .expect("submit start event");
    world.tick_n(2).unwrap();
    let effects = world.drain_effects();
    assert_eq!(effects.len(), 1);
    let intent = &effects[0];
    assert_eq!(intent.kind.as_str(), aos_effects::EffectKind::HTTP_REQUEST);
}

#[test]
fn plan_introspect_denied_by_policy() {
    let store = fixtures::new_mem_store();
    let plan_name = "com.acme/IntrospectPlan@1".to_string();
    let plan = aos_air_types::DefPlan {
        name: plan_name.clone(),
        input: fixtures::schema(fixtures::START_SCHEMA),
        output: None,
        locals: IndexMap::new(),
        steps: vec![aos_air_types::PlanStep {
            id: "emit".into(),
            kind: aos_air_types::PlanStepKind::EmitEffect(aos_air_types::PlanStepEmitEffect {
                kind: AirEffectKind::introspect_manifest(),
                params: ExprOrValue::Literal(ValueLiteral::Record(ValueRecord {
                    record: IndexMap::from([(
                        "consistency".into(),
                        ValueLiteral::Text(ValueText {
                            text: "head".into(),
                        }),
                    )]),
                })),
                cap: "query_cap".into(),
                idempotency_key: None,
                bind: aos_air_types::PlanBindEffect {
                    effect_id_as: "req".into(),
                },
            }),
        }],
        edges: vec![],
        required_caps: vec!["query_cap".into()],
        allowed_effects: vec![AirEffectKind::introspect_manifest()],
        invariants: vec![],
    };

    let mut loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![],
        vec![],
    );
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![def_text_record_schema(
            fixtures::START_SCHEMA,
            vec![("id", text_type())],
        )],
    );

    // Policy denies introspect.* from plans.
    let policy = DefPolicy {
        name: "com.acme/policy@1".into(),
        rules: vec![PolicyRule {
            when: PolicyMatch {
                effect_kind: Some(AirEffectKind::introspect_manifest()),
                origin_kind: Some(OriginKind::Plan),
                ..Default::default()
            },
            decision: PolicyDecision::Deny,
        }],
    };
    attach_default_policy(&mut loaded, policy);

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_result(
            fixtures::START_SCHEMA,
            &serde_json::json!({ "id": "start" }),
        )
        .expect("submit start event");
    let err = world.kernel.tick().unwrap_err();
    assert!(
        matches!(err, KernelError::PolicyDenied { .. }),
        "expected policy denial, got {err:?}"
    );
}

fn allow_http_enforcer(store: &std::sync::Arc<TestStore>) -> aos_air_types::DefModule {
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

#[test]
fn plan_introspect_missing_capability_is_rejected() {
    let store = fixtures::new_mem_store();
    let plan_name = "com.acme/IntrospectPlan@1".to_string();
    let plan = aos_air_types::DefPlan {
        name: plan_name.clone(),
        input: fixtures::schema(fixtures::START_SCHEMA),
        output: None,
        locals: IndexMap::new(),
        steps: vec![aos_air_types::PlanStep {
            id: "emit".into(),
            kind: aos_air_types::PlanStepKind::EmitEffect(aos_air_types::PlanStepEmitEffect {
                kind: AirEffectKind::introspect_manifest(),
                params: ExprOrValue::Literal(ValueLiteral::Record(ValueRecord {
                    record: IndexMap::from([(
                        "consistency".into(),
                        ValueLiteral::Text(ValueText {
                            text: "head".into(),
                        }),
                    )]),
                })),
                cap: "query_cap".into(),
                idempotency_key: None,
                bind: aos_air_types::PlanBindEffect {
                    effect_id_as: "req".into(),
                },
            }),
        }],
        edges: vec![],
        required_caps: vec!["query_cap".into()],
        allowed_effects: vec![AirEffectKind::introspect_manifest()],
        invariants: vec![],
    };

    let mut loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![],
        vec![],
    );
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![def_text_record_schema(
            fixtures::START_SCHEMA,
            vec![("id", text_type())],
        )],
    );

    // Remove the query cap grant/def so the resolver fails.
    if let Some(defaults) = loaded.manifest.defaults.as_mut() {
        defaults.cap_grants.retain(|g| g.name != "query_cap");
    }
    loaded.caps.remove("sys/query@1");
    loaded
        .manifest
        .caps
        .retain(|c| c.name.as_str() != "sys/query@1");

    let err = match TestWorld::with_store(store, loaded) {
        Ok(_) => panic!("expected manifest load to fail due to missing query cap"),
        Err(e) => e,
    };
    match err {
        KernelError::PlanCapabilityMissing { ref cap, .. } if cap == "query_cap" => {}
        other => panic!("expected missing query cap at manifest load, got {other:?}"),
    }
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
