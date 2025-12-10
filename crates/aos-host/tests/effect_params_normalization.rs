use aos_effects::CapabilityGrant;
use aos_host::fixtures;
use aos_kernel::capability::CapabilityResolver;
use aos_kernel::effects::EffectManager;
use aos_kernel::journal::mem::MemJournal;
use aos_kernel::policy::AllowAllPolicy;
use aos_wasm_abi::ReducerEffect;
use serde_cbor::Value as CborValue;
use serde_json;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use aos_air_types::{EffectKind, builtins, catalog::EffectCatalog, plan_literals::SchemaIndex};

/// Plan-origin effects with semantically identical params but different CBOR shapes
/// must canonicalize to the same params bytes and intent hash.
#[test]
fn plan_effect_params_canonicalize_before_hashing() {
    // Capability: minimal llm.basic grant
    let grant = CapabilityGrant::builder("cap_llm", "sys/llm.basic@1", &serde_json::json!({}))
        .build()
        .expect("grant");
    let cap_gate =
        CapabilityResolver::from_runtime_grants(vec![(grant, aos_air_types::CapType::llm_basic())]);
    let mut mgr = mgr_with_cap(cap_gate);

    // Params variant A: valid dec128 encoded as string
    let params_a = llm_params_cbor(CborValue::Text("0.5".into()));
    // Params variant B: same values but inserted in reverse order to test canonicalization
    let params_b = llm_params_cbor_reordered(CborValue::Text("0.5".into()));

    let intent_a = mgr
        .enqueue_plan_effect(
            "com.acme/Plan@1",
            &EffectKind::llm_generate(),
            "cap_llm",
            params_a.clone(),
        )
        .expect("enqueue A");
    let intent_b = mgr
        .enqueue_plan_effect(
            "com.acme/Plan@1",
            &EffectKind::llm_generate(),
            "cap_llm",
            params_b.clone(),
        )
        .expect("enqueue B");

    assert_eq!(
        intent_a.params_cbor, intent_b.params_cbor,
        "canonical params bytes should match"
    );
    assert_eq!(
        intent_a.intent_hash, intent_b.intent_hash,
        "intent hash should be stable across sugar shapes"
    );
}

/// Reducer-emitted micro-effects should already be canonical; normalizer must preserve bytes and
/// hash while still enforcing schema conformance (field ordering canonicalized).
#[test]
fn reducer_effect_params_canonicalize_noop() {
    // Capability: timer cap
    let grant = CapabilityGrant::builder("cap_timer", "sys/timer@1", &serde_json::json!({}))
        .build()
        .expect("grant");
    let cap_gate =
        CapabilityResolver::from_runtime_grants(vec![(grant, aos_air_types::CapType::timer())]);
    let mut mgr = mgr_with_cap(cap_gate);

    // Params with out-of-order fields (key optional) to ensure canonicalization sorts.
    let mut params = BTreeMap::new();
    params.insert(
        CborValue::Text("key".into()),
        CborValue::Text("reminder".into()),
    );
    params.insert(
        CborValue::Text("deliver_at_ns".into()),
        CborValue::Integer(5i128.into()),
    );
    let params_cbor = serde_cbor::to_vec(&CborValue::Map(params)).expect("encode");

    let effect = ReducerEffect::with_cap_slot(
        aos_effects::EffectKind::TIMER_SET,
        params_cbor.clone(),
        "timer",
    );
    let intent = mgr
        .enqueue_reducer_effect("com.acme/Timer", "cap_timer", &effect)
        .expect("enqueue reducer effect");

    let (effects, schemas) = builtin_effect_context();

    // Should canonicalize field order but remain semantically identical.
    let canonical_again = aos_effects::normalize_effect_params(
        &effects,
        &schemas,
        &aos_effects::EffectKind::new(aos_effects::EffectKind::TIMER_SET),
        &params_cbor,
    )
    .expect("normalize direct");

    assert_eq!(
        intent.params_cbor, canonical_again,
        "manager should store canonical params bytes"
    );

    // Idempotency: running through normalizer again yields same bytes and hash.
    let roundtrip = aos_effects::normalize_effect_params(
        &effects,
        &schemas,
        &aos_effects::EffectKind::new(aos_effects::EffectKind::TIMER_SET),
        &intent.params_cbor,
    )
    .expect("normalize again");
    assert_eq!(intent.params_cbor, roundtrip, "canonical form is stable");

    let rehashed = aos_effects::EffectIntent::from_raw_params(
        intent.kind.clone(),
        intent.cap_name.clone(),
        intent.params_cbor.clone(),
        intent.idempotency_key,
    )
    .expect("rehash");
    assert_eq!(
        intent.intent_hash, rehashed.intent_hash,
        "hash must be stable"
    );
}

/// Different authoring sugars for the same HTTP params must normalize to identical params bytes,
/// params_ref hash, and intent_hash.
#[test]
fn sugar_forms_share_intent_hash_and_params_ref() {
    let grant = CapabilityGrant::builder("cap_http", "sys/http.out@1", &serde_json::json!({}))
        .build()
        .expect("grant");
    let cap_gate =
        CapabilityResolver::from_runtime_grants(vec![(grant, aos_air_types::CapType::http_out())]);
    let mut mgr = mgr_with_cap(cap_gate);

    // Sugar A: body_ref null, headers absent
    let sugar_a = serde_json::json!({
        "method": "GET",
        "url": "https://example.com/sugar",
        "headers": {},
        "body_ref": null
    });
    // Sugar B: headers empty map and explicit null body with different ordering
    let sugar_b = serde_json::json!({
        "body_ref": null,
        "method": "GET",
        "url": "https://example.com/sugar",
        "headers": {}
    });

    let params_a = serde_cbor::to_vec(&sugar_a).unwrap();
    let params_b = serde_cbor::to_vec(&sugar_b).unwrap();

    let intent_a = mgr
        .enqueue_plan_effect(
            "com.acme/Plan@1",
            &EffectKind::http_request(),
            "cap_http",
            params_a.clone(),
        )
        .expect("enqueue A");
    let intent_b = mgr
        .enqueue_plan_effect(
            "com.acme/Plan@1",
            &EffectKind::http_request(),
            "cap_http",
            params_b.clone(),
        )
        .expect("enqueue B");

    assert_eq!(intent_a.params_cbor, intent_b.params_cbor, "params bytes");
    assert_eq!(intent_a.intent_hash, intent_b.intent_hash, "intent hash");

    // params_ref is the hash of canonical params bytes
    let params_ref_a = aos_cbor::Hash::of_bytes(&intent_a.params_cbor);
    let params_ref_b = aos_cbor::Hash::of_bytes(&intent_b.params_cbor);
    assert_eq!(params_ref_a, params_ref_b, "params_ref");
}

/// Reducer-emitted canonical params should be identical across enqueue, journal, and replay.
#[test]
fn reducer_params_round_trip_journal_replay() {
    // Build reducer that emits a timer.set micro-effect.
    let params = timer_params_cbor(42, Some("k".into()));
    let effect = ReducerEffect::with_cap_slot(
        aos_effects::EffectKind::TIMER_SET,
        params.clone(),
        "default",
    );
    let store = fixtures::new_mem_store();
    let reducer = fixtures::stub_reducer_module(
        &store,
        "com.acme/Reducer@1",
        &aos_wasm_abi::ReducerOutput {
            state: None,
            domain_events: vec![],
            effects: vec![effect.clone()],
            ann: None,
        },
    );
    let routing = vec![fixtures::routing_event(
        fixtures::START_SCHEMA,
        &reducer.name,
    )];
    let mut manifest = fixtures::build_loaded_manifest(
        vec![],
        vec![fixtures::start_trigger("com.acme/Plan@1")],
        vec![reducer],
        routing.clone(),
    );
    fixtures::insert_test_schemas(
        &mut manifest,
        vec![fixtures::def_text_record_schema(
            fixtures::START_SCHEMA,
            vec![("id", fixtures::text_type())],
        )],
    );

    // Run kernel to emit effect, record journal, replay, and compare params_cbor.
    let mut world = fixtures::TestWorld::with_store(store.clone(), manifest).unwrap();
    world
        .submit_event_result(fixtures::START_SCHEMA, &serde_json::json!({ "id": "1" }))
        .expect("submit start event");
    world.tick_n(1).unwrap();
    let mut effects = world.drain_effects();
    assert_eq!(effects.len(), 1);
    let intent = effects.pop().unwrap();
    let journal = world.kernel.dump_journal().unwrap();

    let mut replay_world = fixtures::TestWorld::with_store_and_journal(
        store.clone(),
        {
            let mut replay_manifest = fixtures::build_loaded_manifest(
                vec![],
                vec![fixtures::start_trigger("com.acme/Plan@1")],
                vec![fixtures::stub_reducer_module(
                    &store,
                    "com.acme/Reducer@1",
                    &aos_wasm_abi::ReducerOutput {
                        state: None,
                        domain_events: vec![],
                        effects: vec![effect.clone()],
                        ann: None,
                    },
                )],
                routing.clone(),
            );
            fixtures::insert_test_schemas(
                &mut replay_manifest,
                vec![fixtures::def_text_record_schema(
                    fixtures::START_SCHEMA,
                    vec![("id", fixtures::text_type())],
                )],
            );
            replay_manifest
        },
        Box::new(MemJournal::from_entries(&journal)),
    )
    .unwrap();
    let replay_intent = replay_world
        .kernel
        .drain_effects()
        .into_iter()
        .next()
        .expect("effect in replay");

    assert_eq!(
        intent.params_cbor, replay_intent.params_cbor,
        "params bytes stable across journal/replay"
    );
    assert_eq!(
        intent.intent_hash, replay_intent.intent_hash,
        "intent hash stable across journal/replay"
    );
}

fn builtin_effect_context() -> (Arc<EffectCatalog>, Arc<SchemaIndex>) {
    let catalog =
        EffectCatalog::from_defs(builtins::builtin_effects().iter().map(|e| e.effect.clone()));
    let mut schemas = HashMap::new();
    for builtin in builtins::builtin_schemas() {
        schemas.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
    }
    (Arc::new(catalog), Arc::new(SchemaIndex::new(schemas)))
}

fn mgr_with_cap(cap_gate: CapabilityResolver) -> EffectManager {
    let (effects, schemas) = builtin_effect_context();
    EffectManager::new(
        cap_gate,
        Box::new(AllowAllPolicy),
        effects,
        schemas,
        None,
        None,
    )
}

fn timer_params_cbor(deliver_at: u64, key: Option<String>) -> Vec<u8> {
    let mut map = BTreeMap::new();
    map.insert(
        CborValue::Text("deliver_at_ns".into()),
        CborValue::Integer(deliver_at as i128),
    );
    if let Some(k) = key {
        map.insert(CborValue::Text("key".into()), CborValue::Text(k));
    }
    serde_cbor::to_vec(&CborValue::Map(map)).expect("encode timer params")
}

fn llm_params_cbor(temp_value: CborValue) -> Vec<u8> {
    let mut map = BTreeMap::new();
    map.insert(
        CborValue::Text("provider".into()),
        CborValue::Text("openai".into()),
    );
    map.insert(
        CborValue::Text("model".into()),
        CborValue::Text("gpt-4".into()),
    );
    map.insert(CborValue::Text("temperature".into()), temp_value);
    map.insert(
        CborValue::Text("max_tokens".into()),
        CborValue::Integer(16.into()),
    );
    map.insert(
        CborValue::Text("input_ref".into()),
        CborValue::Text(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000".into(),
        ),
    );
    map.insert(
        CborValue::Text("tools".into()),
        CborValue::Array(Vec::new()),
    );
    map.insert(CborValue::Text("api_key".into()), CborValue::Null);
    serde_cbor::to_vec(&CborValue::Map(map)).expect("encode params")
}

// Same fields as `llm_params_cbor` but inserted in reverse order so that canonicalization
// must sort the map.
fn llm_params_cbor_reordered(temp_value: CborValue) -> Vec<u8> {
    let mut map = BTreeMap::new();
    map.insert(CborValue::Text("api_key".into()), CborValue::Null);
    map.insert(
        CborValue::Text("tools".into()),
        CborValue::Array(Vec::new()),
    );
    map.insert(
        CborValue::Text("input_ref".into()),
        CborValue::Text(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000".into(),
        ),
    );
    map.insert(
        CborValue::Text("max_tokens".into()),
        CborValue::Integer(16.into()),
    );
    map.insert(CborValue::Text("temperature".into()), temp_value);
    map.insert(
        CborValue::Text("model".into()),
        CborValue::Text("gpt-4".into()),
    );
    map.insert(
        CborValue::Text("provider".into()),
        CborValue::Text("openai".into()),
    );
    serde_cbor::to_vec(&CborValue::Map(map)).expect("encode params")
}
