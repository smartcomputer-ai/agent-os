use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use aos_llm::{
    Client, GenerateOptions, Message, OpenAIAdapter, OpenAIAdapterConfig, OpenAICompatibleAdapter,
    OpenAICompatibleAdapterConfig, Request, StreamEventType, StreamEventTypeOrString, Tool,
    generate,
};
use futures::StreamExt;
use serde_json::json;

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

struct MockResponsePlan {
    status: u16,
    content_type: &'static str,
    body: String,
    must_contain: Vec<&'static str>,
}

fn spawn_sequence_response_server(
    expected_path: &'static str,
    plans: Vec<MockResponsePlan>,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let address = listener.local_addr().expect("listener addr");

    thread::spawn(move || {
        for plan in plans {
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
            for expected in &plan.must_contain {
                assert!(
                    request.contains(expected),
                    "expected request to contain '{}', request: {}",
                    expected,
                    request
                );
            }

            let status_text = match plan.status {
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
                plan.status,
                status_text,
                plan.content_type,
                plan.body.len(),
                plan.body
            );
            socket
                .write_all(response.as_bytes())
                .expect("write response");
            socket.flush().expect("flush");
        }
    });

    format!("http://{}", address)
}

fn minimal_request(provider: &str) -> Request {
    Request {
        model: "gpt-5.2".to_string(),
        messages: vec![Message::user("hello")],
        provider: Some(provider.to_string()),
        tools: None,
        tool_choice: None,
        response_format: None,
        temperature: None,
        top_p: None,
        max_tokens: Some(64),
        stop_sequences: None,
        reasoning_effort: None,
        metadata: None,
        provider_options: None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn client_complete_openai_responses_adapter_returns_response() {
    let body = json!({
        "id": "resp_1",
        "model": "gpt-5.2",
        "status": "completed",
        "output": [{
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "Hello from mocked integration" }]
        }],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "total_tokens": 15,
            "output_tokens_details": { "reasoning_tokens": 2 }
        }
    })
    .to_string();

    let base_url = spawn_single_response_server(200, "application/json", body, "/responses");
    let mut config = OpenAIAdapterConfig::new("test-key");
    config.base_url = base_url;
    let adapter = OpenAIAdapter::new(config).expect("adapter");

    let mut client = Client::default();
    client
        .register_provider(Arc::new(adapter))
        .expect("register provider");
    let response = client
        .complete(minimal_request("openai"))
        .await
        .expect("complete");

    assert_eq!(response.provider, "openai");
    assert_eq!(response.text(), "Hello from mocked integration");
    assert_eq!(response.usage.reasoning_tokens, Some(2));
}

#[tokio::test(flavor = "current_thread")]
async fn client_stream_openai_responses_adapter_emits_finish_event() {
    let sse_body = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\"}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\"Hel\"}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\"lo\"}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-5.2\",\"status\":\"completed\",\"output\":[{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
    )
    .to_string();

    let base_url = spawn_single_response_server(200, "text/event-stream", sse_body, "/responses");
    let mut config = OpenAIAdapterConfig::new("test-key");
    config.base_url = base_url;
    let adapter = OpenAIAdapter::new(config).expect("adapter");

    let mut client = Client::default();
    client
        .register_provider(Arc::new(adapter))
        .expect("register provider");

    let mut stream = client
        .stream(minimal_request("openai"))
        .await
        .expect("stream");

    let mut saw_delta = false;
    let mut saw_finish = false;

    while let Some(event) = stream.next().await {
        let event = event.expect("event");
        if event.event_type == StreamEventTypeOrString::Known(StreamEventType::TextDelta) {
            saw_delta = true;
        }
        if event.event_type == StreamEventTypeOrString::Known(StreamEventType::Finish) {
            saw_finish = true;
            assert_eq!(
                event
                    .response
                    .as_ref()
                    .map(|response| response.text())
                    .as_deref(),
                Some("Hello")
            );
            break;
        }
    }

    assert!(saw_delta);
    assert!(saw_finish);
}

#[tokio::test(flavor = "current_thread")]
async fn client_complete_openai_compatible_adapter_returns_response() {
    let body = json!({
        "id": "chatcmpl_1",
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": "hello from compatible"
            }
        }],
        "usage": {
            "prompt_tokens": 4,
            "completion_tokens": 3,
            "total_tokens": 7
        }
    })
    .to_string();

    let base_url = spawn_single_response_server(200, "application/json", body, "/chat/completions");
    let config = OpenAICompatibleAdapterConfig::new("test-key", base_url);
    let adapter = OpenAICompatibleAdapter::new(config).expect("adapter");

    let mut client = Client::default();
    client
        .register_provider(Arc::new(adapter))
        .expect("register provider");

    let response = client
        .complete(minimal_request("openai-compatible"))
        .await
        .expect("complete");

    assert_eq!(response.provider, "openai-compatible");
    assert_eq!(response.text(), "hello from compatible");
    assert_eq!(response.usage.total_tokens, 7);
}

#[tokio::test(flavor = "current_thread")]
async fn generate_executes_tool_and_sends_function_call_output_to_openai_responses() {
    let first_body = json!({
        "id": "resp_tool_1",
        "model": "gpt-5.2",
        "status": "completed",
        "output": [{
            "id": "fc_1",
            "type": "function_call",
            "call_id": "call_1",
            "name": "calc",
            "arguments": "{\"x\":2,\"y\":2}"
        }],
        "usage": {
            "input_tokens": 8,
            "output_tokens": 5,
            "total_tokens": 13
        }
    })
    .to_string();
    let second_body = json!({
        "id": "resp_tool_2",
        "model": "gpt-5.2",
        "status": "completed",
        "output": [{
            "id": "msg_2",
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "The sum is 4." }]
        }],
        "usage": {
            "input_tokens": 12,
            "output_tokens": 6,
            "total_tokens": 18
        }
    })
    .to_string();

    let base_url = spawn_sequence_response_server(
        "/responses",
        vec![
            MockResponsePlan {
                status: 200,
                content_type: "application/json",
                body: first_body,
                must_contain: vec!["\"type\":\"function\"", "\"name\":\"calc\""],
            },
            MockResponsePlan {
                status: 200,
                content_type: "application/json",
                body: second_body,
                must_contain: vec![
                    "\"type\":\"function_call_output\"",
                    "\"call_id\":\"call_1\"",
                ],
            },
        ],
    );
    let mut config = OpenAIAdapterConfig::new("test-key");
    config.base_url = base_url;
    let adapter = OpenAIAdapter::new(config).expect("adapter");

    let mut client = Client::default();
    client
        .register_provider(Arc::new(adapter))
        .expect("register provider");

    let mut options = GenerateOptions::new("gpt-5.2");
    options.provider = Some("openai".to_string());
    options.prompt = Some("What is 2 + 2? Use the tool.".to_string());
    options.max_tool_rounds = 2;
    options.client = Some(Arc::new(client));
    options.tools = vec![Tool::with_execute(
        "calc",
        "Add two numbers",
        json!({
            "type": "object",
            "properties": {
                "x": { "type": "number" },
                "y": { "type": "number" }
            },
            "required": ["x", "y"]
        }),
        |arguments| async move {
            let x = arguments
                .get("x")
                .and_then(|value| value.as_i64())
                .unwrap_or(0);
            let y = arguments
                .get("y")
                .and_then(|value| value.as_i64())
                .unwrap_or(0);
            Ok(json!({ "sum": x + y }))
        },
    )];

    let result = generate(options).await.expect("generate");
    assert_eq!(result.text, "The sum is 4.");
    assert_eq!(result.steps.len(), 2);
    assert_eq!(result.steps[0].tool_calls.len(), 1);
    assert_eq!(result.steps[0].tool_calls[0].name, "calc");
    assert_eq!(result.steps[0].tool_results.len(), 1);
    assert!(result.steps[1].tool_calls.is_empty());
    assert!(result.steps[1].tool_results.is_empty());
}
