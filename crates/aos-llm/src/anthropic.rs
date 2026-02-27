//! Anthropic adapter using the native Messages API (`/v1/messages`).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Once};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::channel::mpsc;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

use crate::errors::{
    AdapterTimeout, ConfigurationError, HttpErrorClassification, NetworkError, ProviderError,
    ProviderErrorKind, SDKError, StreamError, classify_message, default_retryable_for_kind,
    map_http_status,
};
use crate::provider::{ProviderAdapter, ProviderFactory, register_provider_factory};
use crate::stream::{StreamEvent, StreamEventStream, StreamEventType, StreamEventTypeOrString};
use crate::types::{
    ContentKind, ContentPart, FinishReason, Message, RateLimitInfo, Request, Response, Role,
    ThinkingData, ToolCall, ToolCallData, Usage,
};
use crate::utils::{SseEvent, SseParser, is_local_path, load_file_data};

const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u64 = 4096;
const PROMPT_CACHING_BETA: &str = "prompt-caching-2024-07-31";

#[derive(Clone, Debug)]
pub struct AnthropicAdapterConfig {
    pub api_key: String,
    pub base_url: String,
    pub anthropic_version: String,
    pub timeout: AdapterTimeout,
}

impl AnthropicAdapterConfig {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            anthropic_version: DEFAULT_ANTHROPIC_VERSION.to_string(),
            timeout: AdapterTimeout::default(),
        }
    }

    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY").ok()?;
        let mut config = Self::new(api_key);
        if let Ok(base_url) = std::env::var("ANTHROPIC_BASE_URL") {
            config.base_url = base_url;
        }
        Some(config)
    }
}

#[derive(Clone)]
pub struct AnthropicAdapter {
    client: reqwest::Client,
    config: AnthropicAdapterConfig,
}

impl std::fmt::Debug for AnthropicAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicAdapter")
            .field("base_url", &self.config.base_url)
            .field("anthropic_version", &self.config.anthropic_version)
            .field("timeout", &self.config.timeout)
            .finish()
    }
}

impl AnthropicAdapter {
    pub fn new(config: AnthropicAdapterConfig) -> Result<Self, SDKError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&config.api_key).map_err(|error| {
                SDKError::Configuration(ConfigurationError::new(format!(
                    "invalid Anthropic API key header: {}",
                    error
                )))
            })?,
        );
        headers.insert(
            "anthropic-version",
            HeaderValue::from_str(&config.anthropic_version).map_err(|error| {
                SDKError::Configuration(ConfigurationError::new(format!(
                    "invalid anthropic-version header: {}",
                    error
                )))
            })?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs_f64(config.timeout.connect))
            .timeout(Duration::from_secs_f64(config.timeout.request))
            .default_headers(headers)
            .build()
            .map_err(|error| SDKError::Network(NetworkError::new(error.to_string())))?;

        Ok(Self { client, config })
    }

    fn endpoint(&self) -> String {
        format!("{}/messages", self.config.base_url.trim_end_matches('/'))
    }
}

#[derive(Debug)]
struct PreparedAnthropicRequest {
    body: Value,
    beta_headers: Vec<String>,
}

#[async_trait]
impl ProviderAdapter for AnthropicAdapter {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn complete(&self, request: Request) -> Result<Response, SDKError> {
        let prepared = build_messages_body(&request, false)?;

        let mut req = self.client.post(self.endpoint()).json(&prepared.body);
        if !prepared.beta_headers.is_empty() {
            req = req.header("anthropic-beta", prepared.beta_headers.join(","));
        }

        let response = req
            .send()
            .await
            .map_err(|error| SDKError::Network(NetworkError::new(error.to_string())))?;

        let headers = response.headers().clone();
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let retry_after = parse_retry_after(response.headers());
            let raw = response.text().await.unwrap_or_default();
            return Err(build_provider_error("anthropic", status, &raw, retry_after));
        }

        let raw_json = response
            .json::<Value>()
            .await
            .map_err(|error| SDKError::Network(NetworkError::new(error.to_string())))?;

        parse_anthropic_response(raw_json, "anthropic", Some(&headers))
    }

    async fn stream(&self, request: Request) -> Result<StreamEventStream, SDKError> {
        let prepared = build_messages_body(&request, true)?;

        let mut req = self.client.post(self.endpoint()).json(&prepared.body);
        if !prepared.beta_headers.is_empty() {
            req = req.header("anthropic-beta", prepared.beta_headers.join(","));
        }

        let response = req
            .send()
            .await
            .map_err(|error| SDKError::Network(NetworkError::new(error.to_string())))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let retry_after = parse_retry_after(response.headers());
            let raw = response.text().await.unwrap_or_default();
            return Err(build_provider_error("anthropic", status, &raw, retry_after));
        }

        let mut byte_stream = response.bytes_stream();
        let (tx, rx) = mpsc::unbounded::<Result<StreamEvent, SDKError>>();
        let stream_read_timeout = Duration::from_secs_f64(self.config.timeout.stream_read);

        tokio::spawn(async move {
            let mut parser = SseParser::new();
            let mut state = AnthropicStreamState::default();
            let mut tx = tx;

            loop {
                let next_item = match tokio::time::timeout(stream_read_timeout, byte_stream.next())
                    .await
                {
                    Ok(item) => item,
                    Err(_) => {
                        let _ = send_terminal_stream_error(
                            &mut tx,
                            SDKError::Stream(StreamError::new("Anthropic stream read timed out")),
                        );
                        return;
                    }
                };
                let Some(item) = next_item else {
                    break;
                };
                let bytes = match item {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        let _ = send_terminal_stream_error(
                            &mut tx,
                            SDKError::Stream(StreamError::new(error.to_string())),
                        );
                        return;
                    }
                };
                let chunk = String::from_utf8_lossy(&bytes);
                let events = parser.push(&chunk);
                if process_anthropic_sse_events(&events, &mut state, &mut tx).is_err() {
                    return;
                }
            }

            if let Some(event) = parser.finish() {
                let _ = process_anthropic_sse_events(&[event], &mut state, &mut tx);
            }

            let _ = emit_anthropic_finish_if_needed(&mut state, &mut tx);
        });

        Ok(Box::pin(rx))
    }

    fn supports_tool_choice(&self, mode: &str) -> bool {
        matches!(mode, "auto" | "none" | "required" | "named")
    }
}

struct AnthropicStreamState {
    response_id: Option<String>,
    model: Option<String>,
    finish_reason: Option<String>,
    usage: Usage,
    content: Vec<ContentPart>,
    text_buffer: HashMap<usize, String>,
    reasoning_buffer: HashMap<usize, String>,
    tool_buffers: HashMap<usize, StreamToolBuffer>,
    finish_emitted: bool,
    stream_started: bool,
}

impl Default for AnthropicStreamState {
    fn default() -> Self {
        Self {
            response_id: None,
            model: None,
            finish_reason: None,
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                total_tokens: 0,
                reasoning_tokens: None,
                cache_read_tokens: None,
                cache_write_tokens: None,
                raw: None,
            },
            content: Vec::new(),
            text_buffer: HashMap::new(),
            reasoning_buffer: HashMap::new(),
            tool_buffers: HashMap::new(),
            finish_emitted: false,
            stream_started: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct StreamToolBuffer {
    id: String,
    name: String,
    raw_input: String,
}

fn process_anthropic_sse_events(
    events: &[SseEvent],
    state: &mut AnthropicStreamState,
    tx: &mut mpsc::UnboundedSender<Result<StreamEvent, SDKError>>,
) -> Result<(), ()> {
    for event in events {
        let data = event.data.trim();
        if data.is_empty() {
            continue;
        }

        let payload: Value = match serde_json::from_str(data) {
            Ok(value) => value,
            Err(error) => {
                let _ = send_terminal_stream_error(
                    tx,
                    SDKError::Stream(StreamError::new(format!(
                        "invalid Anthropic SSE JSON: {}",
                        error
                    ))),
                );
                return Err(());
            }
        };

        let event_type = payload
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| event.event.as_deref())
            .unwrap_or_default();

        match event_type {
            "message_start" => {
                state.stream_started = true;
                if tx
                    .unbounded_send(Ok(simple_stream_event(StreamEventType::StreamStart)))
                    .is_err()
                {
                    return Err(());
                }

                if let Some(message) = payload.get("message") {
                    if let Some(id) = message.get("id").and_then(Value::as_str) {
                        state.response_id = Some(id.to_string());
                    }
                    if let Some(model) = message.get("model").and_then(Value::as_str) {
                        state.model = Some(model.to_string());
                    }
                    if let Some(usage) = message.get("usage") {
                        state.usage.input_tokens = usage
                            .get("input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or_default();
                        state.usage.cache_read_tokens =
                            usage.get("cache_read_input_tokens").and_then(Value::as_u64);
                        state.usage.cache_write_tokens = usage
                            .get("cache_creation_input_tokens")
                            .and_then(Value::as_u64);
                    }
                }
            }
            "content_block_start" => {
                let index = payload
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or_default() as usize;
                let Some(block) = payload.get("content_block") else {
                    continue;
                };
                let block_type = block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();

                match block_type {
                    "text" => {
                        let text = block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        state.text_buffer.insert(index, text);
                        if tx
                            .unbounded_send(Ok(StreamEvent {
                                event_type: StreamEventTypeOrString::Known(
                                    StreamEventType::TextStart,
                                ),
                                delta: None,
                                text_id: Some(format!("text_{}", index)),
                                reasoning_delta: None,
                                tool_call: None,
                                finish_reason: None,
                                usage: None,
                                response: None,
                                error: None,
                                raw: None,
                            }))
                            .is_err()
                        {
                            return Err(());
                        }
                    }
                    "tool_use" => {
                        let id = block
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("call_unknown")
                            .to_string();
                        let name = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string();
                        let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                        let raw_input = if input.is_null() {
                            String::new()
                        } else {
                            input.to_string()
                        };
                        state.tool_buffers.insert(
                            index,
                            StreamToolBuffer {
                                id: id.clone(),
                                name: name.clone(),
                                raw_input: raw_input.clone(),
                            },
                        );

                        let tool_call = ToolCall {
                            id,
                            name,
                            arguments: input,
                            raw_arguments: Some(raw_input),
                        };
                        if tx
                            .unbounded_send(Ok(StreamEvent {
                                event_type: StreamEventTypeOrString::Known(
                                    StreamEventType::ToolCallStart,
                                ),
                                delta: None,
                                text_id: None,
                                reasoning_delta: None,
                                tool_call: Some(tool_call),
                                finish_reason: None,
                                usage: None,
                                response: None,
                                error: None,
                                raw: None,
                            }))
                            .is_err()
                        {
                            return Err(());
                        }
                    }
                    "thinking" => {
                        let text = block
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        state.reasoning_buffer.insert(index, text);
                        if tx
                            .unbounded_send(Ok(simple_stream_event(
                                StreamEventType::ReasoningStart,
                            )))
                            .is_err()
                        {
                            return Err(());
                        }
                    }
                    "redacted_thinking" => {
                        let data = block
                            .get("data")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        state.content.push(redacted_thinking_content_part(data));
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let index = payload
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or_default() as usize;
                let Some(delta) = payload.get("delta") else {
                    continue;
                };
                let delta_type = delta
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();

                match delta_type {
                    "text_delta" => {
                        let text_delta = delta
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        state
                            .text_buffer
                            .entry(index)
                            .and_modify(|text| text.push_str(&text_delta))
                            .or_insert_with(|| text_delta.clone());
                        if tx
                            .unbounded_send(Ok(StreamEvent {
                                event_type: StreamEventTypeOrString::Known(
                                    StreamEventType::TextDelta,
                                ),
                                delta: Some(text_delta),
                                text_id: Some(format!("text_{}", index)),
                                reasoning_delta: None,
                                tool_call: None,
                                finish_reason: None,
                                usage: None,
                                response: None,
                                error: None,
                                raw: None,
                            }))
                            .is_err()
                        {
                            return Err(());
                        }
                    }
                    "thinking_delta" => {
                        let reasoning_delta = delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        state
                            .reasoning_buffer
                            .entry(index)
                            .and_modify(|text| text.push_str(&reasoning_delta))
                            .or_insert_with(|| reasoning_delta.clone());
                        if tx
                            .unbounded_send(Ok(StreamEvent {
                                event_type: StreamEventTypeOrString::Known(
                                    StreamEventType::ReasoningDelta,
                                ),
                                delta: None,
                                text_id: None,
                                reasoning_delta: Some(reasoning_delta),
                                tool_call: None,
                                finish_reason: None,
                                usage: None,
                                response: None,
                                error: None,
                                raw: None,
                            }))
                            .is_err()
                        {
                            return Err(());
                        }
                    }
                    "input_json_delta" => {
                        let partial = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let entry = state.tool_buffers.entry(index).or_default();
                        if !partial.is_empty() {
                            entry.raw_input.push_str(&partial);
                        }
                        let parsed = serde_json::from_str::<Value>(&entry.raw_input)
                            .unwrap_or_else(|_| Value::Object(Default::default()));
                        let tool_call = ToolCall {
                            id: entry.id.clone(),
                            name: entry.name.clone(),
                            arguments: parsed,
                            raw_arguments: Some(entry.raw_input.clone()),
                        };
                        if tx
                            .unbounded_send(Ok(StreamEvent {
                                event_type: StreamEventTypeOrString::Known(
                                    StreamEventType::ToolCallDelta,
                                ),
                                delta: None,
                                text_id: None,
                                reasoning_delta: None,
                                tool_call: Some(tool_call),
                                finish_reason: None,
                                usage: None,
                                response: None,
                                error: None,
                                raw: None,
                            }))
                            .is_err()
                        {
                            return Err(());
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let index = payload
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or_default() as usize;

                if let Some(text) = state.text_buffer.remove(&index) {
                    state.content.push(ContentPart::text(text));
                    if tx
                        .unbounded_send(Ok(StreamEvent {
                            event_type: StreamEventTypeOrString::Known(StreamEventType::TextEnd),
                            delta: None,
                            text_id: Some(format!("text_{}", index)),
                            reasoning_delta: None,
                            tool_call: None,
                            finish_reason: None,
                            usage: None,
                            response: None,
                            error: None,
                            raw: None,
                        }))
                        .is_err()
                    {
                        return Err(());
                    }
                }

                if let Some(reasoning) = state.reasoning_buffer.remove(&index) {
                    state.content.push(thinking_content_part(ThinkingData {
                        text: reasoning,
                        signature: None,
                        redacted: false,
                    }));
                    if tx
                        .unbounded_send(Ok(simple_stream_event(StreamEventType::ReasoningEnd)))
                        .is_err()
                    {
                        return Err(());
                    }
                }

                if let Some(tool) = state.tool_buffers.remove(&index) {
                    let arguments = serde_json::from_str::<Value>(&tool.raw_input)
                        .unwrap_or_else(|_| Value::Object(Default::default()));
                    let tool_call = ToolCall {
                        id: tool.id.clone(),
                        name: tool.name.clone(),
                        arguments: arguments.clone(),
                        raw_arguments: Some(tool.raw_input.clone()),
                    };
                    state.content.push(ContentPart::tool_call(ToolCallData {
                        id: tool.id,
                        name: tool.name,
                        arguments,
                        r#type: "function".to_string(),
                    }));

                    if tx
                        .unbounded_send(Ok(StreamEvent {
                            event_type: StreamEventTypeOrString::Known(
                                StreamEventType::ToolCallEnd,
                            ),
                            delta: None,
                            text_id: None,
                            reasoning_delta: None,
                            tool_call: Some(tool_call),
                            finish_reason: None,
                            usage: None,
                            response: None,
                            error: None,
                            raw: None,
                        }))
                        .is_err()
                    {
                        return Err(());
                    }
                }
            }
            "message_delta" => {
                if let Some(delta) = payload.get("delta") {
                    state.finish_reason = delta
                        .get("stop_reason")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                        .or_else(|| state.finish_reason.clone());
                }
                if let Some(usage) = payload.get("usage") {
                    state.usage.output_tokens = usage
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(state.usage.output_tokens);
                    if let Some(value) = usage
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                    {
                        state.usage.cache_write_tokens = Some(value);
                    }
                    if let Some(value) =
                        usage.get("cache_read_input_tokens").and_then(Value::as_u64)
                    {
                        state.usage.cache_read_tokens = Some(value);
                    }
                }
            }
            "message_stop" => {
                if emit_anthropic_finish_if_needed(state, tx).is_err() {
                    return Err(());
                }
            }
            "error" => {
                let message = payload
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Anthropic stream error")
                    .to_string();
                let _ = send_terminal_stream_error(tx, SDKError::Stream(StreamError::new(message)));
                return Err(());
            }
            _ => {
                if tx
                    .unbounded_send(Ok(StreamEvent {
                        event_type: StreamEventTypeOrString::Known(StreamEventType::ProviderEvent),
                        delta: None,
                        text_id: None,
                        reasoning_delta: None,
                        tool_call: None,
                        finish_reason: None,
                        usage: None,
                        response: None,
                        error: None,
                        raw: Some(payload),
                    }))
                    .is_err()
                {
                    return Err(());
                }
            }
        }
    }

    Ok(())
}

fn emit_anthropic_finish_if_needed(
    state: &mut AnthropicStreamState,
    tx: &mut mpsc::UnboundedSender<Result<StreamEvent, SDKError>>,
) -> Result<(), ()> {
    if state.finish_emitted {
        return Ok(());
    }
    state.finish_emitted = true;

    let reasoning_tokens = estimate_reasoning_tokens(&state.content);
    let usage = Usage {
        total_tokens: state.usage.input_tokens + state.usage.output_tokens,
        reasoning_tokens,
        raw: None,
        ..state.usage.clone()
    };

    let finish_reason = map_finish_reason(state.finish_reason.as_deref());
    let response = Response {
        id: state
            .response_id
            .clone()
            .unwrap_or_else(|| "msg_unknown".to_string()),
        model: state.model.clone().unwrap_or_else(|| "unknown".to_string()),
        provider: "anthropic".to_string(),
        message: Message {
            role: Role::Assistant,
            content: state.content.clone(),
            name: None,
            tool_call_id: None,
        },
        finish_reason: finish_reason.clone(),
        usage: usage.clone(),
        raw: None,
        warnings: vec![],
        rate_limit: None,
    };

    tx.unbounded_send(Ok(StreamEvent {
        event_type: StreamEventTypeOrString::Known(StreamEventType::Finish),
        delta: None,
        text_id: None,
        reasoning_delta: None,
        tool_call: None,
        finish_reason: Some(finish_reason),
        usage: Some(usage),
        response: Some(response),
        error: None,
        raw: None,
    }))
    .map_err(|_| ())
}

fn simple_stream_event(kind: StreamEventType) -> StreamEvent {
    StreamEvent {
        event_type: StreamEventTypeOrString::Known(kind),
        delta: None,
        text_id: None,
        reasoning_delta: None,
        tool_call: None,
        finish_reason: None,
        usage: None,
        response: None,
        error: None,
        raw: None,
    }
}

fn send_terminal_stream_error(
    tx: &mut mpsc::UnboundedSender<Result<StreamEvent, SDKError>>,
    error: SDKError,
) -> Result<(), ()> {
    tx.unbounded_send(Ok(StreamEvent::error(error.clone())))
        .map_err(|_| ())?;
    let _ = error;
    Ok(())
}

fn build_messages_body(
    request: &Request,
    stream: bool,
) -> Result<PreparedAnthropicRequest, SDKError> {
    let anthropic_options = request
        .provider_options
        .as_ref()
        .and_then(|options| options.get("anthropic"));

    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "stream": stream,
    });

    let system = extract_system_text(&request.messages);
    if !system.is_empty() {
        body["system"] = Value::Array(
            system
                .into_iter()
                .map(|text| json!({ "type": "text", "text": text }))
                .collect(),
        );
    }
    apply_response_format_hint(&mut body, request.response_format.as_ref());

    let translated_messages = translate_messages_to_anthropic(&request.messages)?;
    body["messages"] = Value::Array(translated_messages);

    let tool_choice_mode = request
        .tool_choice
        .as_ref()
        .map(|choice| choice.mode.as_str());
    let omit_tools_for_none = matches!(tool_choice_mode, Some("none"));

    if let Some(tools) = &request.tools {
        if !omit_tools_for_none {
            body["tools"] = Value::Array(
                tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "name": tool.name,
                            "description": tool.description,
                            "input_schema": tool.parameters,
                        })
                    })
                    .collect(),
            );
        }
    }

    if let Some(choice) = &request.tool_choice {
        if !omit_tools_for_none {
            if let Some(translated) = translate_tool_choice(choice) {
                body["tool_choice"] = translated;
            }
        }
    }

    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        body["top_p"] = json!(top_p);
    }
    if let Some(stop_sequences) = &request.stop_sequences {
        body["stop_sequences"] = json!(stop_sequences);
    }
    if let Some(metadata) = &request.metadata {
        body["metadata"] = json!(metadata);
    }

    let mut beta_headers = collect_beta_headers(anthropic_options);

    if let Some(options) = anthropic_options.and_then(Value::as_object) {
        for (key, value) in options {
            if key == "beta_headers"
                || key == "beta_features"
                || key == "auto_cache"
                || key == "cache"
            {
                continue;
            }
            body[key] = value.clone();
        }
    }

    let auto_cache_enabled = anthropic_options
        .and_then(|options| options.get("auto_cache"))
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let mut has_cache_control = false;
    if auto_cache_enabled {
        has_cache_control = inject_prompt_cache_control(&mut body);
    }
    if has_cache_control {
        beta_headers.push(PROMPT_CACHING_BETA.to_string());
    }

    beta_headers.sort();
    beta_headers.dedup();

    Ok(PreparedAnthropicRequest { body, beta_headers })
}

fn extract_system_text(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter(|message| matches!(message.role, Role::System | Role::Developer))
        .map(Message::text)
        .filter(|text| !text.is_empty())
        .collect()
}

fn translate_messages_to_anthropic(messages: &[Message]) -> Result<Vec<Value>, SDKError> {
    let mut translated = Vec::new();

    for message in messages {
        match message.role {
            Role::System | Role::Developer => {}
            Role::User => {
                let content = translate_parts_to_anthropic_content(&message.content, &Role::User)?;
                if !content.is_empty() {
                    translated.push(json!({ "role": "user", "content": content }));
                }
            }
            Role::Assistant => {
                let content =
                    translate_parts_to_anthropic_content(&message.content, &Role::Assistant)?;
                if !content.is_empty() {
                    translated.push(json!({ "role": "assistant", "content": content }));
                }
            }
            Role::Tool => {
                let mut content = Vec::new();
                for part in &message.content {
                    if let Some(tool_result) = &part.tool_result {
                        content.push(json!({
                            "type": "tool_result",
                            "tool_use_id": tool_result.tool_call_id,
                            "content": serialize_tool_result_content(&tool_result.content),
                            "is_error": tool_result.is_error,
                        }));
                    }
                }
                if !content.is_empty() {
                    translated.push(json!({ "role": "user", "content": content }));
                }
            }
        }
    }

    Ok(merge_consecutive_messages(translated))
}

fn translate_parts_to_anthropic_content(
    parts: &[ContentPart],
    role: &Role,
) -> Result<Vec<Value>, SDKError> {
    let mut content = Vec::new();

    for part in parts {
        if part.kind == ContentKind::Text.into() {
            if let Some(text) = &part.text {
                content.push(json!({ "type": "text", "text": text }));
            }
            continue;
        }

        if part.kind == ContentKind::Image.into() {
            if let Some(image) = &part.image {
                if let Some(url) = &image.url {
                    if is_local_path(url) {
                        let file_data =
                            load_file_data(std::path::Path::new(url)).map_err(|error| {
                                SDKError::Configuration(ConfigurationError::new(format!(
                                    "failed to read image path '{}': {}",
                                    url, error
                                )))
                            })?;
                        content.push(json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": file_data.media_type.unwrap_or_else(|| "image/png".to_string()),
                                "data": file_data.base64,
                            }
                        }));
                    } else {
                        content.push(json!({
                            "type": "image",
                            "source": {
                                "type": "url",
                                "url": url,
                            }
                        }));
                    }
                } else if let Some(data) = &image.data {
                    let encoded =
                        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data);
                    content.push(json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": image.media_type.clone().unwrap_or_else(|| "image/png".to_string()),
                            "data": encoded,
                        }
                    }));
                }
            }
            continue;
        }

        if part.kind == ContentKind::ToolCall.into() && *role == Role::Assistant {
            if let Some(tool_call) = &part.tool_call {
                content.push(json!({
                    "type": "tool_use",
                    "id": tool_call.id,
                    "name": tool_call.name,
                    "input": tool_call.arguments,
                }));
            }
            continue;
        }

        if part.kind == ContentKind::ToolResult.into() {
            if let Some(tool_result) = &part.tool_result {
                content.push(json!({
                    "type": "tool_result",
                    "tool_use_id": tool_result.tool_call_id,
                    "content": serialize_tool_result_content(&tool_result.content),
                    "is_error": tool_result.is_error,
                }));
            }
            continue;
        }

        if part.kind == ContentKind::Thinking.into() {
            if let Some(thinking) = &part.thinking {
                content.push(json!({
                    "type": "thinking",
                    "thinking": thinking.text,
                    "signature": thinking.signature,
                }));
            }
            continue;
        }

        if part.kind == ContentKind::RedactedThinking.into() {
            if let Some(thinking) = &part.thinking {
                content.push(json!({
                    "type": "redacted_thinking",
                    "data": thinking.text,
                }));
            }
            continue;
        }
    }

    Ok(content)
}

fn merge_consecutive_messages(messages: Vec<Value>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::new();

    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        if let Some(last) = merged.last_mut() {
            let last_role = last
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            if last_role == role {
                let additional = message
                    .get("content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if let Some(existing) = last.get_mut("content").and_then(Value::as_array_mut) {
                    existing.extend(additional);
                    continue;
                }
            }
        }

        merged.push(message);
    }

    merged
}

fn translate_tool_choice(choice: &crate::types::ToolChoice) -> Option<Value> {
    match choice.mode.as_str() {
        "auto" => Some(json!({ "type": "auto" })),
        "required" => Some(json!({ "type": "any" })),
        "named" => choice
            .tool_name
            .as_ref()
            .map(|name| json!({ "type": "tool", "name": name })),
        "none" => None,
        _ => None,
    }
}

fn apply_response_format_hint(
    body: &mut Value,
    response_format: Option<&crate::types::ResponseFormat>,
) {
    let Some(response_format) = response_format else {
        return;
    };

    let hint = match response_format.r#type.as_str() {
        "text" => None,
        "json" => Some(
            "Return only valid JSON. Do not wrap the JSON in markdown or add any explanatory text."
                .to_string(),
        ),
        "json_schema" => response_format.json_schema.as_ref().map(|schema| {
            format!(
                "Return only valid JSON that strictly matches this schema: {}",
                schema
            )
        }),
        _ => None,
    };

    let Some(hint) = hint else {
        return;
    };

    if body.get("system").is_none() {
        body["system"] = Value::Array(Vec::new());
    }
    if let Some(system) = body.get_mut("system").and_then(Value::as_array_mut) {
        system.push(json!({
            "type": "text",
            "text": hint,
        }));
    }
}

fn serialize_tool_result_content(value: &Value) -> Value {
    match value {
        Value::String(_) => value.clone(),
        other => Value::String(other.to_string()),
    }
}

fn collect_beta_headers(options: Option<&Value>) -> Vec<String> {
    let mut set = BTreeSet::new();

    if let Some(items) = options
        .and_then(|opts| opts.get("beta_headers"))
        .and_then(Value::as_array)
    {
        for item in items {
            if let Some(beta) = item.as_str() {
                if !beta.is_empty() {
                    set.insert(beta.to_string());
                }
            }
        }
    }

    if let Some(items) = options
        .and_then(|opts| opts.get("beta_features"))
        .and_then(Value::as_array)
    {
        for item in items {
            if let Some(beta) = item.as_str() {
                if !beta.is_empty() {
                    set.insert(beta.to_string());
                }
            }
        }
    }

    set.into_iter().collect()
}

fn inject_prompt_cache_control(body: &mut Value) -> bool {
    let mut applied = false;

    if let Some(system) = body.get_mut("system").and_then(Value::as_array_mut) {
        if let Some(last) = system.last_mut() {
            if ensure_cache_control(last) {
                applied = true;
            }
        }
    }

    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        if let Some(last) = tools.last_mut() {
            if ensure_cache_control(last) {
                applied = true;
            }
        }
    }

    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
        if let Some(first_message) = messages.first_mut() {
            if let Some(first_content) = first_message
                .get_mut("content")
                .and_then(Value::as_array_mut)
                .and_then(|content| content.first_mut())
            {
                if ensure_cache_control(first_content) {
                    applied = true;
                }
            }
        }
    }

    applied
}

fn ensure_cache_control(target: &mut Value) -> bool {
    if target.get("cache_control").is_some() {
        return true;
    }

    let Some(map) = target.as_object_mut() else {
        return false;
    };
    map.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
    true
}

fn parse_anthropic_response(
    raw_json: Value,
    provider: &str,
    headers: Option<&reqwest::header::HeaderMap>,
) -> Result<Response, SDKError> {
    let id = raw_json
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("msg_unknown")
        .to_string();
    let model = raw_json
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let mut content = Vec::new();
    if let Some(items) = raw_json.get("content").and_then(Value::as_array) {
        for item in items {
            match item.get("type").and_then(Value::as_str).unwrap_or_default() {
                "text" => {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        content.push(ContentPart::text(text.to_string()));
                    }
                }
                "tool_use" => {
                    content.push(ContentPart::tool_call(ToolCallData {
                        id: item
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("call_unknown")
                            .to_string(),
                        name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string(),
                        arguments: item.get("input").cloned().unwrap_or_else(|| json!({})),
                        r#type: "function".to_string(),
                    }));
                }
                "thinking" => {
                    content.push(thinking_content_part(ThinkingData {
                        text: item
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        signature: item
                            .get("signature")
                            .and_then(Value::as_str)
                            .map(ToString::to_string),
                        redacted: false,
                    }));
                }
                "redacted_thinking" => {
                    content.push(redacted_thinking_content_part(
                        item.get("data")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ));
                }
                _ => {}
            }
        }
    }

    let usage = parse_anthropic_usage(raw_json.get("usage"), &content);
    let finish_reason = map_finish_reason(raw_json.get("stop_reason").and_then(Value::as_str));

    Ok(Response {
        id,
        model,
        provider: provider.to_string(),
        message: Message {
            role: Role::Assistant,
            content,
            name: None,
            tool_call_id: None,
        },
        finish_reason,
        usage,
        raw: Some(raw_json),
        warnings: vec![],
        rate_limit: headers.and_then(parse_rate_limit_info),
    })
}

fn thinking_content_part(thinking: ThinkingData) -> ContentPart {
    ContentPart {
        kind: ContentKind::Thinking.into(),
        text: None,
        image: None,
        audio: None,
        document: None,
        tool_call: None,
        tool_result: None,
        thinking: Some(thinking),
    }
}

fn redacted_thinking_content_part(data: String) -> ContentPart {
    ContentPart {
        kind: ContentKind::RedactedThinking.into(),
        text: None,
        image: None,
        audio: None,
        document: None,
        tool_call: None,
        tool_result: None,
        thinking: Some(ThinkingData {
            text: data,
            signature: None,
            redacted: true,
        }),
    }
}

fn parse_anthropic_usage(raw_usage: Option<&Value>, content: &[ContentPart]) -> Usage {
    let usage = raw_usage.unwrap_or(&Value::Null);

    let input_tokens = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output_tokens = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();

    Usage {
        input_tokens,
        output_tokens,
        total_tokens: input_tokens + output_tokens,
        reasoning_tokens: estimate_reasoning_tokens(content),
        cache_read_tokens: usage.get("cache_read_input_tokens").and_then(Value::as_u64),
        cache_write_tokens: usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64),
        raw: if usage.is_null() {
            None
        } else {
            Some(usage.clone())
        },
    }
}

fn estimate_reasoning_tokens(content: &[ContentPart]) -> Option<u64> {
    let mut total = 0_u64;
    let mut saw_thinking = false;

    for part in content {
        if part.kind == ContentKind::Thinking.into() {
            if let Some(thinking) = &part.thinking {
                saw_thinking = true;
                total += estimate_token_count(&thinking.text);
            }
        }
    }

    if saw_thinking { Some(total) } else { None }
}

fn estimate_token_count(text: &str) -> u64 {
    if text.is_empty() {
        0
    } else {
        text.split_whitespace().count().max(1) as u64
    }
}

fn map_finish_reason(raw: Option<&str>) -> FinishReason {
    let reason = match raw {
        Some("end_turn") | Some("stop_sequence") | None => "stop",
        Some("max_tokens") => "length",
        Some("tool_use") => "tool_calls",
        _ => "other",
    };

    FinishReason {
        reason: reason.to_string(),
        raw: raw.map(ToString::to_string),
    }
}

fn build_provider_error(
    provider: &str,
    status: u16,
    body_text: &str,
    retry_after: Option<f64>,
) -> SDKError {
    let raw_json = serde_json::from_str::<Value>(body_text).ok();
    let message = raw_json
        .as_ref()
        .and_then(|json| json.get("error"))
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| {
            raw_json
                .as_ref()
                .and_then(|json| json.get("message"))
                .and_then(Value::as_str)
        })
        .unwrap_or(body_text)
        .to_string();

    let classification = map_http_status(status).or_else(|| {
        classify_message(&message).map(|kind| {
            let retryable = default_retryable_for_kind(&kind);
            HttpErrorClassification::Provider(kind, retryable)
        })
    });

    match classification {
        Some(HttpErrorClassification::RequestTimeout(retryable)) => {
            SDKError::RequestTimeout(crate::errors::RequestTimeoutError {
                info: crate::errors::ErrorInfo::new(message),
                retryable,
            })
        }
        Some(HttpErrorClassification::Provider(kind, retryable)) => {
            SDKError::Provider(ProviderError {
                info: crate::errors::ErrorInfo::new(message),
                provider: provider.to_string(),
                kind,
                status_code: Some(status),
                error_code: raw_json
                    .as_ref()
                    .and_then(|json| json.get("error"))
                    .and_then(|error| error.get("type"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                retryable,
                retry_after,
                raw: raw_json,
            })
        }
        None => SDKError::Provider(ProviderError {
            info: crate::errors::ErrorInfo::new(message),
            provider: provider.to_string(),
            kind: ProviderErrorKind::Other,
            status_code: Some(status),
            error_code: None,
            retryable: true,
            retry_after,
            raw: raw_json,
        }),
    }
}

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<f64> {
    headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<f64>().ok())
}

fn parse_rate_limit_info(headers: &reqwest::header::HeaderMap) -> Option<RateLimitInfo> {
    let requests_remaining = parse_u64_header(headers, "x-ratelimit-remaining-requests")
        .or_else(|| parse_u64_header(headers, "anthropic-ratelimit-requests-remaining"));
    let requests_limit = parse_u64_header(headers, "x-ratelimit-limit-requests")
        .or_else(|| parse_u64_header(headers, "anthropic-ratelimit-requests-limit"));
    let tokens_remaining = parse_u64_header(headers, "x-ratelimit-remaining-tokens")
        .or_else(|| parse_u64_header(headers, "anthropic-ratelimit-tokens-remaining"));
    let tokens_limit = parse_u64_header(headers, "x-ratelimit-limit-tokens")
        .or_else(|| parse_u64_header(headers, "anthropic-ratelimit-tokens-limit"));
    let reset_at = headers
        .get("x-ratelimit-reset-requests")
        .or_else(|| headers.get("anthropic-ratelimit-requests-reset"))
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);

    if requests_remaining.is_none()
        && requests_limit.is_none()
        && tokens_remaining.is_none()
        && tokens_limit.is_none()
        && reset_at.is_none()
    {
        None
    } else {
        Some(RateLimitInfo {
            requests_remaining,
            requests_limit,
            tokens_remaining,
            tokens_limit,
            reset_at,
        })
    }
}

fn parse_u64_header(headers: &reqwest::header::HeaderMap, name: &str) -> Option<u64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

struct AnthropicFactory;

impl ProviderFactory for AnthropicFactory {
    fn provider_id(&self) -> &'static str {
        "anthropic"
    }

    fn from_env(&self) -> Option<Arc<dyn ProviderAdapter>> {
        let config = AnthropicAdapterConfig::from_env()?;
        let adapter = AnthropicAdapter::new(config).ok()?;
        Some(Arc::new(adapter))
    }
}

static REGISTER_ANTHROPIC_FACTORY: Once = Once::new();

pub fn ensure_anthropic_factory_registered() {
    REGISTER_ANTHROPIC_FACTORY.call_once(|| {
        register_provider_factory(Arc::new(AnthropicFactory));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn base_request() -> Request {
        Request {
            model: "claude-sonnet-4-5".to_string(),
            messages: vec![Message::user("hello")],
            provider: Some("anthropic".to_string()),
            tools: None,
            tool_choice: None,
            response_format: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            stop_sequences: None,
            reasoning_effort: None,
            metadata: None,
            provider_options: None,
        }
    }

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

    #[test]
    fn build_messages_body_merges_consecutive_roles_and_maps_tool_results_to_user() {
        let mut request = base_request();
        request.messages = vec![
            Message::user("u1"),
            Message::user("u2"),
            Message::assistant("a1"),
            Message::tool_result("call_1", "result", false),
            Message::assistant("a2"),
        ];

        let prepared = build_messages_body(&request, false).expect("body");
        let messages = prepared
            .body
            .get("messages")
            .and_then(Value::as_array)
            .expect("messages array");

        assert_eq!(messages.len(), 4);
        assert_eq!(
            messages[0].get("role").and_then(Value::as_str),
            Some("user")
        );
        assert_eq!(
            messages[1].get("role").and_then(Value::as_str),
            Some("assistant")
        );
        assert_eq!(
            messages[2].get("role").and_then(Value::as_str),
            Some("user")
        );
        assert_eq!(
            messages[3].get("role").and_then(Value::as_str),
            Some("assistant")
        );

        let tool_block = messages[2]
            .get("content")
            .and_then(Value::as_array)
            .and_then(|content| content.first())
            .expect("tool_result block");
        assert_eq!(
            tool_block.get("type").and_then(Value::as_str),
            Some("tool_result")
        );
    }

    #[test]
    fn build_messages_body_injects_cache_controls_and_prompt_caching_beta_header() {
        let mut request = base_request();
        request.messages = vec![Message::system("sys"), Message::user("user")];
        request.tools = Some(vec![crate::types::ToolDefinition {
            name: "calc".to_string(),
            description: "calc".to_string(),
            parameters: json!({"type":"object"}),
        }]);

        let prepared = build_messages_body(&request, false).expect("body");
        assert!(
            prepared
                .beta_headers
                .iter()
                .any(|header| header == PROMPT_CACHING_BETA)
        );

        let system = prepared
            .body
            .get("system")
            .and_then(Value::as_array)
            .and_then(|blocks| blocks.first())
            .expect("system block");
        assert!(system.get("cache_control").is_some());

        let tool = prepared
            .body
            .get("tools")
            .and_then(Value::as_array)
            .and_then(|tools| tools.first())
            .expect("tool block");
        assert!(tool.get("cache_control").is_some());

        let msg_first_part = prepared
            .body
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|msgs| msgs.first())
            .and_then(|msg| msg.get("content"))
            .and_then(Value::as_array)
            .and_then(|parts| parts.first())
            .expect("message part");
        assert!(msg_first_part.get("cache_control").is_some());
    }

    #[test]
    fn parse_anthropic_response_round_trips_thinking_and_tool_use() {
        let raw = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-5",
            "content": [
                {"type":"thinking","thinking":"think step by step","signature":"sig_1"},
                {"type":"redacted_thinking","data":"opaque"},
                {"type":"tool_use","id":"call_1","name":"calc","input":{"x":2}},
                {"type":"text","text":"done"}
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 8,
                "cache_creation_input_tokens": 100,
                "cache_read_input_tokens": 200
            }
        });

        let response = parse_anthropic_response(raw, "anthropic", None).expect("response");
        assert_eq!(response.finish_reason.reason, "tool_calls");
        assert!(response.reasoning().is_some());
        assert_eq!(response.tool_calls().len(), 1);
        assert_eq!(response.usage.cache_write_tokens, Some(100));
        assert_eq!(response.usage.cache_read_tokens, Some(200));
        assert!(response.usage.reasoning_tokens.unwrap_or_default() > 0);

        let redacted = response
            .message
            .content
            .iter()
            .find(|part| part.kind == ContentKind::RedactedThinking.into())
            .expect("redacted thinking");
        assert_eq!(
            redacted.thinking.as_ref().map(|t| t.text.as_str()),
            Some("opaque")
        );
    }

    #[test]
    fn build_messages_body_adds_json_schema_hint_to_system() {
        let mut request = base_request();
        request.response_format = Some(crate::types::ResponseFormat {
            r#type: "json_schema".to_string(),
            json_schema: Some(json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                },
                "required": ["name"]
            })),
            strict: true,
        });

        let prepared = build_messages_body(&request, false).expect("body");
        let system = prepared
            .body
            .get("system")
            .and_then(Value::as_array)
            .expect("system");
        assert!(system.iter().any(|entry| {
            entry
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("strictly matches this schema")
        }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn anthropic_adapter_stream_emits_reasoning_and_tool_events() {
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
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"calc\",\"input\":{\"x\":2}}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":4}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n"
        )
        .to_string();

        let base_url =
            spawn_single_response_server(200, "text/event-stream", sse_body, "/messages");
        let mut config = AnthropicAdapterConfig::new("test-key");
        config.base_url = base_url;
        let adapter = AnthropicAdapter::new(config).expect("adapter");

        let mut saw_reasoning_delta = false;
        let mut saw_tool_end = false;
        let mut saw_finish = false;

        let mut stream = adapter.stream(base_request()).await.expect("stream");
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
                        .map(|r| r.finish_reason.reason.as_str()),
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
    async fn anthropic_adapter_complete_sends_beta_header_from_provider_options() {
        let (base_url, rx) = spawn_capture_server();
        let mut config = AnthropicAdapterConfig::new("test-key");
        config.base_url = base_url;
        let adapter = AnthropicAdapter::new(config).expect("adapter");

        let mut request = base_request();
        request.provider_options = Some(json!({
            "anthropic": {
                "beta_headers": ["interleaved-thinking-2025-05-14"],
                "auto_cache": false
            }
        }));

        let _ = adapter.complete(request).await.expect("complete");
        let captured = rx.recv().expect("captured request");
        assert!(captured.contains("anthropic-beta: interleaved-thinking-2025-05-14"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn anthropic_stream_emits_error_event_and_then_closes() {
        let sse_body =
            "event: message_start\ndata: {\"type\":\"message_start\"}\n\ndata: {not-json}\n\n"
                .to_string();
        let base_url =
            spawn_single_response_server(200, "text/event-stream", sse_body, "/messages");
        let mut config = AnthropicAdapterConfig::new("test-key");
        config.base_url = base_url;
        let adapter = AnthropicAdapter::new(config).expect("adapter");

        let mut stream = adapter.stream(base_request()).await.expect("stream");
        let mut saw_error_event = false;
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) => {
                    if event.event_type == StreamEventTypeOrString::Known(StreamEventType::Error) {
                        saw_error_event = true;
                    }
                }
                Err(_) => panic!("did not expect terminal Err after error event"),
            }
        }

        assert!(saw_error_event);
    }
}
