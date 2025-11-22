use aos_air_types::EffectKind;
use aos_effects::CapabilityGrant;
use aos_kernel::capability::CapabilityResolver;
use aos_kernel::effects::EffectManager;
use aos_kernel::policy::AllowAllPolicy;
use aos_wasm_abi::ReducerEffect;
use serde_cbor::Value as CborValue;
use std::collections::BTreeMap;

/// Plan-origin effects with semantically identical params but different CBOR shapes
/// must canonicalize to the same params bytes and intent hash.
#[test]
fn plan_effect_params_canonicalize_before_hashing() {
    // Capability: minimal llm.basic grant
    let grant = CapabilityGrant::builder("cap_llm", "sys/llm.basic@1", &serde_json::json!({}))
        .build()
        .expect("grant");
    let cap_gate = CapabilityResolver::from_runtime_grants(vec![(
        grant,
        aos_air_types::CapType::LlmBasic,
    )]);
    let mut mgr = EffectManager::new(cap_gate, Box::new(AllowAllPolicy), None, None);

    // Params variant A: temperature as float
    let params_a = llm_params_cbor(CborValue::Float(0.5));
    // Params variant B: temperature as string
    let params_b = llm_params_cbor(CborValue::Text("0.5".into()));

    let intent_a = mgr
        .enqueue_plan_effect(
            "com.acme/Plan@1",
            &EffectKind::LlmGenerate,
            "cap_llm",
            params_a.clone(),
        )
        .expect("enqueue A");
    let intent_b = mgr
        .enqueue_plan_effect(
            "com.acme/Plan@1",
            &EffectKind::LlmGenerate,
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
    let cap_gate = CapabilityResolver::from_runtime_grants(vec![(
        grant,
        aos_air_types::CapType::Timer,
    )]);
    let mut mgr = EffectManager::new(cap_gate, Box::new(AllowAllPolicy), None, None);

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

    // Should canonicalize field order but remain semantically identical.
    let canonical_again = aos_effects::normalize_effect_params(
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
    assert_eq!(intent.intent_hash, rehashed.intent_hash, "hash must be stable");
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
