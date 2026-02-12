use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use forge_llm::{
    AnthropicAdapter, AnthropicAdapterConfig, Client, Message, Request, StreamEventType,
    StreamEventTypeOrString,
};
use futures::StreamExt;
use serde_json::{Value, json};

fn spawn_single_response_server(
    status: u16,
    content_type: &str,
    body: String,
    expected_path: &'static str,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let address = listener.local_addr().expect("listener addr");
    let content_type = content_type.to_string();

    thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut buffer = vec![0_u8; 65536];
        let read = socket.read(&mut buffer).expect("read request");
        let request = String::from_utf8_lossy(&buffer[..read]).to_string();
        let first_line = request.lines().next().unwrap_or_default().to_string();
        assert!(
            first_line.contains(expected_path),
            "expected path '{}', first line: {}",
            expected_path,
            first_line
        );

        let status_text = match status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            404 => "Not Found",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            _ => "OK",
        };
        let response = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            status,
            status_text,
            content_type,
            body.len(),
            body
        );
        socket
            .write_all(response.as_bytes())
            .expect("write response");
        socket.flush().expect("flush");
    });

    format!("http://{}", address)
}

fn spawn_capture_server() -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let address = listener.local_addr().expect("listener addr");
    let (tx, rx) = std::sync::mpsc::channel();

    thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut buffer = vec![0_u8; 65536];
        let read = socket.read(&mut buffer).expect("read request");
        let request = String::from_utf8_lossy(&buffer[..read]).to_string();
        tx.send(request).expect("send request capture");

        let body = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-5",
            "content": [{"type":"text","text":"ok"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket
            .write_all(response.as_bytes())
            .expect("write response");
        socket.flush().expect("flush");
    });

    (format!("http://{}", address), rx)
}

fn minimal_request(provider: &str) -> Request {
    Request {
        model: "claude-sonnet-4-5".to_string(),
        messages: vec![Message::user("hello")],
        provider: Some(provider.to_string()),
        tools: None,
        tool_choice: None,
        response_format: None,
        temperature: None,
        top_p: None,
        max_tokens: Some(128),
        stop_sequences: None,
        reasoning_effort: None,
        metadata: None,
        provider_options: None,
    }
}

fn extract_json_body(raw_request: &str) -> Value {
    let (_, body) = raw_request
        .split_once("\r\n\r\n")
        .expect("http request with body");
    serde_json::from_str(body).expect("request JSON body")
}

#[tokio::test(flavor = "current_thread")]
async fn client_complete_anthropic_adapter_returns_thinking_and_tool_use() {
    let body = json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-5",
        "content": [
            {"type":"thinking","thinking":"reason step","signature":"sig_1"},
            {"type":"tool_use","id":"call_1","name":"echo_payload","input":{"value":"live"}},
            {"type":"text","text":"calling tool"}
        ],
        "stop_reason": "tool_use",
        "usage": {
            "input_tokens": 10,
            "output_tokens": 8,
            "cache_creation_input_tokens": 5,
            "cache_read_input_tokens": 7
        }
    })
    .to_string();

    let base_url = spawn_single_response_server(200, "application/json", body, "/messages");
    let mut config = AnthropicAdapterConfig::new("test-key");
    config.base_url = base_url;
    let adapter = AnthropicAdapter::new(config).expect("adapter");

    let mut client = Client::default();
    client
        .register_provider(Arc::new(adapter))
        .expect("register provider");

    let response = client
        .complete(minimal_request("anthropic"))
        .await
        .expect("complete");

    assert_eq!(response.provider, "anthropic");
    assert_eq!(response.finish_reason.reason, "tool_calls");
    assert_eq!(response.tool_calls().len(), 1);
    assert!(response.reasoning().is_some());
    assert_eq!(response.usage.cache_write_tokens, Some(5));
    assert_eq!(response.usage.cache_read_tokens, Some(7));
}

#[tokio::test(flavor = "current_thread")]
async fn client_stream_anthropic_adapter_emits_reasoning_tool_and_finish() {
    let sse_body = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-sonnet-4-5\",\"usage\":{\"input_tokens\":2}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"analyze\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\" more\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"echo_payload\",\"input\":{\"value\":\"live\"}}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":4}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    )
    .to_string();

    let base_url = spawn_single_response_server(200, "text/event-stream", sse_body, "/messages");
    let mut config = AnthropicAdapterConfig::new("test-key");
    config.base_url = base_url;
    let adapter = AnthropicAdapter::new(config).expect("adapter");

    let mut client = Client::default();
    client
        .register_provider(Arc::new(adapter))
        .expect("register provider");

    let mut saw_reasoning_delta = false;
    let mut saw_tool_end = false;
    let mut saw_finish = false;

    let mut stream = client
        .stream(minimal_request("anthropic"))
        .await
        .expect("stream");

    while let Some(event) = stream.next().await {
        let event = event.expect("event");
        if event.event_type == StreamEventTypeOrString::Known(StreamEventType::ReasoningDelta) {
            saw_reasoning_delta = true;
        }
        if event.event_type == StreamEventTypeOrString::Known(StreamEventType::ToolCallEnd) {
            saw_tool_end = true;
        }
        if event.event_type == StreamEventTypeOrString::Known(StreamEventType::Finish) {
            saw_finish = true;
            assert_eq!(
                event
                    .response
                    .as_ref()
                    .map(|response| response.finish_reason.reason.as_str()),
                Some("tool_calls")
            );
            break;
        }
    }

    assert!(saw_reasoning_delta);
    assert!(saw_tool_end);
    assert!(saw_finish);
}

#[tokio::test(flavor = "current_thread")]
async fn client_complete_anthropic_adapter_sends_tool_results_as_user_and_merges_alternation() {
    let (base_url, rx) = spawn_capture_server();
    let mut config = AnthropicAdapterConfig::new("test-key");
    config.base_url = base_url;
    let adapter = AnthropicAdapter::new(config).expect("adapter");

    let mut client = Client::default();
    client
        .register_provider(Arc::new(adapter))
        .expect("register provider");

    let mut request = minimal_request("anthropic");
    request.max_tokens = None;
    request.messages = vec![
        Message::user("first"),
        Message::user("second"),
        Message::assistant("ready"),
        Message::tool_result("call_1", json!({"ok": true}), false),
    ];
    request.provider_options = Some(json!({
        "anthropic": {
            "auto_cache": false,
            "beta_headers": ["interleaved-thinking-2025-05-14"]
        }
    }));

    let response = client.complete(request).await.expect("complete");
    assert_eq!(response.provider, "anthropic");

    let captured = rx.recv().expect("captured request");
    assert!(captured.contains("anthropic-beta: interleaved-thinking-2025-05-14"));

    let body = extract_json_body(&captured);
    assert_eq!(
        body.get("max_tokens").and_then(Value::as_u64),
        Some(4096),
        "anthropic max_tokens should default to 4096"
    );

    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages array");
    assert_eq!(messages.len(), 3, "expected merged user alternation");

    let first = &messages[0];
    assert_eq!(first.get("role").and_then(Value::as_str), Some("user"));
    let first_content = first
        .get("content")
        .and_then(Value::as_array)
        .expect("first message content");
    assert_eq!(first_content.len(), 2, "expected merged user content parts");

    let tool_result_msg = &messages[2];
    assert_eq!(
        tool_result_msg.get("role").and_then(Value::as_str),
        Some("user")
    );
    let tool_block = tool_result_msg
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .expect("tool_result content block");
    assert_eq!(
        tool_block.get("type").and_then(Value::as_str),
        Some("tool_result")
    );
}
