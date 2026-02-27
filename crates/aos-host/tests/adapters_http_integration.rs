use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aos_effects::builtins::HttpRequestParams;
use aos_effects::{EffectIntent, EffectKind, ReceiptStatus};
use aos_host::adapters::http::HttpAdapter;
use aos_host::adapters::traits::AsyncEffectAdapter;
use aos_host::config::HttpAdapterConfig;
use aos_store::MemStore;
use serde_cbor;
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
