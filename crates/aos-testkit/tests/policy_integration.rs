use aos_air_exec::Value as ExprValue;
use aos_air_types::{
    DefPolicy, EffectKind as AirEffectKind, ManifestDefaults, OriginKind, PolicyDecision,
    PolicyMatch, PolicyRule,
};
use aos_effects::builtins::HttpRequestParams;
use aos_kernel::error::KernelError;
use aos_testkit::TestWorld;
use aos_testkit::fixtures;
use aos_testkit::fixtures::zero_hash;
use aos_wasm_abi::ReducerEffect;
use indexmap::IndexMap;

fn attach_default_policy(loaded: &mut aos_kernel::manifest::LoadedManifest, policy: DefPolicy) {
    loaded.manifest.policies.push(aos_air_types::NamedRef {
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

fn http_reducer_output() -> aos_wasm_abi::ReducerOutput {
    let params = HttpRequestParams {
        method: "POST".into(),
        url: "https://example.com".into(),
        headers: Default::default(),
        body_ref: None,
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
                effect_kind: Some(AirEffectKind::HttpRequest),
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
    assert!(matches!(err, KernelError::PolicyDenied { .. }));
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
                kind: AirEffectKind::HttpRequest,
                params: fixtures::text_expr("body"),
                cap: "cap_http".into(),
                bind: aos_air_types::PlanBindEffect {
                    effect_id_as: "req".into(),
                },
            }),
        }],
        edges: vec![],
        required_caps: vec!["cap_http".into()],
        allowed_effects: vec![AirEffectKind::HttpRequest],
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
                effect_kind: Some(AirEffectKind::HttpRequest),
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
