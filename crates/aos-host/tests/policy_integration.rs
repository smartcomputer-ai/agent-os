use aos_air_types::{
    DefPolicy, DefSchema, EffectKind as AirEffectKind, OriginKind, PolicyDecision, PolicyMatch,
    PolicyRule, ReducerAbi,
};
use aos_effects::builtins::HttpRequestParams;
use aos_kernel::cap_enforcer::CapCheckOutput;
use aos_kernel::error::KernelError;
use aos_wasm_abi::{PureOutput, ReducerEffect, ReducerOutput};
use helpers::fixtures::{self, TestStore, TestWorld, zero_hash};
use indexmap::IndexMap;

mod helpers;
use helpers::{attach_default_policy, def_text_record_schema, text_type};

fn http_reducer_output(url: &str) -> ReducerOutput {
    let mut headers = IndexMap::new();
    headers.insert("x-test".into(), "1".into());
    let params = HttpRequestParams {
        method: "POST".into(),
        url: url.into(),
        headers,
        body_ref: Some(zero_hash()),
    };
    ReducerOutput {
        state: None,
        domain_events: vec![],
        effects: vec![ReducerEffect::new(
            aos_effects::EffectKind::HTTP_REQUEST,
            serde_cbor::to_vec(&params).unwrap(),
        )],
        ann: None,
    }
}

fn introspect_reducer_output() -> ReducerOutput {
    let params = serde_json::json!({ "consistency": "head" });
    ReducerOutput {
        state: None,
        domain_events: vec![],
        effects: vec![ReducerEffect::new(
            aos_effects::EffectKind::INTROSPECT_MANIFEST,
            serde_cbor::to_vec(&params).unwrap(),
        )],
        ann: None,
    }
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

fn build_http_workflow_manifest(
    store: &std::sync::Arc<TestStore>,
    reducer_name: &str,
    output: ReducerOutput,
) -> aos_kernel::manifest::LoadedManifest {
    let mut reducer = fixtures::stub_reducer_module(store, reducer_name, &output);
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
        reducer_name,
    )];
    let mut loaded = fixtures::build_loaded_manifest(
        vec![],
        vec![],
        vec![reducer, allow_http_enforcer(store)],
        routing,
    );
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
    if let Some(binding) = loaded.manifest.module_bindings.get_mut(reducer_name) {
        binding.slots.insert("default".into(), "cap_http".into());
    }
    loaded
}

fn build_introspect_workflow_manifest(
    store: &std::sync::Arc<TestStore>,
) -> aos_kernel::manifest::LoadedManifest {
    let reducer_name = "com.acme/IntrospectWorkflow@1";
    let mut reducer =
        fixtures::stub_reducer_module(store, reducer_name, &introspect_reducer_output());
    reducer.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/IntrospectState@1"),
        event: fixtures::schema(fixtures::START_SCHEMA),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::INTROSPECT_MANIFEST.into()],
        cap_slots: Default::default(),
    });

    let routing = vec![fixtures::routing_event(
        fixtures::START_SCHEMA,
        reducer_name,
    )];
    let mut loaded = fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing);
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(fixtures::START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/IntrospectState@1".into(),
                ty: text_type(),
            },
        ],
    );
    if let Some(binding) = loaded.manifest.module_bindings.get_mut(reducer_name) {
        binding.slots.insert("default".into(), "query_cap".into());
    }
    loaded
}

#[test]
fn reducer_http_effect_is_denied() {
    let store = fixtures::new_mem_store();
    let reducer_name = "com.acme/HttpReducer@1";
    let mut loaded =
        build_http_workflow_manifest(&store, reducer_name, http_reducer_output("https://denied"));

    let policy = DefPolicy {
        name: "com.acme/policy@1".into(),
        rules: vec![PolicyRule {
            when: PolicyMatch {
                effect_kind: Some(AirEffectKind::http_request()),
                origin_kind: Some(OriginKind::Workflow),
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

#[test]
fn workflow_effect_allowed_by_policy() {
    let store = fixtures::new_mem_store();
    let reducer_name = "com.acme/HttpAllowed@1";
    let mut loaded = build_http_workflow_manifest(
        &store,
        reducer_name,
        http_reducer_output("https://example.com/allowed"),
    );

    let policy = DefPolicy {
        name: "com.acme/workflow-policy@1".into(),
        rules: vec![PolicyRule {
            when: PolicyMatch {
                effect_kind: Some(AirEffectKind::http_request()),
                origin_kind: Some(OriginKind::Workflow),
                ..Default::default()
            },
            decision: PolicyDecision::Allow,
        }],
    };
    attach_default_policy(&mut loaded, policy);

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_result(fixtures::START_SCHEMA, &serde_json::json!({ "id": "123" }))
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 1);
    assert_eq!(
        effects[0].kind.as_str(),
        aos_effects::EffectKind::HTTP_REQUEST
    );
}

#[test]
fn workflow_effect_params_are_preserved() {
    let store = fixtures::new_mem_store();
    let reducer_name = "com.acme/HttpParams@1";
    let expected_url = "https://example.com/preserved";
    let mut loaded =
        build_http_workflow_manifest(&store, reducer_name, http_reducer_output(expected_url));

    let policy = DefPolicy {
        name: "com.acme/workflow-policy@1".into(),
        rules: vec![PolicyRule {
            when: PolicyMatch {
                effect_kind: Some(AirEffectKind::http_request()),
                origin_kind: Some(OriginKind::Workflow),
                ..Default::default()
            },
            decision: PolicyDecision::Allow,
        }],
    };
    attach_default_policy(&mut loaded, policy);

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_result(fixtures::START_SCHEMA, &serde_json::json!({ "id": "expr" }))
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 1);
    let params: HttpRequestParams = serde_cbor::from_slice(&effects[0].params_cbor).unwrap();
    assert_eq!(params.url, expected_url);
}

#[test]
fn workflow_introspect_denied_by_policy() {
    let store = fixtures::new_mem_store();
    let mut loaded = build_introspect_workflow_manifest(&store);

    let policy = DefPolicy {
        name: "com.acme/policy@1".into(),
        rules: vec![PolicyRule {
            when: PolicyMatch {
                effect_kind: Some(AirEffectKind::introspect_manifest()),
                origin_kind: Some(OriginKind::Workflow),
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

#[test]
fn workflow_introspect_missing_capability_is_rejected() {
    let store = fixtures::new_mem_store();
    let mut loaded = build_introspect_workflow_manifest(&store);

    if let Some(defaults) = loaded.manifest.defaults.as_mut() {
        defaults.cap_grants.retain(|g| g.name != "query_cap");
    }

    let err = match TestWorld::with_store(store, loaded) {
        Ok(_) => panic!("expected manifest load to fail due to missing query cap"),
        Err(e) => e,
    };
    match err {
        KernelError::ModuleCapabilityMissing { ref cap, .. } if cap == "query_cap" => {}
        other => panic!("expected missing query cap at manifest load, got {other:?}"),
    }
}
