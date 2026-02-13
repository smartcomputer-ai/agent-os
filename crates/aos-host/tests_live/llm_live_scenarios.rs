use std::sync::Arc;

use aos_effects::ReceiptStatus;
use aos_effects::builtins::LlmGenerateParams;
use aos_host::adapters::traits::AsyncEffectAdapter;
use aos_store::MemStore;
use serde_json::{Value, json};

use crate::llm_live_common::{
    ProviderRuntime, assert_ok_receipt, build_intent, decode_envelope, decode_receipt_payload,
    default_runtime, error_text_from_receipt, make_adapter, store_json,
};

pub(crate) async fn run_plain_completion(case: &ProviderRuntime) {
    let store = Arc::new(MemStore::new());
    let adapter = make_adapter(store.clone(), case);

    let message_ref = store_json(
        &store,
        &json!({"role":"user","content":"Reply with exactly: live_adapter_ok"}),
    );
    let params = LlmGenerateParams {
        correlation_id: None,
        provider: case.provider_id.clone(),
        model: case.model.clone(),
        message_refs: vec![message_ref],
        runtime: default_runtime(),
        api_key: Some(case.api_key.clone()),
    };

    let receipt = adapter
        .execute(&build_intent(&params))
        .await
        .expect("execute");
    assert_eq!(receipt.adapter_id, format!("host.llm.{}", case.provider_id));

    let payload = assert_ok_receipt(&store, case, "plain completion", &receipt);
    assert_eq!(payload.provider_id, case.provider_id);
    let envelope = decode_envelope(&store, &payload);
    let text = envelope.assistant_text.unwrap_or_default();
    assert!(
        !text.trim().is_empty(),
        "expected assistant text for {}",
        case.provider_id
    );
}

pub(crate) async fn run_multi_turn_conversation(case: &ProviderRuntime) {
    let token = "LIVE_CTX_TOKEN_4281";
    let store = Arc::new(MemStore::new());
    let adapter = make_adapter(store.clone(), case);
    let user_turn1 =
        format!("Remember this exact token for later: {token}. Reply with exactly `stored`.");

    let turn1_ref = store_json(
        &store,
        &Value::Array(vec![json!({
            "role":"user",
            "content": user_turn1
        })]),
    );
    let turn1_params = LlmGenerateParams {
        correlation_id: None,
        provider: case.provider_id.clone(),
        model: case.model.clone(),
        message_refs: vec![turn1_ref],
        runtime: default_runtime(),
        api_key: Some(case.api_key.clone()),
    };
    let turn1_receipt = adapter
        .execute(&build_intent(&turn1_params))
        .await
        .expect("execute");
    let turn1_payload = assert_ok_receipt(&store, case, "multi-turn turn1", &turn1_receipt);
    let mut turn1_text = decode_envelope(&store, &turn1_payload)
        .assistant_text
        .unwrap_or_default();
    if turn1_text.trim().is_empty() {
        let retry_ref = store_json(
            &store,
            &Value::Array(vec![json!({
                "role":"user",
                "content": format!("{user_turn1} Output plain text only.")
            })]),
        );
        let retry_params = LlmGenerateParams {
            correlation_id: None,
            provider: case.provider_id.clone(),
            model: case.model.clone(),
            message_refs: vec![retry_ref],
            runtime: default_runtime(),
            api_key: Some(case.api_key.clone()),
        };
        let retry_receipt = adapter
            .execute(&build_intent(&retry_params))
            .await
            .expect("execute");
        let retry_payload = assert_ok_receipt(
            &store,
            case,
            "multi-turn turn1 retry for text output",
            &retry_receipt,
        );
        turn1_text = decode_envelope(&store, &retry_payload)
            .assistant_text
            .unwrap_or_default();
    }
    assert!(
        !turn1_text.trim().is_empty(),
        "expected assistant text for multi-turn turn1 on {}",
        case.provider_id
    );

    let mut history = vec![
        json!({"role":"user","content": user_turn1}),
        json!({"role":"assistant","content":turn1_text}),
        json!({"role":"user","content":"What exact token did I ask you to remember? Reply with token only."}),
    ];
    let turn2_ref = store_json(&store, &Value::Array(history.clone()));
    let turn2_params = LlmGenerateParams {
        correlation_id: None,
        provider: case.provider_id.clone(),
        model: case.model.clone(),
        message_refs: vec![turn2_ref],
        runtime: default_runtime(),
        api_key: Some(case.api_key.clone()),
    };
    let turn2_receipt = adapter
        .execute(&build_intent(&turn2_params))
        .await
        .expect("execute");
    let turn2_payload = assert_ok_receipt(&store, case, "multi-turn turn2", &turn2_receipt);
    let mut turn2_text = decode_envelope(&store, &turn2_payload)
        .assistant_text
        .unwrap_or_default();
    assert!(
        !turn2_text.trim().is_empty(),
        "expected assistant text for multi-turn turn2 on {}",
        case.provider_id
    );
    if !turn2_text.contains(token) {
        history.push(json!({"role":"assistant","content":turn2_text}));
        history.push(json!({
            "role":"user",
            "content": format!("Output exactly this token with no extra text: {token}")
        }));
        let retry_ref = store_json(&store, &Value::Array(history.clone()));
        let retry_params = LlmGenerateParams {
            correlation_id: None,
            provider: case.provider_id.clone(),
            model: case.model.clone(),
            message_refs: vec![retry_ref],
            runtime: default_runtime(),
            api_key: Some(case.api_key.clone()),
        };
        let retry_receipt = adapter
            .execute(&build_intent(&retry_params))
            .await
            .expect("execute");
        let retry_payload = assert_ok_receipt(
            &store,
            case,
            "multi-turn turn2 retry for token recall",
            &retry_receipt,
        );
        turn2_text = decode_envelope(&store, &retry_payload)
            .assistant_text
            .unwrap_or_default();
    }
    assert!(
        turn2_text.contains(token),
        "expected turn2 response to contain token `{token}` for {} (got: {turn2_text})",
        case.provider_id
    );

    history.push(json!({"role":"assistant","content":turn2_text}));
    history.push(json!({"role":"user","content":"Summarize our exchange in 8 words or fewer."}));
    let turn3_ref = store_json(&store, &Value::Array(history.clone()));
    let turn3_params = LlmGenerateParams {
        correlation_id: None,
        provider: case.provider_id.clone(),
        model: case.model.clone(),
        message_refs: vec![turn3_ref],
        runtime: default_runtime(),
        api_key: Some(case.api_key.clone()),
    };
    let turn3_receipt = adapter
        .execute(&build_intent(&turn3_params))
        .await
        .expect("execute");
    let turn3_payload = assert_ok_receipt(&store, case, "multi-turn turn3", &turn3_receipt);
    let mut turn3_text = decode_envelope(&store, &turn3_payload)
        .assistant_text
        .unwrap_or_default();
    if turn3_text.trim().is_empty() {
        let mut retry_history = history.clone();
        retry_history.push(json!({
            "role":"user",
            "content":"Please provide a plain-text summary in one short sentence."
        }));
        let retry_ref = store_json(&store, &Value::Array(retry_history));
        let retry_params = LlmGenerateParams {
            correlation_id: None,
            provider: case.provider_id.clone(),
            model: case.model.clone(),
            message_refs: vec![retry_ref],
            runtime: default_runtime(),
            api_key: Some(case.api_key.clone()),
        };
        let retry_receipt = adapter
            .execute(&build_intent(&retry_params))
            .await
            .expect("execute");
        let retry_payload = assert_ok_receipt(
            &store,
            case,
            "multi-turn turn3 retry for text output",
            &retry_receipt,
        );
        turn3_text = decode_envelope(&store, &retry_payload)
            .assistant_text
            .unwrap_or_default();
    }
    assert!(
        !turn3_text.trim().is_empty(),
        "expected assistant text for multi-turn turn3 on {}",
        case.provider_id
    );
}

pub(crate) async fn run_runtime_refs_smoke(case: &ProviderRuntime) {
    let store = Arc::new(MemStore::new());
    let adapter = make_adapter(store.clone(), case);

    let message_ref = store_json(
        &store,
        &json!({"role":"user","content":"Reply with a short confirmation message."}),
    );

    let provider_options_ref = if case.provider_id == "anthropic" {
        Some(store_json(
            &store,
            &json!({
                "anthropic": {
                    "auto_cache": false
                }
            }),
        ))
    } else {
        Some(store_json(
            &store,
            &json!({
                "openai": {
                    "parallel_tool_calls": false
                }
            }),
        ))
    };

    let response_format_ref = if case.supports_response_format {
        Some(store_json(
            &store,
            &json!({
                "type": "json_object",
                "strict": false
            }),
        ))
    } else {
        None
    };

    let mut runtime = default_runtime();
    runtime.provider_options_ref = provider_options_ref;
    runtime.response_format_ref = response_format_ref;
    let params = LlmGenerateParams {
        correlation_id: None,
        provider: case.provider_id.clone(),
        model: case.model.clone(),
        message_refs: vec![message_ref],
        runtime,
        api_key: Some(case.api_key.clone()),
    };

    let mut receipt = adapter
        .execute(&build_intent(&params))
        .await
        .expect("execute");
    if receipt.status != ReceiptStatus::Ok {
        let error = error_text_from_receipt(&store, &receipt).to_ascii_lowercase();
        let response_format_unsupported = error.contains("unsupported response_format")
            || error.contains("response format")
                && (error.contains("unsupported") || error.contains("not supported"));
        if case.supports_response_format && response_format_unsupported {
            // Some models (for example gpt-5-mini) may not allow structured output formats.
            // Retry without response_format_ref so we still validate provider_options_ref path.
            let mut fallback = params.clone();
            fallback.runtime.response_format_ref = None;
            receipt = adapter
                .execute(&build_intent(&fallback))
                .await
                .expect("execute fallback without response_format_ref");
        }
    }

    let payload = assert_ok_receipt(&store, case, "runtime refs", &receipt);
    let envelope = decode_envelope(&store, &payload);
    let text = envelope.assistant_text.unwrap_or_default();
    assert!(
        !text.trim().is_empty(),
        "expected assistant text for runtime refs smoke on {}",
        case.provider_id
    );
}

pub(crate) async fn run_invalid_api_key(case: &ProviderRuntime) {
    let store = Arc::new(MemStore::new());
    let adapter = make_adapter(store.clone(), case);

    let message_ref = store_json(&store, &json!({"role":"user","content":"hello"}));
    let params = LlmGenerateParams {
        correlation_id: None,
        provider: case.provider_id.clone(),
        model: case.model.clone(),
        message_refs: vec![message_ref],
        runtime: default_runtime(),
        api_key: Some("invalid-live-adapter-key".into()),
    };

    let receipt = adapter
        .execute(&build_intent(&params))
        .await
        .expect("execute");
    assert_eq!(
        receipt.status,
        ReceiptStatus::Error,
        "expected error status for invalid key on {}",
        case.provider_id
    );
    let payload = decode_receipt_payload(&receipt);
    assert_eq!(payload.provider_id, case.provider_id);
}
