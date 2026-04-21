use aos_air_types::{DefSchema, WorkflowAbi};
use aos_effects::builtins::HttpRequestParams;
use aos_wasm_abi::{WorkflowEffect, WorkflowOutput};
use helpers::fixtures::{self, TestStore, TestWorld, zero_hash};

#[path = "support/helpers.rs"]
mod helpers;
use helpers::{def_text_record_schema, text_type};

fn http_workflow_output(url: &str) -> WorkflowOutput {
    let mut headers = aos_effects::builtins::HeaderMap::new();
    headers.insert("x-test".into(), "1".into());
    let params = HttpRequestParams {
        method: "POST".into(),
        url: url.into(),
        headers,
        body_ref: Some(zero_hash()),
    };
    WorkflowOutput {
        state: None,
        domain_events: vec![],
        effects: vec![WorkflowEffect::new(
            aos_effects::EffectKind::HTTP_REQUEST,
            serde_cbor::to_vec(&params).unwrap(),
        )],
        ann: None,
    }
}

fn introspect_workflow_output() -> WorkflowOutput {
    let params = serde_json::json!({ "consistency": "head" });
    WorkflowOutput {
        state: None,
        domain_events: vec![],
        effects: vec![WorkflowEffect::new(
            aos_effects::EffectKind::INTROSPECT_MANIFEST,
            serde_cbor::to_vec(&params).unwrap(),
        )],
        ann: None,
    }
}

fn build_http_workflow_manifest(
    store: &std::sync::Arc<TestStore>,
    workflow_name: &str,
    output: WorkflowOutput,
) -> aos_kernel::manifest::LoadedManifest {
    let mut workflow = fixtures::stub_workflow_module(store, workflow_name, &output);
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/HttpState@1"),
        event: fixtures::schema(fixtures::START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::HTTP_REQUEST.into()],
        cap_slots: Default::default(),
    });

    let routing = vec![fixtures::routing_event(
        fixtures::START_SCHEMA,
        workflow_name,
    )];
    let mut loaded = fixtures::build_loaded_manifest(vec![workflow], routing);
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
    loaded
}

fn build_introspect_workflow_manifest(
    store: &std::sync::Arc<TestStore>,
) -> aos_kernel::manifest::LoadedManifest {
    let workflow_name = "com.acme/IntrospectWorkflow@1";
    let mut workflow =
        fixtures::stub_workflow_module(store, workflow_name, &introspect_workflow_output());
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/IntrospectState@1"),
        event: fixtures::schema(fixtures::START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::INTROSPECT_MANIFEST.into()],
        cap_slots: Default::default(),
    });

    let routing = vec![fixtures::routing_event(
        fixtures::START_SCHEMA,
        workflow_name,
    )];
    let mut loaded = fixtures::build_loaded_manifest(vec![workflow], routing);
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
    loaded
}

#[test]
fn workflow_http_effect_is_allowed_without_policy() {
    let store = fixtures::new_mem_store();
    let workflow_name = "com.acme/HttpWorkflow@1";
    let loaded = build_http_workflow_manifest(
        &store,
        workflow_name,
        http_workflow_output("https://example.com/allowed"),
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_result(
            fixtures::START_SCHEMA,
            &serde_json::json!({ "id": "start" }),
        )
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
fn workflow_effect_declared_in_abi_is_allowed() {
    let store = fixtures::new_mem_store();
    let workflow_name = "com.acme/HttpAllowed@1";
    let loaded = build_http_workflow_manifest(
        &store,
        workflow_name,
        http_workflow_output("https://example.com/allowed"),
    );

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
    let workflow_name = "com.acme/HttpParams@1";
    let expected_url = "https://example.com/preserved";
    let loaded =
        build_http_workflow_manifest(&store, workflow_name, http_workflow_output(expected_url));

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
fn workflow_introspect_effect_is_allowed_without_policy() {
    let store = fixtures::new_mem_store();
    let loaded = build_introspect_workflow_manifest(&store);

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_result(
            fixtures::START_SCHEMA,
            &serde_json::json!({ "id": "start" }),
        )
        .expect("submit start event");
    world.kernel.tick_until_idle().unwrap();
}
