use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aos_air_types::HashRef;
use aos_effect_adapters::adapters::llm::{LlmAdapter, LlmCompactAdapter};
use aos_effect_adapters::config::{LlmAdapterConfig, LlmApiKind, ProviderConfig};
use aos_effect_adapters::traits::AsyncEffectAdapter;
use aos_effects::builtins::{
    LlmCompactParams, LlmCompactReceipt, LlmCompactStrategy, LlmCompactionArtifactKind,
    LlmGenerateParams, LlmProviderCompatibility, LlmRuntimeArgs, LlmWindowItem, LlmWindowItemKind,
};
use aos_effects::{EffectIntent, ReceiptStatus, effect_ops};
use aos_kernel::{MemStore, Store};
use serde_cbor;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

fn build_intent(kind: impl Into<String>, params_cbor: Vec<u8>) -> EffectIntent {
    EffectIntent::from_raw_params(kind, params_cbor, [0u8; 32]).unwrap()
}

async fn start_test_server(
    body: &'static [u8],
    status_line: &'static str,
    delay: Option<Duration>,
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = vec![0u8; 1024];
            let _ = stream.read(&mut buf).await;
            if let Some(d) = delay {
                tokio::time::sleep(d).await;
            }
            let response = format!(
                "HTTP/1.1 {}\r\nContent-Length: {}\r\n\r\n",
                status_line,
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.write_all(body).await;
        }
    });
    addr
}

async fn start_test_server_with_capture(
    body: &'static [u8],
    status_line: &'static str,
) -> (SocketAddr, oneshot::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut raw = Vec::new();
            let mut header_end = None;
            let mut content_length = 0usize;

            loop {
                let mut chunk = vec![0u8; 2048];
                let n = stream.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
                if header_end.is_none() {
                    if let Some(idx) = find_subslice(&raw, b"\r\n\r\n") {
                        let end = idx + 4;
                        header_end = Some(end);
                        content_length = parse_content_length(&raw[..end]);
                    }
                }
                if let Some(end) = header_end {
                    if raw.len() >= end + content_length {
                        break;
                    }
                }
            }

            let _ = tx.send(raw);

            let response = format!(
                "HTTP/1.1 {}\r\nContent-Length: {}\r\n\r\n",
                status_line,
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.write_all(body).await;
        }
    });
    (addr, rx)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn parse_content_length(header_bytes: &[u8]) -> usize {
    let Ok(text) = std::str::from_utf8(header_bytes) else {
        return 0;
    };
    text.lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                return value.trim().parse::<usize>().ok();
            }
            None
        })
        .unwrap_or(0)
}

fn extract_http_body(raw_request: &[u8]) -> &[u8] {
    let end = find_subslice(raw_request, b"\r\n\r\n").expect("request header delimiter") + 4;
    &raw_request[end..]
}

async fn loopback_available() -> bool {
    TcpListener::bind("127.0.0.1:0").await.is_ok()
}

#[tokio::test]
async fn llm_errors_missing_api_key() {
    let store = Arc::new(MemStore::new());
    let mut providers = HashMap::new();
    providers.insert(
        "openai".into(),
        ProviderConfig {
            base_url: "http://127.0.0.1:0".into(),
            timeout: Duration::from_secs(5),
            api_kind: LlmApiKind::ChatCompletions,
        },
    );
    let cfg = LlmAdapterConfig {
        providers,
        default_provider: "openai".into(),
    };
    let adapter = LlmAdapter::new(store.clone(), cfg);

    // Missing api_key
    let params = LlmGenerateParams {
        correlation_id: None,
        provider: "openai".into(),
        model: "gpt-5.2".into(),
        window_items: vec![LlmWindowItem::message_ref(
            HashRef::new(store.put_blob(b"[]").unwrap().to_hex()).unwrap(),
        )],
        runtime: LlmRuntimeArgs {
            temperature: Some("0".into()),
            top_p: None,
            max_tokens: Some(1024 * 16),
            tool_refs: None,
            tool_choice: None,
            reasoning_effort: None,
            stop_sequences: None,
            metadata: None,
            provider_options_ref: None,
            response_format_ref: None,
        },
        api_key: None,
    };
    let intent = build_intent(
        effect_ops::LLM_GENERATE,
        serde_cbor::to_vec(&params).unwrap(),
    );
    let receipt = adapter.execute(&intent).await.unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Error);
}

#[tokio::test]
async fn llm_unknown_provider_errors() {
    let store = Arc::new(MemStore::new());
    let cfg = LlmAdapterConfig {
        providers: HashMap::new(),
        default_provider: "openai".into(),
    };
    let adapter = LlmAdapter::new(store.clone(), cfg);

    let params = LlmGenerateParams {
        correlation_id: None,
        provider: "missing".into(),
        model: "gpt".into(),
        window_items: vec![LlmWindowItem::message_ref(
            HashRef::new(store.put_blob(b"[]").unwrap().to_hex()).unwrap(),
        )],
        runtime: LlmRuntimeArgs {
            temperature: Some("0".into()),
            top_p: None,
            max_tokens: Some(16),
            tool_refs: None,
            tool_choice: None,
            reasoning_effort: None,
            stop_sequences: None,
            metadata: None,
            provider_options_ref: None,
            response_format_ref: None,
        },
        api_key: Some("key".into()),
    };
    let intent = build_intent(
        effect_ops::LLM_GENERATE,
        serde_cbor::to_vec(&params).unwrap(),
    );
    let receipt = adapter.execute(&intent).await.unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Error);
}

#[tokio::test]
async fn llm_message_ref_missing_errors() {
    let store = Arc::new(MemStore::new());
    let mut providers = HashMap::new();
    providers.insert(
        "openai".into(),
        ProviderConfig {
            base_url: "http://127.0.0.1:0".into(),
            timeout: Duration::from_secs(5),
            api_kind: LlmApiKind::ChatCompletions,
        },
    );
    let cfg = LlmAdapterConfig {
        providers,
        default_provider: "openai".into(),
    };
    let adapter = LlmAdapter::new(store, cfg);

    // Build params with a valid-looking hash that is not present in the store.
    let missing_ref =
        HashRef::new("sha256:0000000000000000000000000000000000000000000000000000000000000000")
            .unwrap();
    let params = LlmGenerateParams {
        correlation_id: None,
        provider: "openai".into(),
        model: "gpt".into(),
        window_items: vec![LlmWindowItem::message_ref(missing_ref)],
        runtime: LlmRuntimeArgs {
            temperature: Some("0".into()),
            top_p: None,
            max_tokens: Some(16),
            tool_refs: None,
            tool_choice: None,
            reasoning_effort: None,
            stop_sequences: None,
            metadata: None,
            provider_options_ref: None,
            response_format_ref: None,
        },
        api_key: Some("key".into()),
    };
    let intent = build_intent(
        effect_ops::LLM_GENERATE,
        serde_cbor::to_vec(&params).unwrap(),
    );
    let receipt = adapter.execute(&intent).await.unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Error);
}

#[tokio::test]
async fn llm_rejects_incompatible_provider_native_window_item() {
    let store = Arc::new(MemStore::new());
    let mut providers = HashMap::new();
    providers.insert(
        "openai".into(),
        ProviderConfig {
            base_url: "http://127.0.0.1:0".into(),
            timeout: Duration::from_secs(5),
            api_kind: LlmApiKind::ChatCompletions,
        },
    );
    let cfg = LlmAdapterConfig {
        providers,
        default_provider: "openai".into(),
    };
    let adapter = LlmAdapter::new(store.clone(), cfg);

    let artifact_ref = HashRef::new(
        store
            .put_blob(b"opaque-provider-artifact")
            .unwrap()
            .to_hex(),
    )
    .expect("artifact ref");
    let params = LlmGenerateParams {
        correlation_id: None,
        provider: "openai".into(),
        model: "gpt".into(),
        window_items: vec![LlmWindowItem {
            item_id: "provider-native:anthropic:1".into(),
            kind: LlmWindowItemKind::ProviderNativeArtifactRef,
            ref_: artifact_ref,
            lane: Some("Summary".into()),
            source_range: None,
            source_refs: Vec::new(),
            provider_compatibility: Some(LlmProviderCompatibility {
                provider: "anthropic".into(),
                api_kind: "messages".into(),
                model: None,
                model_family: Some("claude".into()),
                artifact_type: "context_management_block".into(),
                opaque: true,
                encrypted: false,
            }),
            estimated_tokens: Some(12),
            metadata: BTreeMap::new(),
        }],
        runtime: LlmRuntimeArgs {
            temperature: Some("0".into()),
            top_p: None,
            max_tokens: Some(16),
            tool_refs: None,
            tool_choice: None,
            reasoning_effort: None,
            stop_sequences: None,
            metadata: None,
            provider_options_ref: None,
            response_format_ref: None,
        },
        api_key: Some("key".into()),
    };
    let intent = build_intent(
        effect_ops::LLM_GENERATE,
        serde_cbor::to_vec(&params).unwrap(),
    );

    let receipt = adapter.execute(&intent).await.unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Error);
}

#[tokio::test]
async fn llm_happy_path_ok_receipt() {
    if !loopback_available().await {
        eprintln!("skipping llm_happy_path_ok_receipt: loopback bind not permitted");
        return;
    }

    let store = Arc::new(MemStore::new());
    // Prepare prompt message blob
    let message = serde_json::to_vec(&json!({"role":"user","content":"hi"})).unwrap();
    let input_hash = store.put_blob(&message).unwrap();
    let message_ref = HashRef::new(input_hash.to_hex()).unwrap();

    // Start local fake LLM server
    let body = br#"{
      "choices": [ { "message": { "content": "hello" } } ],
      "usage": { "prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3 }
    }"#;
    let addr = start_test_server(body, "200 OK", None).await;

    let mut providers = HashMap::new();
    providers.insert(
        "mock".into(),
        ProviderConfig {
            base_url: format!("http://{}", addr),
            timeout: Duration::from_secs(2),
            api_kind: LlmApiKind::ChatCompletions,
        },
    );
    let cfg = LlmAdapterConfig {
        providers,
        default_provider: "mock".into(),
    };
    let adapter = LlmAdapter::new(store.clone(), cfg);

    let params = LlmGenerateParams {
        correlation_id: None,
        provider: "mock".into(),
        model: "gpt-mock".into(),
        window_items: vec![LlmWindowItem::message_ref(message_ref)],
        runtime: LlmRuntimeArgs {
            temperature: Some("0".into()),
            top_p: None,
            max_tokens: Some(8),
            tool_refs: None,
            tool_choice: None,
            reasoning_effort: None,
            stop_sequences: None,
            metadata: None,
            provider_options_ref: None,
            response_format_ref: None,
        },
        api_key: Some("key".into()),
    };
    let intent = build_intent(
        effect_ops::LLM_GENERATE,
        serde_cbor::to_vec(&params).unwrap(),
    );

    let receipt = adapter.execute(&intent).await.unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Ok);
    // output_ref should exist in payload; parse to confirm
    let payload: aos_effects::builtins::LlmGenerateReceipt =
        serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    assert!(!payload.output_ref.as_str().is_empty());
}

#[tokio::test]
async fn llm_generate_surfaces_provider_context_items_from_responses() {
    if !loopback_available().await {
        eprintln!(
            "skipping llm_generate_surfaces_provider_context_items_from_responses: loopback bind not permitted"
        );
        return;
    }

    let store = Arc::new(MemStore::new());
    let message = serde_json::to_vec(&json!({"role":"user","content":"hi"})).unwrap();
    let message_ref = HashRef::new(store.put_blob(&message).unwrap().to_hex()).unwrap();

    let body = br#"{
      "id": "resp_generate_1",
      "model": "gpt-5.2",
      "status": "completed",
      "output": [
        {
          "id": "msg_001",
          "type": "message",
          "status": "completed",
          "role": "assistant",
          "content": [{ "type": "output_text", "text": "hello" }]
        },
        {
          "id": "cmp_001",
          "type": "compaction",
          "encrypted_content": "encrypted-summary"
        }
      ],
      "usage": {
        "input_tokens": 20,
        "output_tokens": 5,
        "total_tokens": 25
      }
    }"#;
    let addr = start_test_server(body, "200 OK", None).await;

    let mut providers = HashMap::new();
    providers.insert(
        "openai-responses".into(),
        ProviderConfig {
            base_url: format!("http://{}", addr),
            timeout: Duration::from_secs(2),
            api_kind: LlmApiKind::Responses,
        },
    );
    let cfg = LlmAdapterConfig {
        providers,
        default_provider: "openai-responses".into(),
    };
    let adapter = LlmAdapter::new(store.clone(), cfg);

    let params = LlmGenerateParams {
        correlation_id: None,
        provider: "openai-responses".into(),
        model: "gpt-5.2".into(),
        window_items: vec![LlmWindowItem::message_ref(message_ref.clone())],
        runtime: LlmRuntimeArgs {
            temperature: Some("0".into()),
            top_p: None,
            max_tokens: Some(8),
            tool_refs: None,
            tool_choice: None,
            reasoning_effort: None,
            stop_sequences: None,
            metadata: None,
            provider_options_ref: None,
            response_format_ref: None,
        },
        api_key: Some("key".into()),
    };
    let intent = build_intent(
        effect_ops::LLM_GENERATE,
        serde_cbor::to_vec(&params).unwrap(),
    );

    let receipt = adapter.execute(&intent).await.unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Ok);
    let payload: aos_effects::builtins::LlmGenerateReceipt =
        serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    assert_eq!(payload.provider_context_items.len(), 1);
    let item = payload.provider_context_items.first().unwrap();
    assert!(matches!(
        item.kind,
        LlmWindowItemKind::ProviderNativeArtifactRef
    ));
    assert_eq!(item.source_refs, vec![message_ref]);
    let compat = item
        .provider_compatibility
        .as_ref()
        .expect("provider compatibility");
    assert_eq!(compat.provider, "openai-responses");
    assert_eq!(compat.api_kind, "responses");
    assert_eq!(compat.artifact_type, "compaction");
    assert!(compat.encrypted);
}

#[tokio::test]
async fn llm_compact_adapter_returns_provider_native_window_items() {
    if !loopback_available().await {
        eprintln!(
            "skipping llm_compact_adapter_returns_provider_native_window_items: loopback bind not permitted"
        );
        return;
    }

    let store = Arc::new(MemStore::new());
    let message = serde_json::to_vec(&json!({"role":"user","content":"compact me"})).unwrap();
    let message_ref = HashRef::new(store.put_blob(&message).unwrap().to_hex()).unwrap();

    let body = br#"{
      "id": "resp_compact_1",
      "object": "response.compaction",
      "created_at": 1764967971,
      "output": [
        {
          "id": "msg_000",
          "type": "message",
          "status": "completed",
          "role": "user",
          "content": [{ "type": "input_text", "text": "compact me" }]
        },
        {
          "id": "cmp_001",
          "type": "compaction",
          "encrypted_content": "encrypted-summary"
        }
      ],
      "usage": {
        "input_tokens": 10,
        "output_tokens": 4,
        "total_tokens": 14
      }
    }"#;
    let (addr, capture_rx) = start_test_server_with_capture(body, "200 OK").await;

    let mut providers = HashMap::new();
    providers.insert(
        "openai-responses".into(),
        ProviderConfig {
            base_url: format!("http://{}", addr),
            timeout: Duration::from_secs(2),
            api_kind: LlmApiKind::Responses,
        },
    );
    let cfg = LlmAdapterConfig {
        providers,
        default_provider: "openai-responses".into(),
    };
    let adapter = LlmCompactAdapter::new(store.clone(), cfg);

    let params = LlmCompactParams {
        correlation_id: None,
        operation_id: "ctx-op-1".into(),
        provider: "openai-responses".into(),
        model: "gpt-5.2".into(),
        strategy: LlmCompactStrategy::ProviderNative,
        source_window_items: vec![LlmWindowItem::message_ref(message_ref.clone())],
        preserve_window_items: Vec::new(),
        recent_tail_items: Vec::new(),
        source_range: Some(aos_effects::builtins::LlmTranscriptRange {
            start_seq: 0,
            end_seq: 1,
        }),
        target_tokens: Some(1024),
        provider_options_ref: None,
        api_key: Some("key".into()),
    };
    let intent = build_intent(
        effect_ops::LLM_COMPACT,
        serde_cbor::to_vec(&params).unwrap(),
    );

    let receipt = adapter.execute(&intent).await.unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Ok);
    let raw_request = capture_rx.await.expect("captured request");
    assert!(
        String::from_utf8_lossy(&raw_request)
            .lines()
            .next()
            .unwrap_or_default()
            .contains("/responses/compact")
    );
    let request_body: serde_json::Value =
        serde_json::from_slice(extract_http_body(&raw_request)).expect("request body JSON");
    assert_eq!(
        request_body.get("model").and_then(|v| v.as_str()),
        Some("gpt-5.2")
    );
    assert!(request_body.get("input").is_some());

    let payload: LlmCompactReceipt = serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    assert_eq!(payload.operation_id, "ctx-op-1");
    assert!(matches!(
        payload.artifact_kind,
        LlmCompactionArtifactKind::ProviderNative
    ));
    assert_eq!(payload.artifact_refs.len(), 1);
    assert_eq!(payload.active_window_items.len(), 2);
    assert!(payload.active_window_items.iter().any(|item| {
        matches!(item.kind, LlmWindowItemKind::ProviderNativeArtifactRef)
            && item
                .provider_compatibility
                .as_ref()
                .is_some_and(|compat| compat.provider == "openai-responses")
    }));
    assert_eq!(
        payload.token_usage.as_ref().map(|usage| usage.total),
        Some(Some(14))
    );
}

#[tokio::test]
async fn llm_runtime_refs_roundtrip_into_provider_request_body() {
    if !loopback_available().await {
        eprintln!(
            "skipping llm_runtime_refs_roundtrip_into_provider_request_body: loopback bind not permitted"
        );
        return;
    }

    let store = Arc::new(MemStore::new());
    let message =
        serde_json::to_vec(&json!({"role":"user","content":"Return compact JSON."})).unwrap();
    let message_ref = HashRef::new(store.put_blob(&message).unwrap().to_hex()).unwrap();

    let provider_options = serde_json::to_vec(&json!({
        "openai": {
            "parallel_tool_calls": false,
            "seed": 7
        }
    }))
    .unwrap();
    let provider_options_ref =
        HashRef::new(store.put_blob(&provider_options).unwrap().to_hex()).unwrap();

    let response_format = serde_json::to_vec(&json!({
        "type": "json_schema",
        "json_schema": {
            "type": "object",
            "properties": { "answer": { "type": "string" } },
            "required": ["answer"]
        },
        "strict": true
    }))
    .unwrap();
    let response_format_ref =
        HashRef::new(store.put_blob(&response_format).unwrap().to_hex()).unwrap();

    let body = br#"{
      "choices": [ { "message": { "content": "{\"answer\":\"ok\"}" } } ],
      "usage": { "prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5 }
    }"#;
    let (addr, capture_rx) = start_test_server_with_capture(body, "200 OK").await;

    let mut providers = HashMap::new();
    providers.insert(
        "mock".into(),
        ProviderConfig {
            base_url: format!("http://{}", addr),
            timeout: Duration::from_secs(2),
            api_kind: LlmApiKind::ChatCompletions,
        },
    );
    let cfg = LlmAdapterConfig {
        providers,
        default_provider: "mock".into(),
    };
    let adapter = LlmAdapter::new(store.clone(), cfg);

    let params = LlmGenerateParams {
        correlation_id: Some("run-1".into()),
        provider: "mock".into(),
        model: "gpt-mock".into(),
        window_items: vec![LlmWindowItem::message_ref(message_ref)],
        runtime: LlmRuntimeArgs {
            temperature: Some("0".into()),
            top_p: None,
            max_tokens: Some(24),
            tool_refs: None,
            tool_choice: None,
            reasoning_effort: None,
            stop_sequences: None,
            metadata: None,
            provider_options_ref: Some(provider_options_ref),
            response_format_ref: Some(response_format_ref),
        },
        api_key: Some("key".into()),
    };
    let intent = build_intent(
        effect_ops::LLM_GENERATE,
        serde_cbor::to_vec(&params).unwrap(),
    );

    let receipt = adapter.execute(&intent).await.unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Ok);

    let raw_request = capture_rx.await.expect("captured request bytes");
    let request_body: serde_json::Value =
        serde_json::from_slice(extract_http_body(&raw_request)).expect("json request body");

    assert_eq!(request_body["model"], json!("gpt-mock"));
    assert_eq!(request_body["max_tokens"], json!(24));
    assert_eq!(request_body["parallel_tool_calls"], json!(false));
    assert_eq!(request_body["seed"], json!(7));
    assert_eq!(
        request_body["response_format"]["type"],
        json!("json_schema")
    );
}
