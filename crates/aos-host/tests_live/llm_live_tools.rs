use std::collections::BTreeSet;
use std::sync::Arc;

use aos_effects::builtins::{LlmGenerateParams, LlmToolCallList, LlmToolChoice};
use aos_host::adapters::traits::AsyncEffectAdapter;
use aos_store::MemStore;
use serde_json::{Value, json};

use crate::llm_live_common::{
    ProviderRuntime, assert_ok_receipt, build_intent, decode_envelope, decode_tool_calls,
    default_runtime, load_json, make_adapter, store_json,
};

pub(crate) async fn run_required_tool_call(
    case: &ProviderRuntime,
) -> (Arc<MemStore>, String, String, Value) {
    let store = Arc::new(MemStore::new());
    let adapter = make_adapter(store.clone(), case);

    let message_ref = store_json(
        &store,
        &json!({"role":"user","content":"Call echo_payload exactly once with an object containing a non-empty string field named `value`."}),
    );
    let tool_ref = store_json(
        &store,
        &json!({
            "tools": [
                {
                    "name": "echo_payload",
                    "description": "Echo payload for adapter contract tests",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "value": { "type": "string" }
                        },
                        "required": ["value"],
                        "additionalProperties": false
                    }
                }
            ],
            "tool_choice": "required"
        }),
    );

    let mut runtime = default_runtime();
    runtime.tool_refs = Some(vec![tool_ref]);
    runtime.tool_choice = Some(LlmToolChoice::Required);
    let params = LlmGenerateParams {
        provider: case.provider_id.clone(),
        model: case.model.clone(),
        message_refs: vec![message_ref],
        runtime,
        api_key: Some(case.api_key.clone()),
    };

    let receipt = adapter
        .execute(&build_intent(&params))
        .await
        .expect("execute");
    let payload = assert_ok_receipt(&store, case, "required tool call", &receipt);
    let envelope = decode_envelope(&store, &payload);
    let calls = decode_tool_calls(&store, &envelope);
    assert!(
        !calls.is_empty(),
        "expected at least one tool call for {}",
        case.provider_id
    );

    let call = &calls[0];
    assert_eq!(call.tool_name, "echo_payload");
    let arguments = load_json(&store, &call.arguments_ref);
    let arg_value = arguments
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !arg_value.is_empty(),
        "expected non-empty `value` tool argument for {}",
        case.provider_id
    );
    (
        store,
        call.call_id.clone(),
        call.tool_name.clone(),
        arguments,
    )
}

async fn run_multi_tool_call(case: &ProviderRuntime) -> (Arc<MemStore>, LlmToolCallList) {
    let mut last_distinct_count = 0usize;
    let mut last_tool_names: Vec<String> = Vec::new();
    for attempt in 0..3 {
        let store = Arc::new(MemStore::new());
        let adapter = make_adapter(store.clone(), case);

        let message_ref = store_json(
            &store,
            &json!({
                "role":"user",
                "content": format!(
                    "Call BOTH tools before any final answer. Tool plan: \
                     (1) echo_payload with {{\"value\":\"alpha-{attempt}\"}} \
                     (2) sum_pair with {{\"a\":7,\"b\":8}}."
                )
            }),
        );
        let tool_ref = store_json(
            &store,
            &json!({
                "tools": [
                    {
                        "name": "echo_payload",
                        "description": "Echo payload for multi-tool test",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "value": { "type": "string" }
                            },
                            "required": ["value"],
                            "additionalProperties": false
                        }
                    },
                    {
                        "name": "sum_pair",
                        "description": "Return the sum of two integers",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "a": { "type": "integer" },
                                "b": { "type": "integer" }
                            },
                            "required": ["a", "b"],
                            "additionalProperties": false
                        }
                    }
                ],
                "tool_choice": "required"
            }),
        );

        let mut runtime = default_runtime();
        runtime.tool_refs = Some(vec![tool_ref]);
        runtime.tool_choice = Some(LlmToolChoice::Required);
        if case.provider_id == "openai-responses" {
            runtime.provider_options_ref = Some(store_json(
                &store,
                &json!({
                    "openai": {
                        "parallel_tool_calls": true
                    }
                }),
            ));
        }

        let params = LlmGenerateParams {
            provider: case.provider_id.clone(),
            model: case.model.clone(),
            message_refs: vec![message_ref],
            runtime,
            api_key: Some(case.api_key.clone()),
        };
        let receipt = adapter
            .execute(&build_intent(&params))
            .await
            .expect("execute");
        let payload = assert_ok_receipt(&store, case, "multi-tool call", &receipt);
        let envelope = decode_envelope(&store, &payload);
        let calls = decode_tool_calls(&store, &envelope);
        assert!(
            !calls.is_empty(),
            "expected at least one tool call for {}",
            case.provider_id
        );

        let allowed = ["echo_payload", "sum_pair"];
        let distinct_names: BTreeSet<String> =
            calls.iter().map(|call| call.tool_name.clone()).collect();
        for name in &distinct_names {
            assert!(
                allowed.contains(&name.as_str()),
                "unexpected tool name `{name}` for {}",
                case.provider_id
            );
        }

        last_distinct_count = distinct_names.len();
        last_tool_names = distinct_names.into_iter().collect();
        if !case.strict_multi_tool || last_distinct_count >= 2 {
            return (store, calls);
        }
    }

    panic!(
        "strict multi-tool expectation not met for provider={} model={}; \
         distinct_tools={} names={:?}",
        case.provider_id, case.model, last_distinct_count, last_tool_names
    );
}

pub(crate) async fn run_tool_result_roundtrip(case: &ProviderRuntime) {
    let (store, call_id, tool_name, arguments) = run_required_tool_call(case).await;
    let adapter = make_adapter(store.clone(), case);

    let roundtrip_messages_ref = store_json(
        &store,
        &json!([
            {
                "role": "user",
                "content": "You received the tool result. Reply with a brief plain-text answer."
            },
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_call",
                        "id": call_id,
                        "name": tool_name,
                        "arguments": arguments
                    }
                ]
            },
            {
                "type": "function_call_output",
                "call_id": call_id,
                "output": {
                    "ok": true,
                    "note": "tool_result_from_live_adapter_test"
                }
            }
        ]),
    );

    let params = LlmGenerateParams {
        provider: case.provider_id.clone(),
        model: case.model.clone(),
        message_refs: vec![roundtrip_messages_ref.clone()],
        runtime: default_runtime(),
        api_key: Some(case.api_key.clone()),
    };
    let receipt = adapter
        .execute(&build_intent(&params))
        .await
        .expect("execute");
    let payload = assert_ok_receipt(&store, case, "tool result roundtrip", &receipt);
    let envelope = decode_envelope(&store, &payload);
    let text = envelope.assistant_text.unwrap_or_default();
    assert!(
        !text.trim().is_empty(),
        "expected assistant text after tool-result roundtrip for {}",
        case.provider_id
    );

    // Third turn: verify conversation can continue after tool roundtrip.
    let mut history = load_json(&store, &roundtrip_messages_ref)
        .as_array()
        .cloned()
        .unwrap_or_default();
    history.push(json!({"role":"assistant","content":text}));
    history.push(json!({
        "role":"user",
        "content":"In one short sentence, confirm that you processed the tool result."
    }));
    let turn3_ref = store_json(&store, &Value::Array(history.clone()));

    let turn3_params = LlmGenerateParams {
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
    let turn3_payload = assert_ok_receipt(
        &store,
        case,
        "tool result roundtrip follow-up turn",
        &turn3_receipt,
    );
    let turn3_envelope = decode_envelope(&store, &turn3_payload);
    let turn3_text = turn3_envelope.assistant_text.unwrap_or_default();
    assert!(
        !turn3_text.trim().is_empty(),
        "expected assistant text in follow-up turn for {}",
        case.provider_id
    );
}

pub(crate) async fn run_multi_tool_roundtrip(case: &ProviderRuntime) {
    let (store, calls) = run_multi_tool_call(case).await;
    let adapter = make_adapter(store.clone(), case);

    let mut history: Vec<Value> = vec![json!({
        "role":"user",
        "content":"Use the tool outputs and reply with a concise combined answer."
    })];
    history.push(json!({
        "role":"assistant",
        "content": calls.iter().map(|call| {
            json!({
                "type":"tool_call",
                "id": call.call_id,
                "name": call.tool_name,
                "arguments": load_json(&store, &call.arguments_ref)
            })
        }).collect::<Vec<_>>()
    }));
    for call in &calls {
        history.push(json!({
            "type":"function_call_output",
            "call_id": call.call_id,
            "output": {
                "ok": true,
                "tool": call.tool_name,
                "result": format!("live_result_{}", call.tool_name)
            }
        }));
    }

    let mut roundtrip_text: Option<String> = None;
    for round in 0..4 {
        let roundtrip_ref = store_json(&store, &Value::Array(history.clone()));
        let roundtrip_params = LlmGenerateParams {
            provider: case.provider_id.clone(),
            model: case.model.clone(),
            message_refs: vec![roundtrip_ref],
            runtime: default_runtime(),
            api_key: Some(case.api_key.clone()),
        };
        let roundtrip_receipt = adapter
            .execute(&build_intent(&roundtrip_params))
            .await
            .expect("execute");
        let roundtrip_payload = assert_ok_receipt(
            &store,
            case,
            &format!("multi-tool roundtrip (round {})", round + 1),
            &roundtrip_receipt,
        );
        let roundtrip_envelope = decode_envelope(&store, &roundtrip_payload);
        let text = roundtrip_envelope
            .assistant_text
            .clone()
            .unwrap_or_default();
        if !text.trim().is_empty() {
            roundtrip_text = Some(text);
            break;
        }

        if roundtrip_envelope.tool_calls_ref.is_none() {
            history.push(json!({
                "role":"user",
                "content":"Provide a plain-text final answer now. Do not call any more tools."
            }));
            continue;
        }
        let tool_calls = decode_tool_calls(&store, &roundtrip_envelope);
        history.push(json!({
            "role":"assistant",
            "content": tool_calls.iter().map(|call| {
                json!({
                    "type":"tool_call",
                    "id": call.call_id,
                    "name": call.tool_name,
                    "arguments": load_json(&store, &call.arguments_ref)
                })
            }).collect::<Vec<_>>()
        }));
        for call in &tool_calls {
            history.push(json!({
                "type":"function_call_output",
                "call_id": call.call_id,
                "output": {
                    "ok": true,
                    "tool": call.tool_name,
                    "result": format!("live_result_followup_{}", call.tool_name)
                }
            }));
        }
    }
    let roundtrip_text = roundtrip_text.unwrap_or_else(|| {
        panic!(
            "expected assistant text after iterative multi-tool roundtrip for {}",
            case.provider_id
        )
    });

    // Follow-up turn to ensure multi-turn conversation still works after multi-tool results.
    history.push(json!({"role":"assistant","content":roundtrip_text.clone()}));
    history.push(json!({
        "role":"user",
        "content":"Now answer in one sentence: which tool outputs did you use?"
    }));
    let followup_ref = store_json(&store, &Value::Array(history));
    let followup_params = LlmGenerateParams {
        provider: case.provider_id.clone(),
        model: case.model.clone(),
        message_refs: vec![followup_ref],
        runtime: default_runtime(),
        api_key: Some(case.api_key.clone()),
    };
    let followup_receipt = adapter
        .execute(&build_intent(&followup_params))
        .await
        .expect("execute");
    let followup_payload = assert_ok_receipt(
        &store,
        case,
        "multi-tool roundtrip follow-up conversation",
        &followup_receipt,
    );
    let followup_envelope = decode_envelope(&store, &followup_payload);
    let followup_text = followup_envelope.assistant_text.unwrap_or_default();
    assert!(
        !followup_text.trim().is_empty(),
        "expected assistant text in multi-tool follow-up turn for {}",
        case.provider_id
    );
}
