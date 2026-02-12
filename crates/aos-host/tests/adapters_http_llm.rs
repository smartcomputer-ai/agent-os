use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aos_air_types::HashRef;
use aos_effects::builtins::{HttpRequestParams, LlmGenerateParams};
use aos_effects::{EffectIntent, EffectKind, ReceiptStatus};
use aos_host::adapters::http::HttpAdapter;
use aos_host::adapters::llm::LlmAdapter;
use aos_host::adapters::traits::AsyncEffectAdapter;
use aos_host::config::{HttpAdapterConfig, LlmAdapterConfig, LlmApiKind, ProviderConfig};
use aos_store::{MemStore, Store};
use serde_cbor;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

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

async fn loopback_available() -> bool {
    TcpListener::bind("127.0.0.1:0").await.is_ok()
}

#[tokio::test]
async fn http_invalid_header_errors() {
    let store = Arc::new(MemStore::new());
    let adapter = HttpAdapter::new(store, HttpAdapterConfig::default());

    let mut headers = aos_effects::builtins::HeaderMap::new();
    headers.insert("Bad Header".into(), "x".into()); // space is invalid

    let params = HttpRequestParams {
        method: "GET".into(),
        url: "http://127.0.0.1".into(),
        headers,
        body_ref: None,
    };
    let intent = build_intent(
        EffectKind::http_request(),
        serde_cbor::to_vec(&params).unwrap(),
    );

    let receipt = adapter.execute(&intent).await.unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Error);
    assert_eq!(receipt.adapter_id, "host.http");
}

#[tokio::test]
async fn http_body_too_large_errors() {
    if !loopback_available().await {
        eprintln!("skipping http_body_too_large_errors: loopback bind not permitted");
        return;
    }

    let store = Arc::new(MemStore::new());
    let mut cfg = HttpAdapterConfig::default();
    cfg.max_body_size = 1;
    let adapter = HttpAdapter::new(store, cfg);

    let addr = start_test_server(b"hi", "200 OK", None).await;
    let params = HttpRequestParams {
        method: "GET".into(),
        url: format!("http://{}", addr),
        headers: aos_effects::builtins::HeaderMap::new(),
        body_ref: None,
    };
    let intent = build_intent(
        EffectKind::http_request(),
        serde_cbor::to_vec(&params).unwrap(),
    );

    let receipt = adapter.execute(&intent).await.unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Error);
}

#[tokio::test]
async fn http_timeout_returns_timeout_status() {
    if !loopback_available().await {
        eprintln!("skipping http_timeout_returns_timeout_status: loopback bind not permitted");
        return;
    }

    let store = Arc::new(MemStore::new());
    let mut cfg = HttpAdapterConfig::default();
    cfg.timeout = Duration::from_millis(10);
    let adapter = HttpAdapter::new(store, cfg);

    let addr = start_test_server(b"ok", "200 OK", Some(Duration::from_millis(50))).await;
    let params = HttpRequestParams {
        method: "GET".into(),
        url: format!("http://{}", addr),
        headers: aos_effects::builtins::HeaderMap::new(),
        body_ref: None,
    };
    let intent = build_intent(
        EffectKind::http_request(),
        serde_cbor::to_vec(&params).unwrap(),
    );

    let receipt = adapter.execute(&intent).await.unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Timeout);
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
        provider: "openai".into(),
        model: "gpt-5.2".into(),
        temperature: "0".into(),
        max_tokens: Some(1024 * 16),
        message_refs: vec![HashRef::new(store.put_blob(b"[]").unwrap().to_hex()).unwrap()],
        tool_refs: None,
        tool_choice: None,
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
        provider: "missing".into(),
        model: "gpt".into(),
        temperature: "0".into(),
        max_tokens: Some(16),
        message_refs: vec![HashRef::new(store.put_blob(b"[]").unwrap().to_hex()).unwrap()],
        tool_refs: None,
        tool_choice: None,
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
        provider: "openai".into(),
        model: "gpt".into(),
        temperature: "0".into(),
        max_tokens: Some(16),
        message_refs: vec![missing_ref],
        tool_refs: None,
        tool_choice: None,
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
        provider: "mock".into(),
        model: "gpt-mock".into(),
        temperature: "0".into(),
        max_tokens: Some(8),
        message_refs: vec![message_ref],
        tool_refs: None,
        tool_choice: None,
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
