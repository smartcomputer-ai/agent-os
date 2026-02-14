use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aos_air_types::HashRef;
use aos_effects::builtins::{LlmGenerateParams, LlmRuntimeArgs};
use aos_effects::{EffectIntent, EffectKind, ReceiptStatus};
use aos_host::adapters::llm::LlmAdapter;
use aos_host::adapters::traits::AsyncEffectAdapter;
use aos_host::config::{LlmAdapterConfig, LlmApiKind, ProviderConfig};
use aos_store::{MemStore, Store};
use serde_cbor;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

fn build_intent(kind: EffectKind, params_cbor: Vec<u8>) -> EffectIntent {
    // cap name is irrelevant for adapter execution here
    EffectIntent::from_raw_params(kind, "cap", params_cbor, [0u8; 32]).unwrap()
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
        message_refs: vec![HashRef::new(store.put_blob(b"[]").unwrap().to_hex()).unwrap()],
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
        EffectKind::llm_generate(),
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
        message_refs: vec![HashRef::new(store.put_blob(b"[]").unwrap().to_hex()).unwrap()],
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
        EffectKind::llm_generate(),
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
        message_refs: vec![missing_ref],
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
        EffectKind::llm_generate(),
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
        message_refs: vec![message_ref],
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
        EffectKind::llm_generate(),
        serde_cbor::to_vec(&params).unwrap(),
    );

    let receipt = adapter.execute(&intent).await.unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Ok);
    assert_eq!(receipt.adapter_id, "host.llm.mock");
    // output_ref should exist in payload; parse to confirm
    let payload: aos_effects::builtins::LlmGenerateReceipt =
        serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    assert!(!payload.output_ref.as_str().is_empty());
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
    let message = serde_json::to_vec(&json!({"role":"user","content":"Return compact JSON."})).unwrap();
    let message_ref = HashRef::new(store.put_blob(&message).unwrap().to_hex()).unwrap();

    let provider_options = serde_json::to_vec(&json!({
        "openai": {
            "parallel_tool_calls": false,
            "seed": 7
        }
    }))
    .unwrap();
    let provider_options_ref = HashRef::new(store.put_blob(&provider_options).unwrap().to_hex()).unwrap();

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
    let response_format_ref = HashRef::new(store.put_blob(&response_format).unwrap().to_hex()).unwrap();

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
        message_refs: vec![message_ref],
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
        EffectKind::llm_generate(),
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
    assert_eq!(request_body["response_format"]["type"], json!("json_schema"));
}
