//! OpenAI adapters:
//! - `OpenAIAdapter` for OpenAI Responses API (`/v1/responses`)
//! - `OpenAICompatibleAdapter` for OpenAI-compatible chat completions endpoints

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Once};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::channel::mpsc;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
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
    ToolCall, ToolCallData, Usage,
};
use crate::utils::{SseEvent, SseParser, is_local_path, load_file_data};

#[derive(Clone, Debug)]
pub struct OpenAIAdapterConfig {
    pub api_key: String,
    pub base_url: String,
    pub org_id: Option<String>,
    pub project_id: Option<String>,
    pub timeout: AdapterTimeout,
}

impl OpenAIAdapterConfig {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.openai.com/v1".to_string(),
            org_id: None,
            project_id: None,
            timeout: AdapterTimeout::default(),
        }
    }

    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("OPENAI_API_KEY").ok()?;
        let mut config = Self::new(api_key);
        if let Ok(base_url) = std::env::var("OPENAI_BASE_URL") {
            config.base_url = base_url;
        }
        if let Ok(org_id) = std::env::var("OPENAI_ORG_ID") {
            config.org_id = Some(org_id);
        }
        if let Ok(project_id) = std::env::var("OPENAI_PROJECT_ID") {
            config.project_id = Some(project_id);
        }
        Some(config)
    }
}

#[derive(Clone)]
pub struct OpenAIAdapter {
    client: reqwest::Client,
    config: OpenAIAdapterConfig,
}

impl std::fmt::Debug for OpenAIAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAIAdapter")
            .field("base_url", &self.config.base_url)
            .field("org_id", &self.config.org_id)
            .field("project_id", &self.config.project_id)
            .field("timeout", &self.config.timeout)
            .finish()
    }
}

impl OpenAIAdapter {
    pub fn new(config: OpenAIAdapterConfig) -> Result<Self, SDKError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.api_key)).map_err(|error| {
                SDKError::Configuration(ConfigurationError::new(format!(
                    "invalid OpenAI API key header: {}",
                    error
                )))
            })?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(org_id) = &config.org_id {
            headers.insert(
                "OpenAI-Organization",
                HeaderValue::from_str(org_id).map_err(|error| {
                    SDKError::Configuration(ConfigurationError::new(format!(
                        "invalid OPENAI_ORG_ID header: {}",
                        error
                    )))
                })?,
            );
        }
        if let Some(project_id) = &config.project_id {
            headers.insert(
                "OpenAI-Project",
                HeaderValue::from_str(project_id).map_err(|error| {
                    SDKError::Configuration(ConfigurationError::new(format!(
                        "invalid OPENAI_PROJECT_ID header: {}",
                        error
                    )))
                })?,
            );
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs_f64(config.timeout.connect))
            .timeout(Duration::from_secs_f64(config.timeout.request))
            .default_headers(headers)
            .build()
            .map_err(|error| SDKError::Network(NetworkError::new(error.to_string())))?;
        Ok(Self { client, config })
    }

    fn endpoint(&self) -> String {
        format!("{}/responses", self.config.base_url.trim_end_matches('/'))
    }
}

#[async_trait]
impl ProviderAdapter for OpenAIAdapter {
    fn name(&self) -> &str {
        "openai"
    }

    async fn complete(&self, request: Request) -> Result<Response, SDKError> {
        let body = build_responses_body(&request, false)?;
        let response = self
            .client
            .post(self.endpoint())
            .json(&body)
            .send()
            .await
            .map_err(|error| SDKError::Network(NetworkError::new(error.to_string())))?;

        let headers = response.headers().clone();
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let retry_after = parse_retry_after(response.headers());
            let raw = response.text().await.unwrap_or_default();
            return Err(build_provider_error("openai", status, &raw, retry_after));
        }

        let raw_json = response
            .json::<Value>()
            .await
            .map_err(|error| SDKError::Network(NetworkError::new(error.to_string())))?;
        parse_responses_api_response(raw_json, "openai", Some(&headers))
    }

    async fn stream(&self, request: Request) -> Result<StreamEventStream, SDKError> {
        let body = build_responses_body(&request, true)?;
        let response = self
            .client
            .post(self.endpoint())
            .json(&body)
            .send()
            .await
            .map_err(|error| SDKError::Network(NetworkError::new(error.to_string())))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let retry_after = parse_retry_after(response.headers());
            let raw = response.text().await.unwrap_or_default();
            return Err(build_provider_error("openai", status, &raw, retry_after));
        }

        let mut byte_stream = response.bytes_stream();
        let (tx, rx) = mpsc::unbounded::<Result<StreamEvent, SDKError>>();
        let stream_read_timeout = Duration::from_secs_f64(self.config.timeout.stream_read);
        tokio::spawn(async move {
            let mut parser = SseParser::new();
            let mut state = OpenAIStreamState::default();
            let mut tx = tx;

            loop {
                let next_item =
                    match tokio::time::timeout(stream_read_timeout, byte_stream.next()).await {
                        Ok(item) => item,
                        Err(_) => {
                            let _ = send_terminal_stream_error(
                                &mut tx,
                                SDKError::Stream(StreamError::new("OpenAI stream read timed out")),
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
                if process_openai_sse_events(&events, &mut state, &mut tx).is_err() {
                    return;
                }
            }

            if let Some(event) = parser.finish() {
                let _ = process_openai_sse_events(&[event], &mut state, &mut tx);
            }
        });

        Ok(Box::pin(rx))
    }

    fn supports_tool_choice(&self, mode: &str) -> bool {
        matches!(mode, "auto" | "none" | "required" | "named")
    }
}

#[derive(Clone, Debug)]
pub struct OpenAICompatibleAdapterConfig {
    pub api_key: String,
    pub base_url: String,
    pub timeout: AdapterTimeout,
}

impl OpenAICompatibleAdapterConfig {
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            timeout: AdapterTimeout::default(),
        }
    }
}

#[derive(Clone)]
pub struct OpenAICompatibleAdapter {
    client: reqwest::Client,
    config: OpenAICompatibleAdapterConfig,
}

impl std::fmt::Debug for OpenAICompatibleAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAICompatibleAdapter")
            .field("base_url", &self.config.base_url)
            .field("timeout", &self.config.timeout)
            .finish()
    }
}

impl OpenAICompatibleAdapter {
    pub fn new(config: OpenAICompatibleAdapterConfig) -> Result<Self, SDKError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.api_key)).map_err(|error| {
                SDKError::Configuration(ConfigurationError::new(format!(
                    "invalid compatible API key header: {}",
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
        format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
    }
}

#[async_trait]
impl ProviderAdapter for OpenAICompatibleAdapter {
    fn name(&self) -> &str {
        "openai-compatible"
    }

    async fn complete(&self, request: Request) -> Result<Response, SDKError> {
        let body = build_chat_completions_body(&request, false)?;
        let response = self
            .client
            .post(self.endpoint())
            .json(&body)
            .send()
            .await
            .map_err(|error| SDKError::Network(NetworkError::new(error.to_string())))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let retry_after = parse_retry_after(response.headers());
            let raw = response.text().await.unwrap_or_default();
            return Err(build_provider_error(
                "openai-compatible",
                status,
                &raw,
                retry_after,
            ));
        }

        let raw_json = response
            .json::<Value>()
            .await
            .map_err(|error| SDKError::Network(NetworkError::new(error.to_string())))?;
        parse_chat_completions_response(raw_json, "openai-compatible")
    }

    async fn stream(&self, request: Request) -> Result<StreamEventStream, SDKError> {
        let body = build_chat_completions_body(&request, true)?;
        let response = self
            .client
            .post(self.endpoint())
            .json(&body)
            .send()
            .await
            .map_err(|error| SDKError::Network(NetworkError::new(error.to_string())))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let retry_after = parse_retry_after(response.headers());
            let raw = response.text().await.unwrap_or_default();
            return Err(build_provider_error(
                "openai-compatible",
                status,
                &raw,
                retry_after,
            ));
        }

        let mut byte_stream = response.bytes_stream();
        let (tx, rx) = mpsc::unbounded::<Result<StreamEvent, SDKError>>();
        let stream_read_timeout = Duration::from_secs_f64(self.config.timeout.stream_read);
        tokio::spawn(async move {
            let mut parser = SseParser::new();
            let mut state = CompatibleStreamState::default();
            let mut tx = tx;
            loop {
                let next_item =
                    match tokio::time::timeout(stream_read_timeout, byte_stream.next()).await {
                        Ok(item) => item,
                        Err(_) => {
                            let _ = send_terminal_stream_error(
                                &mut tx,
                                SDKError::Stream(StreamError::new(
                                    "OpenAI-compatible stream read timed out",
                                )),
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
                if process_compatible_sse_events(&events, &mut state, &mut tx).is_err() {
                    return;
                }
            }
            if let Some(event) = parser.finish() {
                let _ = process_compatible_sse_events(&[event], &mut state, &mut tx);
            }
            let _ = emit_compatible_finish_if_needed(&mut state, &mut tx);
        });

        Ok(Box::pin(rx))
    }

    fn supports_tool_choice(&self, mode: &str) -> bool {
        matches!(mode, "auto" | "none" | "required" | "named")
    }
}

#[derive(Default)]
struct OpenAIStreamState {
    text_started: HashSet<String>,
    function_calls: HashMap<String, ToolCall>,
}

#[derive(Default)]
struct CompatibleStreamState {
    response_id: Option<String>,
    model: Option<String>,
    provider: String,
    text_started: bool,
    text: String,
    tool_calls: HashMap<usize, ToolCall>,
    finish_reason: Option<String>,
    usage: Option<Usage>,
    finish_emitted: bool,
}

fn process_openai_sse_events(
    events: &[SseEvent],
    state: &mut OpenAIStreamState,
    tx: &mut mpsc::UnboundedSender<Result<StreamEvent, SDKError>>,
) -> Result<(), ()> {
    for event in events {
        if event.data.trim().is_empty() {
            continue;
        }
        if event.data.trim() == "[DONE]" {
            continue;
        }
        let payload: Value = match serde_json::from_str(&event.data) {
            Ok(value) => value,
            Err(error) => {
                let _ = send_terminal_stream_error(
                    tx,
                    SDKError::Stream(StreamError::new(format!(
                        "invalid OpenAI SSE event JSON: {}",
                        error
                    ))),
                );
                return Err(());
            }
        };
        let event_type = payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match event_type {
            "response.created" => {
                if tx
                    .unbounded_send(Ok(stream_event(StreamEventType::StreamStart)))
                    .is_err()
                {
                    return Err(());
                }
            }
            "response.output_text.delta" => {
                let text_id = payload
                    .get("item_id")
                    .and_then(Value::as_str)
                    .unwrap_or("text_0")
                    .to_string();
                if !state.text_started.contains(&text_id) {
                    state.text_started.insert(text_id.clone());
                    if tx
                        .unbounded_send(Ok(StreamEvent {
                            event_type: StreamEventTypeOrString::Known(StreamEventType::TextStart),
                            delta: None,
                            text_id: Some(text_id.clone()),
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
                let delta = payload
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if tx
                    .unbounded_send(Ok(StreamEvent {
                        event_type: StreamEventTypeOrString::Known(StreamEventType::TextDelta),
                        delta: Some(delta),
                        text_id: Some(text_id),
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
            "response.output_text.done" => {
                let text_id = payload
                    .get("item_id")
                    .and_then(Value::as_str)
                    .unwrap_or("text_0")
                    .to_string();
                if tx
                    .unbounded_send(Ok(StreamEvent {
                        event_type: StreamEventTypeOrString::Known(StreamEventType::TextEnd),
                        delta: None,
                        text_id: Some(text_id),
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
            "response.reasoning_summary_text.delta" => {
                let delta = payload
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if tx
                    .unbounded_send(Ok(StreamEvent {
                        event_type: StreamEventTypeOrString::Known(StreamEventType::ReasoningDelta),
                        delta: None,
                        text_id: None,
                        reasoning_delta: Some(delta),
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
            "response.output_item.added" => {
                if let Some(item) = payload.get("item") {
                    if item.get("type").and_then(Value::as_str) == Some("function_call") {
                        let call_id = item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .or_else(|| item.get("id").and_then(Value::as_str))
                            .unwrap_or("call_unknown")
                            .to_string();
                        let tool_call = ToolCall {
                            id: call_id.clone(),
                            name: item
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown")
                                .to_string(),
                            arguments: Value::Object(Default::default()),
                            raw_arguments: item
                                .get("arguments")
                                .and_then(Value::as_str)
                                .map(ToString::to_string),
                        };
                        state.function_calls.insert(call_id, tool_call.clone());
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
                }
            }
            "response.function_call_arguments.delta" => {
                let call_id = payload
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("call_unknown")
                    .to_string();
                let delta = payload
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let entry = state
                    .function_calls
                    .entry(call_id.clone())
                    .or_insert_with(|| ToolCall {
                        id: call_id.clone(),
                        name: "unknown".to_string(),
                        arguments: Value::Object(Default::default()),
                        raw_arguments: Some(String::new()),
                    });
                let mut raw = entry.raw_arguments.clone().unwrap_or_default();
                raw.push_str(&delta);
                entry.raw_arguments = Some(raw);
                if tx
                    .unbounded_send(Ok(StreamEvent {
                        event_type: StreamEventTypeOrString::Known(StreamEventType::ToolCallDelta),
                        delta: None,
                        text_id: None,
                        reasoning_delta: None,
                        tool_call: Some(entry.clone()),
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
            "response.output_item.done" => {
                if let Some(item) = payload.get("item") {
                    if item.get("type").and_then(Value::as_str) == Some("function_call") {
                        let call_id = item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .or_else(|| item.get("id").and_then(Value::as_str))
                            .unwrap_or("call_unknown")
                            .to_string();
                        let raw_arguments = item
                            .get("arguments")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                            .or_else(|| {
                                state
                                    .function_calls
                                    .get(&call_id)
                                    .and_then(|call| call.raw_arguments.clone())
                            });
                        let tool_call = ToolCall {
                            id: call_id.clone(),
                            name: item
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown")
                                .to_string(),
                            arguments: raw_arguments
                                .as_deref()
                                .and_then(|value| serde_json::from_str::<Value>(value).ok())
                                .unwrap_or_else(|| Value::Object(Default::default())),
                            raw_arguments,
                        };
                        state.function_calls.insert(call_id, tool_call.clone());
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
            }
            "response.completed" | "response.incomplete" => {
                let Some(response_object) = payload.get("response").cloned() else {
                    continue;
                };
                let parsed = match parse_responses_api_response(response_object, "openai", None) {
                    Ok(response) => response,
                    Err(error) => {
                        let _ = send_terminal_stream_error(tx, error);
                        return Err(());
                    }
                };
                if tx
                    .unbounded_send(Ok(StreamEvent {
                        event_type: StreamEventTypeOrString::Known(StreamEventType::Finish),
                        delta: None,
                        text_id: None,
                        reasoning_delta: None,
                        tool_call: None,
                        finish_reason: Some(parsed.finish_reason.clone()),
                        usage: Some(parsed.usage.clone()),
                        response: Some(parsed),
                        error: None,
                        raw: None,
                    }))
                    .is_err()
                {
                    return Err(());
                }
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

fn process_compatible_sse_events(
    events: &[SseEvent],
    state: &mut CompatibleStreamState,
    tx: &mut mpsc::UnboundedSender<Result<StreamEvent, SDKError>>,
) -> Result<(), ()> {
    if state.provider.is_empty() {
        state.provider = "openai-compatible".to_string();
    }

    for event in events {
        let data = event.data.trim();
        if data.is_empty() {
            continue;
        }
        if data == "[DONE]" {
            continue;
        }
        let payload: Value = match serde_json::from_str(data) {
            Ok(value) => value,
            Err(error) => {
                let _ = send_terminal_stream_error(
                    tx,
                    SDKError::Stream(StreamError::new(format!(
                        "invalid compatible SSE JSON: {}",
                        error
                    ))),
                );
                return Err(());
            }
        };
        if let Some(id) = payload.get("id").and_then(Value::as_str) {
            state.response_id = Some(id.to_string());
        }
        if let Some(model) = payload.get("model").and_then(Value::as_str) {
            state.model = Some(model.to_string());
        }
        if let Some(usage) = payload.get("usage") {
            state.usage = Some(parse_chat_usage(usage));
        }
        if let Some(choices) = payload.get("choices").and_then(Value::as_array) {
            for choice in choices {
                if let Some(delta) = choice.get("delta") {
                    if let Some(content) = delta.get("content").and_then(Value::as_str) {
                        if !state.text_started {
                            state.text_started = true;
                            if tx
                                .unbounded_send(Ok(StreamEvent {
                                    event_type: StreamEventTypeOrString::Known(
                                        StreamEventType::TextStart,
                                    ),
                                    delta: None,
                                    text_id: Some("text_0".to_string()),
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
                        state.text.push_str(content);
                        if tx
                            .unbounded_send(Ok(StreamEvent {
                                event_type: StreamEventTypeOrString::Known(
                                    StreamEventType::TextDelta,
                                ),
                                delta: Some(content.to_string()),
                                text_id: Some("text_0".to_string()),
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

                    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                        for item in tool_calls {
                            let index =
                                item.get("index")
                                    .and_then(Value::as_u64)
                                    .unwrap_or_default() as usize;
                            let call_id = item
                                .get("id")
                                .and_then(Value::as_str)
                                .map(ToString::to_string)
                                .or_else(|| {
                                    state.tool_calls.get(&index).map(|call| call.id.clone())
                                })
                                .unwrap_or_else(|| format!("call_{}", index));
                            let name = item
                                .get("function")
                                .and_then(|function| function.get("name"))
                                .and_then(Value::as_str)
                                .map(ToString::to_string)
                                .or_else(|| {
                                    state.tool_calls.get(&index).map(|call| call.name.clone())
                                })
                                .unwrap_or_else(|| "unknown".to_string());
                            let arguments_delta = item
                                .get("function")
                                .and_then(|function| function.get("arguments"))
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let mut current =
                                state.tool_calls.get(&index).cloned().unwrap_or(ToolCall {
                                    id: call_id.clone(),
                                    name,
                                    arguments: Value::Object(Default::default()),
                                    raw_arguments: Some(String::new()),
                                });
                            let mut raw = current.raw_arguments.clone().unwrap_or_default();
                            raw.push_str(arguments_delta);
                            current.raw_arguments = Some(raw);
                            state.tool_calls.insert(index, current.clone());

                            if !item.get("id").is_none() {
                                if tx
                                    .unbounded_send(Ok(StreamEvent {
                                        event_type: StreamEventTypeOrString::Known(
                                            StreamEventType::ToolCallStart,
                                        ),
                                        delta: None,
                                        text_id: None,
                                        reasoning_delta: None,
                                        tool_call: Some(current.clone()),
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

                            if tx
                                .unbounded_send(Ok(StreamEvent {
                                    event_type: StreamEventTypeOrString::Known(
                                        StreamEventType::ToolCallDelta,
                                    ),
                                    delta: None,
                                    text_id: None,
                                    reasoning_delta: None,
                                    tool_call: Some(current),
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
                }
                if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
                    if !finish_reason.is_empty() {
                        state.finish_reason = Some(finish_reason.to_string());
                    }
                }
            }
        }
    }
    Ok(())
}

fn emit_compatible_finish_if_needed(
    state: &mut CompatibleStreamState,
    tx: &mut mpsc::UnboundedSender<Result<StreamEvent, SDKError>>,
) -> Result<(), ()> {
    if state.finish_emitted {
        return Ok(());
    }
    state.finish_emitted = true;

    if state.text_started {
        tx.unbounded_send(Ok(StreamEvent {
            event_type: StreamEventTypeOrString::Known(StreamEventType::TextEnd),
            delta: None,
            text_id: Some("text_0".to_string()),
            reasoning_delta: None,
            tool_call: None,
            finish_reason: None,
            usage: None,
            response: None,
            error: None,
            raw: None,
        }))
        .map_err(|_| ())?;
    }

    let mut content = Vec::new();
    if !state.text.is_empty() {
        content.push(ContentPart::text(state.text.clone()));
    }
    let mut ordered: Vec<_> = state.tool_calls.iter().collect();
    ordered.sort_by_key(|(index, _)| **index);
    for (_, tool_call) in ordered {
        let parsed_arguments = tool_call
            .raw_arguments
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .unwrap_or_else(|| Value::Object(Default::default()));
        content.push(ContentPart::tool_call(ToolCallData {
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            arguments: parsed_arguments,
            r#type: "function".to_string(),
        }));
    }

    let reason = match state.finish_reason.as_deref() {
        Some("length") => "length",
        Some("tool_calls") => "tool_calls",
        Some("content_filter") => "content_filter",
        Some("stop") | Some("end_turn") | Some("stop_sequence") | None => {
            if !state.tool_calls.is_empty() {
                "tool_calls"
            } else {
                "stop"
            }
        }
        _ => "other",
    };

    let response = Response {
        id: state
            .response_id
            .clone()
            .unwrap_or_else(|| "chatcmpl_unknown".to_string()),
        model: state.model.clone().unwrap_or_else(|| "unknown".to_string()),
        provider: state.provider.clone(),
        message: Message {
            role: Role::Assistant,
            content,
            name: None,
            tool_call_id: None,
        },
        finish_reason: FinishReason {
            reason: reason.to_string(),
            raw: state.finish_reason.clone(),
        },
        usage: state.usage.clone().unwrap_or_else(zero_usage),
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
        finish_reason: Some(response.finish_reason.clone()),
        usage: Some(response.usage.clone()),
        response: Some(response),
        error: None,
        raw: None,
    }))
    .map_err(|_| ())
}

fn stream_event(kind: StreamEventType) -> StreamEvent {
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

fn build_responses_body(request: &Request, stream: bool) -> Result<Value, SDKError> {
    let mut body = json!({
        "model": request.model,
        "stream": stream,
    });

    let (instructions, input) = translate_messages_to_responses_input(&request.messages)?;
    if let Some(instructions) = instructions {
        body["instructions"] = Value::String(instructions);
    }
    body["input"] = input;

    if let Some(tools) = &request.tools {
        body["tools"] = Value::Array(
            tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters
                    })
                })
                .collect(),
        );
    }
    if let Some(tool_choice) = &request.tool_choice {
        body["tool_choice"] = translate_tool_choice(tool_choice);
    }
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        body["top_p"] = json!(top_p);
    }
    if let Some(max_tokens) = request.max_tokens {
        body["max_output_tokens"] = json!(max_tokens);
    }
    if let Some(stop_sequences) = &request.stop_sequences {
        body["stop"] = json!(stop_sequences);
    }
    if let Some(reasoning_effort) = &request.reasoning_effort {
        body["reasoning"] = json!({ "effort": reasoning_effort });
    }
    if let Some(metadata) = &request.metadata {
        body["metadata"] = json!(metadata);
    }
    if let Some(response_format) = &request.response_format {
        apply_responses_response_format(&mut body, response_format)?;
    }
    if let Some(provider_options) = &request.provider_options {
        if let Some(openai_options) = provider_options.get("openai") {
            if let Some(object) = openai_options.as_object() {
                for (key, value) in object {
                    body[key] = value.clone();
                }
            }
        }
    }

    Ok(body)
}

fn build_chat_completions_body(request: &Request, stream: bool) -> Result<Value, SDKError> {
    let messages = translate_messages_to_chat_messages(&request.messages)?;
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": stream,
    });
    if let Some(tools) = &request.tools {
        body["tools"] = Value::Array(
            tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters
                        }
                    })
                })
                .collect(),
        );
    }
    if let Some(tool_choice) = &request.tool_choice {
        body["tool_choice"] = translate_tool_choice(tool_choice);
    }
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        body["top_p"] = json!(top_p);
    }
    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if let Some(stop_sequences) = &request.stop_sequences {
        body["stop"] = json!(stop_sequences);
    }
    if let Some(metadata) = &request.metadata {
        body["metadata"] = json!(metadata);
    }
    if let Some(response_format) = &request.response_format {
        apply_chat_completions_response_format(&mut body, response_format)?;
    }
    if let Some(provider_options) = &request.provider_options {
        if let Some(openai_options) = provider_options.get("openai") {
            if let Some(object) = openai_options.as_object() {
                for (key, value) in object {
                    body[key] = value.clone();
                }
            }
        }
    }
    Ok(body)
}

fn apply_responses_response_format(
    body: &mut Value,
    response_format: &crate::types::ResponseFormat,
) -> Result<(), SDKError> {
    match response_format.r#type.as_str() {
        "text" => {}
        "json" => {
            body["text"] = json!({
                "format": { "type": "json_object" }
            });
        }
        "json_schema" => {
            let schema = response_format.json_schema.clone().ok_or_else(|| {
                SDKError::Configuration(ConfigurationError::new(
                    "response_format type 'json_schema' requires json_schema",
                ))
            })?;
            body["text"] = json!({
                "format": {
                    "type": "json_schema",
                    "name": "output",
                    "schema": schema,
                    "strict": response_format.strict
                }
            });
        }
        other => {
            return Err(SDKError::Configuration(ConfigurationError::new(format!(
                "unsupported response_format type '{}'",
                other
            ))));
        }
    }
    Ok(())
}

fn apply_chat_completions_response_format(
    body: &mut Value,
    response_format: &crate::types::ResponseFormat,
) -> Result<(), SDKError> {
    match response_format.r#type.as_str() {
        "text" => {}
        "json" => {
            body["response_format"] = json!({ "type": "json_object" });
        }
        "json_schema" => {
            let schema = response_format.json_schema.clone().ok_or_else(|| {
                SDKError::Configuration(ConfigurationError::new(
                    "response_format type 'json_schema' requires json_schema",
                ))
            })?;
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "output",
                    "schema": schema,
                    "strict": response_format.strict
                }
            });
        }
        other => {
            return Err(SDKError::Configuration(ConfigurationError::new(format!(
                "unsupported response_format type '{}'",
                other
            ))));
        }
    }
    Ok(())
}

fn translate_messages_to_responses_input(
    messages: &[Message],
) -> Result<(Option<String>, Value), SDKError> {
    let mut instructions = Vec::new();
    let mut input = Vec::new();

    for message in messages {
        match message.role {
            Role::System | Role::Developer => {
                let text = message.text();
                if !text.is_empty() {
                    instructions.push(text);
                }
            }
            Role::Tool => {
                for part in &message.content {
                    if let Some(tool_result) = &part.tool_result {
                        input.push(json!({
                            "type": "function_call_output",
                            "call_id": tool_result.tool_call_id,
                            "output": tool_result.content.to_string()
                        }));
                    }
                }
            }
            Role::User | Role::Assistant => {
                let content =
                    translate_parts_to_responses_content(message.role.clone(), &message.content)?;
                input.push(json!({
                    "type": "message",
                    "role": match message.role {
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        _ => "user"
                    },
                    "content": content
                }));

                if message.role == Role::Assistant {
                    for part in &message.content {
                        if let Some(tool_call) = &part.tool_call {
                            input.push(json!({
                                "type": "function_call",
                                "call_id": tool_call.id,
                                "name": tool_call.name,
                                "arguments": tool_call.arguments.to_string()
                            }));
                        }
                    }
                }
            }
        }
    }

    let instructions = if instructions.is_empty() {
        None
    } else {
        Some(instructions.join("\n\n"))
    };

    Ok((instructions, Value::Array(input)))
}

fn translate_parts_to_responses_content(
    role: Role,
    parts: &[ContentPart],
) -> Result<Value, SDKError> {
    let mut out = Vec::new();
    for part in parts {
        if part.kind == ContentKind::Text.into() {
            if let Some(text) = &part.text {
                let kind = if role == Role::Assistant {
                    "output_text"
                } else {
                    "input_text"
                };
                out.push(json!({
                    "type": kind,
                    "text": text
                }));
            }
        } else if part.kind == ContentKind::Image.into() {
            if let Some(image) = &part.image {
                let image_url = if let Some(url) = &image.url {
                    if is_local_path(url) {
                        let path = std::path::PathBuf::from(url);
                        let file_data = load_file_data(&path).map_err(|error| {
                            SDKError::Configuration(ConfigurationError::new(format!(
                                "failed to read image path '{}': {}",
                                url, error
                            )))
                        })?;
                        format!(
                            "data:{};base64,{}",
                            file_data
                                .media_type
                                .unwrap_or_else(|| "image/png".to_string()),
                            file_data.base64
                        )
                    } else {
                        url.clone()
                    }
                } else if let Some(data) = &image.data {
                    let encoded =
                        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data);
                    format!(
                        "data:{};base64,{}",
                        image
                            .media_type
                            .clone()
                            .unwrap_or_else(|| "image/png".to_string()),
                        encoded
                    )
                } else {
                    continue;
                };
                out.push(json!({
                    "type": "input_image",
                    "image_url": image_url
                }));
            }
        }
    }
    Ok(Value::Array(out))
}

fn translate_messages_to_chat_messages(messages: &[Message]) -> Result<Value, SDKError> {
    let mut out = Vec::new();
    for message in messages {
        match message.role {
            Role::System | Role::Developer => {
                out.push(json!({
                    "role": "system",
                    "content": message.text()
                }));
            }
            Role::User => {
                out.push(json!({
                    "role": "user",
                    "content": message.text()
                }));
            }
            Role::Assistant => {
                let mut tool_calls = Vec::new();
                for part in &message.content {
                    if let Some(tool_call) = &part.tool_call {
                        tool_calls.push(json!({
                            "id": tool_call.id,
                            "type": "function",
                            "function": {
                                "name": tool_call.name,
                                "arguments": tool_call.arguments.to_string()
                            }
                        }));
                    }
                }
                let mut item = json!({
                    "role": "assistant",
                    "content": message.text()
                });
                if !tool_calls.is_empty() {
                    item["tool_calls"] = Value::Array(tool_calls);
                }
                out.push(item);
            }
            Role::Tool => {
                for part in &message.content {
                    if let Some(tool_result) = &part.tool_result {
                        out.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_result.tool_call_id,
                            "content": tool_result.content.to_string()
                        }));
                    }
                }
            }
        }
    }
    Ok(Value::Array(out))
}

fn translate_tool_choice(choice: &crate::types::ToolChoice) -> Value {
    match choice.mode.as_str() {
        "auto" => Value::String("auto".to_string()),
        "none" => Value::String("none".to_string()),
        "required" => Value::String("required".to_string()),
        "named" => {
            if let Some(tool_name) = &choice.tool_name {
                json!({
                    "type": "function",
                    "function": { "name": tool_name }
                })
            } else {
                Value::String("auto".to_string())
            }
        }
        _ => Value::String(choice.mode.clone()),
    }
}

fn parse_responses_api_response(
    raw_json: Value,
    provider: &str,
    headers: Option<&reqwest::header::HeaderMap>,
) -> Result<Response, SDKError> {
    let id = raw_json
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("resp_unknown")
        .to_string();
    let model = raw_json
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let output = raw_json
        .get("output")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut message_parts = Vec::new();
    let mut has_tool_calls = false;
    for item in &output {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if part.get("type").and_then(Value::as_str) == Some("output_text") {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                message_parts.push(ContentPart::text(text.to_string()));
                            }
                        } else if part.get("type").and_then(Value::as_str)
                            == Some("reasoning_summary_text")
                        {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                message_parts.push(ContentPart::thinking(
                                    crate::types::ThinkingData {
                                        text: text.to_string(),
                                        signature: None,
                                        redacted: false,
                                    },
                                ));
                            }
                        }
                    }
                }
            }
            Some("function_call") => {
                has_tool_calls = true;
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("id").and_then(Value::as_str))
                    .unwrap_or("call_unknown");
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                    .unwrap_or_else(|| Value::Object(Default::default()));
                message_parts.push(ContentPart::tool_call(ToolCallData {
                    id: call_id.to_string(),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    arguments,
                    r#type: "function".to_string(),
                }));
            }
            _ => {}
        }
    }

    let finish_reason = if has_tool_calls {
        FinishReason {
            reason: "tool_calls".to_string(),
            raw: Some("tool_calls".to_string()),
        }
    } else if raw_json
        .get("incomplete_details")
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str)
        == Some("max_output_tokens")
    {
        FinishReason {
            reason: "length".to_string(),
            raw: Some("max_output_tokens".to_string()),
        }
    } else {
        FinishReason {
            reason: "stop".to_string(),
            raw: raw_json
                .get("status")
                .and_then(Value::as_str)
                .map(ToString::to_string),
        }
    };

    let usage = parse_responses_usage(raw_json.get("usage"));

    Ok(Response {
        id,
        model,
        provider: provider.to_string(),
        message: Message {
            role: Role::Assistant,
            content: message_parts,
            name: None,
            tool_call_id: None,
        },
        finish_reason,
        usage,
        raw: Some(raw_json),
        warnings: Vec::new(),
        rate_limit: headers.and_then(parse_rate_limit_info),
    })
}

fn parse_chat_completions_response(raw_json: Value, provider: &str) -> Result<Response, SDKError> {
    let id = raw_json
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chatcmpl_unknown")
        .to_string();
    let model = raw_json
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let choice = raw_json
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let message = choice.get("message").cloned().unwrap_or_else(|| json!({}));

    let mut content = Vec::new();
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        content.push(ContentPart::text(text.to_string()));
    }
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            let arguments = tool_call
                .get("function")
                .and_then(|function| function.get("arguments"))
                .and_then(Value::as_str)
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .unwrap_or_else(|| Value::Object(Default::default()));
            content.push(ContentPart::tool_call(ToolCallData {
                id: tool_call
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("call_unknown")
                    .to_string(),
                name: tool_call
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                arguments,
                r#type: "function".to_string(),
            }));
        }
    }

    let finish_reason = map_finish_reason(choice.get("finish_reason").and_then(Value::as_str));

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
        usage: parse_chat_usage(raw_json.get("usage").unwrap_or(&Value::Null)),
        raw: Some(raw_json),
        warnings: vec![],
        rate_limit: None,
    })
}

fn parse_responses_usage(raw_usage: Option<&Value>) -> Usage {
    let usage = raw_usage.unwrap_or(&Value::Null);
    let reasoning_tokens = usage
        .get("output_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64);
    let cache_read_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
        });
    Usage {
        input_tokens: usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        output_tokens: usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        total_tokens: usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        reasoning_tokens,
        cache_read_tokens,
        cache_write_tokens: None,
        raw: if usage.is_null() {
            None
        } else {
            Some(usage.clone())
        },
    }
}

fn parse_chat_usage(raw_usage: &Value) -> Usage {
    Usage {
        input_tokens: raw_usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        output_tokens: raw_usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        total_tokens: raw_usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        reasoning_tokens: raw_usage
            .get("completion_tokens_details")
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(Value::as_u64),
        cache_read_tokens: raw_usage
            .get("prompt_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64),
        cache_write_tokens: None,
        raw: if raw_usage.is_null() {
            None
        } else {
            Some(raw_usage.clone())
        },
    }
}

fn map_finish_reason(raw: Option<&str>) -> FinishReason {
    let reason = match raw {
        Some("stop") => "stop",
        Some("length") => "length",
        Some("tool_calls") => "tool_calls",
        Some("content_filter") => "content_filter",
        Some(_) => "other",
        None => "stop",
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
                    .and_then(|error| error.get("code"))
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
    let requests_remaining = parse_u64_header(headers, "x-ratelimit-remaining-requests");
    let requests_limit = parse_u64_header(headers, "x-ratelimit-limit-requests");
    let tokens_remaining = parse_u64_header(headers, "x-ratelimit-remaining-tokens");
    let tokens_limit = parse_u64_header(headers, "x-ratelimit-limit-tokens");
    let reset_at = headers
        .get("x-ratelimit-reset-requests")
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

fn zero_usage() -> Usage {
    Usage {
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        reasoning_tokens: None,
        cache_read_tokens: None,
        cache_write_tokens: None,
        raw: None,
    }
}

struct OpenAIFactory;

impl ProviderFactory for OpenAIFactory {
    fn provider_id(&self) -> &'static str {
        "openai"
    }

    fn from_env(&self) -> Option<Arc<dyn ProviderAdapter>> {
        let config = OpenAIAdapterConfig::from_env()?;
        let adapter = OpenAIAdapter::new(config).ok()?;
        Some(Arc::new(adapter))
    }
}

static REGISTER_OPENAI_FACTORIES: Once = Once::new();

pub fn ensure_openai_factory_registered() {
    REGISTER_OPENAI_FACTORIES.call_once(|| {
        register_provider_factory(Arc::new(OpenAIFactory));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

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
            max_tokens: None,
            stop_sequences: None,
            reasoning_effort: None,
            metadata: None,
            provider_options: None,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn openai_adapter_complete_uses_responses_endpoint_and_maps_reasoning_tokens() {
        let body = json!({
            "id": "resp_1",
            "model": "gpt-5.2",
            "status": "completed",
            "output": [{
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "Hello" }]
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

        let response = adapter
            .complete(minimal_request("openai"))
            .await
            .expect("complete");
        assert_eq!(response.text(), "Hello");
        assert_eq!(response.usage.reasoning_tokens, Some(2));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn openai_adapter_stream_maps_delta_and_finish_events() {
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
        let base_url =
            spawn_single_response_server(200, "text/event-stream", sse_body, "/responses");
        let mut config = OpenAIAdapterConfig::new("test-key");
        config.base_url = base_url;
        let adapter = OpenAIAdapter::new(config).expect("adapter");

        let mut stream = adapter
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
                    event.response.as_ref().map(Response::text).as_deref(),
                    Some("Hello")
                );
                break;
            }
        }
        assert!(saw_delta);
        assert!(saw_finish);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn openai_compatible_adapter_complete_uses_chat_completions_endpoint() {
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
        let base_url =
            spawn_single_response_server(200, "application/json", body, "/chat/completions");
        let config = OpenAICompatibleAdapterConfig::new("test-key", base_url);
        let adapter = OpenAICompatibleAdapter::new(config).expect("adapter");

        let response = adapter
            .complete(minimal_request("openai-compatible"))
            .await
            .expect("complete");
        assert_eq!(response.text(), "hello from compatible");
        assert_eq!(response.usage.total_tokens, 7);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn openai_stream_emits_error_event_and_then_closes() {
        let sse_body = "event: response.created\ndata: {\"type\":\"response.created\"}\n\ndata: {not-json}\n\n"
            .to_string();
        let base_url =
            spawn_single_response_server(200, "text/event-stream", sse_body, "/responses");
        let mut config = OpenAIAdapterConfig::new("test-key");
        config.base_url = base_url;
        let adapter = OpenAIAdapter::new(config).expect("adapter");

        let mut stream = adapter
            .stream(minimal_request("openai"))
            .await
            .expect("stream");

        // First event can be STREAM_START; advance until we hit error behavior.
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

    #[test]
    fn build_responses_body_includes_json_schema_response_format() {
        let mut request = minimal_request("openai");
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

        let body = build_responses_body(&request, false).expect("body");
        assert_eq!(
            body.get("text")
                .and_then(|value| value.get("format"))
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str),
            Some("json_schema")
        );
    }

    #[test]
    fn build_provider_error_captures_retry_after_header() {
        let error = build_provider_error(
            "openai",
            429,
            "{\"error\":{\"message\":\"rate limited\"}}",
            Some(12.5),
        );

        match error {
            SDKError::Provider(provider) => {
                assert_eq!(provider.kind, ProviderErrorKind::RateLimit);
                assert_eq!(provider.retry_after, Some(12.5));
            }
            other => panic!("expected provider error, got {:?}", other),
        }
    }
}
