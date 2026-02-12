use forge_llm::{
    AnthropicAdapter, AnthropicAdapterConfig, Client, Message, ProviderErrorKind, Request,
    Response, SDKError, StreamEventType, StreamEventTypeOrString, ToolChoice, ToolDefinition,
};
use futures::StreamExt;
use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::time::{Duration, sleep, timeout};

const LIVE_RETRIES: usize = 3;
const LIVE_MAX_TOKENS: u64 = 10_000;

fn live_tests_enabled() -> bool {
    match env::var("RUN_LIVE_ANTHROPIC_TESTS") {
        Ok(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes"
        }
        Err(_) => false,
    }
}

fn live_model() -> String {
    env_or_dotenv_var("ANTHROPIC_LIVE_MODEL").unwrap_or_else(|| "claude-sonnet-4-5".to_string())
}

fn dotenv_candidates() -> Vec<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    vec![
        manifest_dir.join("../../.env"),
        manifest_dir.join(".env"),
        PathBuf::from(".env"),
    ]
}

fn parse_dotenv_value(contents: &str, key: &str) -> Option<String> {
    for raw_line in contents.lines() {
        let mut line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(stripped) = line.strip_prefix("export ") {
            line = stripped.trim();
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != key {
            continue;
        }
        let value = value.trim();
        let unquoted = if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
            || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        if !unquoted.is_empty() {
            return Some(unquoted.to_string());
        }
    }
    None
}

fn env_or_dotenv_var(key: &str) -> Option<String> {
    if let Ok(value) = env::var(key) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    for path in dotenv_candidates() {
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(value) = parse_dotenv_value(&contents, key) {
            return Some(value);
        }
    }

    None
}

fn build_live_client() -> Option<Client> {
    let api_key = env_or_dotenv_var("ANTHROPIC_API_KEY")?;
    let mut config = AnthropicAdapterConfig::new(api_key);
    if let Some(base_url) = env_or_dotenv_var("ANTHROPIC_BASE_URL") {
        config.base_url = base_url;
    }
    let adapter = AnthropicAdapter::new(config).ok()?;

    let mut client = Client::default();
    client
        .register_provider(Arc::new(adapter))
        .expect("register provider");
    Some(client)
}

fn greeting_request_with_max_tokens(max_tokens: u64) -> Request {
    Request {
        model: live_model(),
        messages: vec![Message::user("Reply with a short greeting.")],
        provider: Some("anthropic".to_string()),
        tools: None,
        tool_choice: None,
        response_format: None,
        temperature: None,
        top_p: None,
        max_tokens: Some(max_tokens),
        stop_sequences: None,
        reasoning_effort: None,
        metadata: None,
        provider_options: None,
    }
}

fn low_token_long_output_request(max_tokens: u64) -> Request {
    Request {
        messages: vec![Message::user(
            "Write a detailed 200-word summary of Rust ownership rules.",
        )],
        max_tokens: Some(max_tokens),
        ..greeting_request_with_max_tokens(max_tokens)
    }
}

fn required_tool_call_request(max_tokens: u64) -> Request {
    Request {
        messages: vec![Message::user(
            "Call the `echo_payload` tool exactly once with {\"value\":\"live\"}.",
        )],
        tools: Some(vec![ToolDefinition {
            name: "echo_payload".to_string(),
            description: "Echo payload for adapter tool-call testing".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                },
                "required": ["value"],
                "additionalProperties": false
            }),
        }]),
        tool_choice: Some(ToolChoice {
            mode: "required".to_string(),
            tool_name: None,
        }),
        max_tokens: Some(max_tokens),
        ..greeting_request_with_max_tokens(max_tokens)
    }
}

fn thinking_request_with_max_tokens(max_tokens: u64) -> Request {
    Request {
        messages: vec![Message::user("Compute 37 * 23 and explain briefly.")],
        max_tokens: Some(max_tokens),
        provider_options: Some(json!({
            "anthropic": {
                "thinking": { "type": "enabled", "budget_tokens": 1024 },
                "beta_headers": ["interleaved-thinking-2025-05-14"]
            }
        })),
        ..greeting_request_with_max_tokens(max_tokens)
    }
}

async fn complete_with_retries(client: &Client, request: Request) -> Result<Response, SDKError> {
    let mut last_error: Option<SDKError> = None;

    for attempt in 0..LIVE_RETRIES {
        match client.complete(request.clone()).await {
            Ok(response) => return Ok(response),
            Err(error) if error.retryable() && attempt + 1 < LIVE_RETRIES => {
                last_error = Some(error);
                sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
            }
            Err(error) => return Err(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        SDKError::Stream(forge_llm::StreamError::new("complete exhausted retries"))
    }))
}

async fn stream_finish_with_retries(
    client: &Client,
    request: Request,
) -> Result<(bool, Response), SDKError> {
    let mut last_error: Option<SDKError> = None;

    for attempt in 0..LIVE_RETRIES {
        let mut stream = match client.stream(request.clone()).await {
            Ok(stream) => stream,
            Err(error) if error.retryable() && attempt + 1 < LIVE_RETRIES => {
                last_error = Some(error);
                sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
                continue;
            }
            Err(error) => return Err(error),
        };

        let mut saw_text_delta = false;
        let mut stream_error: Option<SDKError> = None;

        loop {
            let next = timeout(Duration::from_secs(90), stream.next())
                .await
                .map_err(|_| {
                    SDKError::Stream(forge_llm::StreamError::new(
                        "timed out waiting for live stream event",
                    ))
                })?;

            let Some(event_result) = next else {
                break;
            };

            match event_result {
                Ok(event) => {
                    if event.event_type
                        == StreamEventTypeOrString::Known(StreamEventType::TextDelta)
                    {
                        saw_text_delta = true;
                    }
                    if event.event_type == StreamEventTypeOrString::Known(StreamEventType::Finish) {
                        if let Some(response) = event.response {
                            return Ok((saw_text_delta, response));
                        }
                        return Err(SDKError::Stream(forge_llm::StreamError::new(
                            "finish event was missing response payload",
                        )));
                    }
                }
                Err(error) => {
                    stream_error = Some(error);
                    break;
                }
            }
        }

        if let Some(error) = stream_error {
            if error.retryable() && attempt + 1 < LIVE_RETRIES {
                last_error = Some(error);
                sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
                continue;
            }
            return Err(error);
        }

        if attempt + 1 < LIVE_RETRIES {
            sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
            continue;
        }
    }

    Err(last_error.unwrap_or_else(|| {
        SDKError::Stream(forge_llm::StreamError::new(
            "stream ended without terminal finish event",
        ))
    }))
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_ANTHROPIC_TESTS=1 and ANTHROPIC_API_KEY (env or .env)"]
async fn anthropic_live_complete_returns_non_empty_text() {
    if !live_tests_enabled() {
        return;
    }

    let Some(client) = build_live_client() else {
        return;
    };

    let response =
        complete_with_retries(&client, greeting_request_with_max_tokens(LIVE_MAX_TOKENS))
            .await
            .expect("anthropic live complete");

    assert_eq!(response.provider, "anthropic");
    assert!(!response.text().trim().is_empty());
    assert!(response.usage.total_tokens > 0);
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_ANTHROPIC_TESTS=1 and ANTHROPIC_API_KEY (env or .env)"]
async fn anthropic_live_complete_with_thinking_populates_reasoning() {
    if !live_tests_enabled() {
        return;
    }

    let Some(client) = build_live_client() else {
        return;
    };

    let response =
        complete_with_retries(&client, thinking_request_with_max_tokens(LIVE_MAX_TOKENS))
            .await
            .expect("anthropic live complete with thinking");

    assert_eq!(response.provider, "anthropic");
    assert!(
        response.usage.reasoning_tokens.unwrap_or_default() > 0,
        "expected reasoning_tokens > 0 when thinking is enabled"
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_ANTHROPIC_TESTS=1 and ANTHROPIC_API_KEY (env or .env)"]
async fn anthropic_live_complete_low_max_tokens_maps_length_finish_reason() {
    if !live_tests_enabled() {
        return;
    }

    let Some(client) = build_live_client() else {
        return;
    };

    let response = complete_with_retries(&client, low_token_long_output_request(16))
        .await
        .expect("anthropic live complete with low max tokens");

    assert_eq!(response.provider, "anthropic");
    assert_eq!(
        response.finish_reason.reason, "length",
        "expected max-tokens truncation to map to finish reason 'length'"
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_ANTHROPIC_TESTS=1 and ANTHROPIC_API_KEY (env or .env)"]
async fn anthropic_live_stream_emits_text_delta_and_finish_response() {
    if !live_tests_enabled() {
        return;
    }

    let Some(client) = build_live_client() else {
        return;
    };

    let (saw_text_delta, response) =
        stream_finish_with_retries(&client, greeting_request_with_max_tokens(LIVE_MAX_TOKENS))
            .await
            .expect("anthropic live stream terminal finish");

    assert!(saw_text_delta, "expected at least one text delta event");
    assert_eq!(response.provider, "anthropic");
    assert!(!response.text().trim().is_empty());
    assert!(response.usage.total_tokens > 0);
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_ANTHROPIC_TESTS=1 and ANTHROPIC_API_KEY (env or .env)"]
async fn anthropic_live_stream_low_max_tokens_maps_length_finish_reason() {
    if !live_tests_enabled() {
        return;
    }

    let Some(client) = build_live_client() else {
        return;
    };

    let (_saw_text_delta, response) =
        stream_finish_with_retries(&client, low_token_long_output_request(16))
            .await
            .expect("anthropic live stream with low max tokens");

    assert_eq!(response.provider, "anthropic");
    assert_eq!(response.finish_reason.reason, "length");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_ANTHROPIC_TESTS=1 and ANTHROPIC_API_KEY (env or .env)"]
async fn anthropic_live_complete_required_tool_choice_returns_tool_call() {
    if !live_tests_enabled() {
        return;
    }

    let Some(client) = build_live_client() else {
        return;
    };

    let response = complete_with_retries(&client, required_tool_call_request(LIVE_MAX_TOKENS))
        .await
        .expect("anthropic live required tool choice complete");

    let tool_calls = response.tool_calls();
    assert!(
        !tool_calls.is_empty(),
        "expected at least one tool call for required tool choice"
    );
    assert_eq!(tool_calls[0].name, "echo_payload");
    let value = tool_calls[0]
        .arguments
        .get("value")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    assert!(
        !value.is_empty(),
        "expected tool call arguments to include non-empty `value`"
    );
    assert_eq!(response.finish_reason.reason, "tool_calls");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_ANTHROPIC_TESTS=1 and ANTHROPIC_API_KEY (env or .env)"]
async fn anthropic_live_stream_required_tool_choice_emits_tool_call_events() {
    if !live_tests_enabled() {
        return;
    }

    let Some(client) = build_live_client() else {
        return;
    };

    let request = required_tool_call_request(LIVE_MAX_TOKENS);
    let mut last_error: Option<SDKError> = None;

    for attempt in 0..LIVE_RETRIES {
        let mut stream = match client.stream(request.clone()).await {
            Ok(stream) => stream,
            Err(error) if error.retryable() && attempt + 1 < LIVE_RETRIES => {
                last_error = Some(error);
                sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
                continue;
            }
            Err(error) => panic!("anthropic live stream setup failed: {error}"),
        };

        let mut saw_tool_start = false;
        let mut saw_tool_end = false;
        let mut saw_finish = false;
        let mut start_name: Option<String> = None;
        let mut start_id: Option<String> = None;
        let mut end_id: Option<String> = None;
        let mut stream_error: Option<SDKError> = None;

        loop {
            let next = timeout(Duration::from_secs(90), stream.next()).await;
            let next = match next {
                Ok(next) => next,
                Err(_) => {
                    stream_error = Some(SDKError::Stream(forge_llm::StreamError::new(
                        "timed out waiting for live stream event",
                    )));
                    break;
                }
            };
            let Some(event_result) = next else {
                break;
            };

            match event_result {
                Ok(event) => {
                    if event.event_type
                        == StreamEventTypeOrString::Known(StreamEventType::ToolCallStart)
                    {
                        saw_tool_start = true;
                        if let Some(tool_call) = event.tool_call.as_ref() {
                            start_name = Some(tool_call.name.clone());
                            start_id = Some(tool_call.id.clone());
                        }
                    }
                    if event.event_type
                        == StreamEventTypeOrString::Known(StreamEventType::ToolCallEnd)
                    {
                        saw_tool_end = true;
                        if let Some(tool_call) = event.tool_call.as_ref() {
                            end_id = Some(tool_call.id.clone());
                        }
                    }
                    if event.event_type == StreamEventTypeOrString::Known(StreamEventType::Finish) {
                        saw_finish = true;
                        break;
                    }
                }
                Err(error) => {
                    stream_error = Some(error);
                    break;
                }
            }
        }

        if let Some(error) = stream_error {
            if error.retryable() && attempt + 1 < LIVE_RETRIES {
                last_error = Some(error);
                sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
                continue;
            }
            panic!("anthropic live stream returned error: {error}");
        }

        if saw_tool_start && saw_tool_end && saw_finish {
            assert_eq!(start_name.as_deref(), Some("echo_payload"));
            assert!(
                start_id.as_deref().unwrap_or_default().len() > 0
                    || end_id.as_deref().unwrap_or_default().len() > 0,
                "expected non-empty tool-call id at tool-call start or end"
            );
            return;
        }

        if attempt + 1 < LIVE_RETRIES {
            sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
            continue;
        }
    }

    panic!(
        "expected tool-call stream events after retries, last_error={:?}",
        last_error
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_ANTHROPIC_TESTS=1 and ANTHROPIC_API_KEY (env or .env)"]
async fn anthropic_live_invalid_model_returns_provider_error() {
    if !live_tests_enabled() {
        return;
    }

    let Some(client) = build_live_client() else {
        return;
    };

    let mut request = greeting_request_with_max_tokens(64);
    request.model = "claude-this-model-should-not-exist-live-test".to_string();

    let error = client
        .complete(request)
        .await
        .expect_err("invalid model should return provider error");

    match error {
        SDKError::Provider(provider) => {
            assert!(
                matches!(
                    provider.kind,
                    ProviderErrorKind::InvalidRequest
                        | ProviderErrorKind::NotFound
                        | ProviderErrorKind::Other
                ),
                "expected InvalidRequest/NotFound/Other for invalid Anthropic model, got {:?}",
                provider.kind
            );
            assert!(provider.status_code.is_some());
        }
        other => panic!("expected provider error, got: {other:?}"),
    }
}
