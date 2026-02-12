//! High-level API helpers (`generate`, `stream`, object generation, and tool loop).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc;
use futures::future::join_all;
use futures::stream;
use serde_json::{Value, json};

use crate::client::{Client, default_client};
use crate::errors::{
    AbortError, AbortSignal, NoObjectGeneratedError, RequestTimeoutError, RetryPolicy, SDKError,
    TimeoutConfig, compute_backoff_delay,
};
use crate::stream::{StreamEvent, StreamEventStream, StreamEventTypeOrString};
use crate::types::{
    Message, Request, Response, ResponseFormat, ToolCall, ToolChoice, ToolDefinition, Usage,
    Warning,
};
use crate::utils::{ResponseSeed, StreamAccumulator};
use crate::utils::{is_object_schema, require_object_schema};

type ToolFuture = Pin<Box<dyn Future<Output = Result<Value, SDKError>> + Send>>;
type ToolExecutor = Arc<dyn Fn(Value) -> ToolFuture + Send + Sync>;

pub type StopCondition = Arc<dyn Fn(&[StepResult]) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct Tool {
    pub definition: ToolDefinition,
    execute: Option<ToolExecutor>,
}

impl std::fmt::Debug for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tool")
            .field("definition", &self.definition)
            .field("active", &self.execute.is_some())
            .finish()
    }
}

impl Tool {
    pub fn passive(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
    ) -> Self {
        Self {
            definition: ToolDefinition {
                name: name.into(),
                description: description.into(),
                parameters,
            },
            execute: None,
        }
    }

    pub fn with_execute<F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        execute: F,
    ) -> Self
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, SDKError>> + Send + 'static,
    {
        let execute =
            Arc::new(move |arguments: Value| -> ToolFuture { Box::pin(execute(arguments)) });
        Self {
            definition: ToolDefinition {
                name: name.into(),
                description: description.into(),
                parameters,
            },
            execute: Some(execute),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: Value,
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StepResult {
    pub text: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub tool_results: Vec<ToolResult>,
    pub finish_reason: crate::types::FinishReason,
    pub usage: Usage,
    pub response: Response,
    pub warnings: Vec<Warning>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenerateResult {
    pub text: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub tool_results: Vec<ToolResult>,
    pub finish_reason: crate::types::FinishReason,
    pub usage: Usage,
    pub total_usage: Usage,
    pub steps: Vec<StepResult>,
    pub response: Response,
    pub output: Option<Value>,
}

#[derive(Clone)]
pub struct GenerateOptions {
    pub model: String,
    pub prompt: Option<String>,
    pub messages: Option<Vec<Message>>,
    pub system: Option<String>,
    pub tools: Vec<Tool>,
    pub tool_choice: Option<ToolChoice>,
    pub max_tool_rounds: usize,
    pub stop_when: Option<StopCondition>,
    pub response_format: Option<ResponseFormat>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<u64>,
    pub stop_sequences: Option<Vec<String>>,
    pub reasoning_effort: Option<String>,
    pub metadata: Option<HashMap<String, String>>,
    pub provider: Option<String>,
    pub provider_options: Option<Value>,
    pub retry_policy: RetryPolicy,
    pub timeout: Option<TimeoutConfig>,
    pub abort_signal: Option<AbortSignal>,
    pub client: Option<Arc<Client>>,
}

impl std::fmt::Debug for GenerateOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenerateOptions")
            .field("model", &self.model)
            .field("has_prompt", &self.prompt.is_some())
            .field(
                "messages_len",
                &self.messages.as_ref().map(|messages| messages.len()),
            )
            .field("system", &self.system)
            .field("tools_len", &self.tools.len())
            .field("tool_choice", &self.tool_choice)
            .field("max_tool_rounds", &self.max_tool_rounds)
            .field("response_format", &self.response_format)
            .field("temperature", &self.temperature)
            .field("top_p", &self.top_p)
            .field("max_tokens", &self.max_tokens)
            .field("stop_sequences", &self.stop_sequences)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("metadata", &self.metadata)
            .field("provider", &self.provider)
            .field("provider_options", &self.provider_options)
            .field("retry_policy", &self.retry_policy)
            .field("timeout", &self.timeout)
            .field("abort_signal", &self.abort_signal.is_some())
            .field("has_client_override", &self.client.is_some())
            .finish()
    }
}

impl GenerateOptions {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            prompt: None,
            messages: None,
            system: None,
            tools: Vec::new(),
            tool_choice: None,
            max_tool_rounds: 1,
            stop_when: None,
            response_format: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            stop_sequences: None,
            reasoning_effort: None,
            metadata: None,
            provider: None,
            provider_options: None,
            retry_policy: RetryPolicy::default(),
            timeout: None,
            abort_signal: None,
            client: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GenerateObjectOptions {
    pub generate: GenerateOptions,
    pub schema: Value,
    pub strict: bool,
}

impl GenerateObjectOptions {
    pub fn new(generate: GenerateOptions, schema: Value) -> Self {
        Self {
            generate,
            schema,
            strict: false,
        }
    }
}

pub struct StreamResult {
    pub events: StreamEventStream,
    state: Arc<Mutex<StreamState>>,
}

impl std::fmt::Debug for StreamResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.lock().expect("stream state");
        f.debug_struct("StreamResult")
            .field("has_response", &state.response.is_some())
            .field("has_partial_response", &state.partial_response.is_some())
            .finish()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct StreamState {
    response: Option<Response>,
    partial_response: Option<Response>,
}

impl StreamResult {
    pub fn response(&self) -> Option<Response> {
        self.state.lock().expect("stream state").response.clone()
    }

    pub fn partial_response(&self) -> Option<Response> {
        self.state
            .lock()
            .expect("stream state")
            .partial_response
            .clone()
    }

    pub fn into_events(self) -> StreamEventStream {
        self.events
    }

    pub fn text_stream(
        self,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<String, SDKError>> + Send>> {
        Box::pin(self.events.filter_map(|item| async move {
            match item {
                Ok(event) => event.delta.map(Ok),
                Err(error) => Some(Err(error)),
            }
        }))
    }
}

#[derive(Debug)]
pub struct StreamObjectResult {
    pub stream: StreamResult,
    pub partial_objects: Vec<Value>,
    pub object: Value,
}

pub async fn generate(options: GenerateOptions) -> Result<GenerateResult, SDKError> {
    let client = resolve_client(options.client.clone())?;
    validate_tools(&options.tools)?;
    let started = tokio::time::Instant::now();
    let mut conversation = standardize_messages(
        options.prompt.as_deref(),
        options.messages.clone(),
        options.system.as_deref(),
    )?;
    let tool_definitions = to_tool_definitions(&options.tools);
    let tool_map = to_tool_map(&options.tools);

    let mut steps = Vec::new();
    let mut total_usage = zero_usage();
    let mut executed_tool_rounds = 0usize;

    loop {
        let request = Request {
            model: options.model.clone(),
            messages: conversation.clone(),
            provider: options.provider.clone(),
            tools: tool_definitions.clone(),
            tool_choice: options.tool_choice.clone(),
            response_format: options.response_format.clone(),
            temperature: options.temperature,
            top_p: options.top_p,
            max_tokens: options.max_tokens,
            stop_sequences: options.stop_sequences.clone(),
            reasoning_effort: options.reasoning_effort.clone(),
            metadata: options.metadata.clone(),
            provider_options: options.provider_options.clone(),
        };

        let response = complete_with_retry(
            client.clone(),
            request,
            &options.retry_policy,
            options.timeout,
            started,
            options.abort_signal.as_ref(),
        )
        .await?;
        total_usage += response.usage.clone();
        let tool_calls = response.tool_calls();

        let mut tool_results = Vec::new();
        let should_execute_tools = !tool_calls.is_empty()
            && response.finish_reason.reason == "tool_calls"
            && options.max_tool_rounds > 0
            && executed_tool_rounds < options.max_tool_rounds;

        if should_execute_tools {
            tool_results =
                execute_tool_calls(&tool_map, &tool_calls, options.abort_signal.as_ref()).await;
            executed_tool_rounds += 1;

            conversation.push(response.message.clone());
            for result in &tool_results {
                conversation.push(Message::tool_result(
                    result.tool_call_id.clone(),
                    result.content.clone(),
                    result.is_error,
                ));
            }
        }

        steps.push(StepResult {
            text: response.text(),
            reasoning: response.reasoning(),
            tool_calls: tool_calls.clone(),
            tool_results: tool_results.clone(),
            finish_reason: response.finish_reason.clone(),
            usage: response.usage.clone(),
            warnings: response.warnings.clone(),
            response,
        });

        if let Some(stop_when) = &options.stop_when {
            if stop_when(&steps) {
                break;
            }
        }

        if !should_execute_tools {
            break;
        }
    }

    let final_step = steps.last().cloned().ok_or_else(|| {
        SDKError::Configuration(crate::errors::ConfigurationError::new(
            "generate produced no steps",
        ))
    })?;

    Ok(GenerateResult {
        text: final_step.text.clone(),
        reasoning: final_step.reasoning.clone(),
        tool_calls: final_step.tool_calls.clone(),
        tool_results: final_step.tool_results.clone(),
        finish_reason: final_step.finish_reason.clone(),
        usage: final_step.usage.clone(),
        total_usage,
        response: final_step.response.clone(),
        steps,
        output: None,
    })
}

pub async fn stream(options: GenerateOptions) -> Result<StreamResult, SDKError> {
    let client = resolve_client(options.client.clone())?;
    validate_tools(&options.tools)?;
    let started = tokio::time::Instant::now();
    let conversation = standardize_messages(
        options.prompt.as_deref(),
        options.messages.clone(),
        options.system.as_deref(),
    )?;
    let (tx, rx) = mpsc::unbounded::<Result<StreamEvent, SDKError>>();
    let state = Arc::new(Mutex::new(StreamState::default()));
    let state_for_task = state.clone();
    let options_for_task = options.clone();

    tokio::spawn(async move {
        let tx = tx;
        let mut conversation = conversation;
        let tool_definitions = to_tool_definitions(&options_for_task.tools);
        let tool_map = to_tool_map(&options_for_task.tools);
        let mut executed_tool_rounds = 0usize;

        loop {
            let request = Request {
                model: options_for_task.model.clone(),
                messages: conversation.clone(),
                provider: options_for_task.provider.clone(),
                tools: tool_definitions.clone(),
                tool_choice: options_for_task.tool_choice.clone(),
                response_format: options_for_task.response_format.clone(),
                temperature: options_for_task.temperature,
                top_p: options_for_task.top_p,
                max_tokens: options_for_task.max_tokens,
                stop_sequences: options_for_task.stop_sequences.clone(),
                reasoning_effort: options_for_task.reasoning_effort.clone(),
                metadata: options_for_task.metadata.clone(),
                provider_options: options_for_task.provider_options.clone(),
            };

            let mut provider_stream = match stream_with_retry(
                client.clone(),
                request,
                &options_for_task.retry_policy,
                options_for_task.timeout,
                started,
                options_for_task.abort_signal.as_ref(),
            )
            .await
            {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = tx.unbounded_send(Err(error));
                    return;
                }
            };

            let provider_name = options_for_task
                .provider
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let mut accumulator = StreamAccumulator::new(ResponseSeed {
                id: String::new(),
                model: options_for_task.model.clone(),
                provider: provider_name,
            });

            while let Some(item) = provider_stream.next().await {
                if let Some(signal) = options_for_task.abort_signal.as_ref() {
                    if signal.is_aborted() {
                        let _ = tx.unbounded_send(Err(SDKError::Abort(AbortError::new(
                            "operation aborted",
                        ))));
                        return;
                    }
                }
                match item {
                    Ok(event) => {
                        if event.event_type
                            == StreamEventTypeOrString::Known(crate::stream::StreamEventType::Error)
                        {
                            let error = event.error.clone().unwrap_or_else(|| {
                                SDKError::Stream(crate::errors::StreamError::new(
                                    "stream terminated with error event",
                                ))
                            });
                            let _ = tx.unbounded_send(Err(error));
                            return;
                        }
                        accumulator.process(&event);
                        let _ = tx.unbounded_send(Ok(event));
                    }
                    Err(error) => {
                        let _ = tx.unbounded_send(Err(error));
                        return;
                    }
                }
            }

            let response = accumulator.response();
            {
                let mut guard = state_for_task.lock().expect("stream state");
                guard.partial_response = Some(response.clone());
            }

            let tool_calls = response.tool_calls();
            let should_execute_tools = !tool_calls.is_empty()
                && response.finish_reason.reason == "tool_calls"
                && options_for_task.max_tool_rounds > 0
                && executed_tool_rounds < options_for_task.max_tool_rounds;

            if should_execute_tools {
                let tool_results = execute_tool_calls(
                    &tool_map,
                    &tool_calls,
                    options_for_task.abort_signal.as_ref(),
                )
                .await;
                executed_tool_rounds += 1;
                conversation.push(response.message.clone());
                for result in &tool_results {
                    conversation.push(Message::tool_result(
                        result.tool_call_id.clone(),
                        result.content.clone(),
                        result.is_error,
                    ));
                }
                let _ = tx.unbounded_send(Ok(StreamEvent {
                    event_type: StreamEventTypeOrString::Other("step_finish".to_string()),
                    delta: None,
                    text_id: None,
                    reasoning_delta: None,
                    tool_call: None,
                    finish_reason: None,
                    usage: None,
                    response: None,
                    error: None,
                    raw: None,
                }));
            } else {
                let mut guard = state_for_task.lock().expect("stream state");
                guard.response = Some(response);
                return;
            }
        }
    });

    Ok(StreamResult {
        events: Box::pin(rx),
        state,
    })
}

pub async fn generate_object(options: GenerateObjectOptions) -> Result<GenerateResult, SDKError> {
    require_object_schema(&options.schema).map_err(|error| {
        SDKError::NoObjectGenerated(NoObjectGeneratedError::new(error.to_string()))
    })?;

    let mut generate_options = options.generate.clone();
    generate_options.response_format = Some(ResponseFormat {
        r#type: "json_schema".to_string(),
        json_schema: Some(options.schema.clone()),
        strict: options.strict,
    });

    let mut result = generate(generate_options).await?;
    let parsed = parse_and_validate_object(&result.text, &options.schema)?;
    result.output = Some(parsed);
    Ok(result)
}

pub async fn stream_object(options: GenerateObjectOptions) -> Result<StreamObjectResult, SDKError> {
    require_object_schema(&options.schema).map_err(|error| {
        SDKError::NoObjectGenerated(NoObjectGeneratedError::new(error.to_string()))
    })?;

    let mut generate_options = options.generate.clone();
    generate_options.response_format = Some(ResponseFormat {
        r#type: "json_schema".to_string(),
        json_schema: Some(options.schema.clone()),
        strict: options.strict,
    });

    let streamed = stream(generate_options).await?;
    let mut provider_events = streamed.into_events();
    let mut replay_items: Vec<Result<StreamEvent, SDKError>> = Vec::new();
    let mut text_buffer = String::new();
    let mut partial_objects = Vec::new();
    let mut last_partial: Option<Value> = None;
    let mut final_response: Option<Response> = None;

    while let Some(item) = provider_events.next().await {
        match &item {
            Ok(event) => {
                if event.event_type
                    == StreamEventTypeOrString::Known(crate::stream::StreamEventType::TextDelta)
                {
                    if let Some(delta) = &event.delta {
                        text_buffer.push_str(delta);
                        for parsed in incremental_parse_objects(&text_buffer) {
                            if last_partial.as_ref() != Some(&parsed) {
                                last_partial = Some(parsed.clone());
                                partial_objects.push(parsed);
                            }
                        }
                    }
                }
                if event.event_type
                    == StreamEventTypeOrString::Known(crate::stream::StreamEventType::Finish)
                {
                    final_response = event.response.clone();
                }
            }
            Err(error) => return Err(error.clone()),
        }
        replay_items.push(item);
    }

    let response = if let Some(response) = final_response {
        response
    } else {
        return Err(SDKError::NoObjectGenerated(NoObjectGeneratedError::new(
            "stream ended without a final response",
        )));
    };

    let object = parse_and_validate_object(&response.text(), &options.schema)?;
    let state = Arc::new(Mutex::new(StreamState {
        response: Some(response.clone()),
        partial_response: Some(response),
    }));
    let stream = StreamResult {
        events: Box::pin(stream::iter(replay_items.into_iter())),
        state,
    };

    Ok(StreamObjectResult {
        stream,
        partial_objects,
        object,
    })
}

fn resolve_client(client: Option<Arc<Client>>) -> Result<Arc<Client>, SDKError> {
    match client {
        Some(client) => Ok(client),
        None => default_client(),
    }
}

fn standardize_messages(
    prompt: Option<&str>,
    messages: Option<Vec<Message>>,
    system: Option<&str>,
) -> Result<Vec<Message>, SDKError> {
    if prompt.is_some() && messages.is_some() {
        return Err(SDKError::Configuration(
            crate::errors::ConfigurationError::new("prompt and messages are mutually exclusive"),
        ));
    }
    if prompt.is_none() && messages.is_none() {
        return Err(SDKError::Configuration(
            crate::errors::ConfigurationError::new("either prompt or messages must be provided"),
        ));
    }

    let mut out = Vec::new();
    if let Some(system) = system {
        out.push(Message::system(system.to_string()));
    }

    if let Some(messages) = messages {
        out.extend(messages);
    } else if let Some(prompt) = prompt {
        out.push(Message::user(prompt.to_string()));
    }

    Ok(out)
}

fn to_tool_definitions(tools: &[Tool]) -> Option<Vec<ToolDefinition>> {
    if tools.is_empty() {
        None
    } else {
        Some(tools.iter().map(|tool| tool.definition.clone()).collect())
    }
}

fn validate_tools(tools: &[Tool]) -> Result<(), SDKError> {
    for tool in tools {
        let name = &tool.definition.name;
        if name.is_empty()
            || name.len() > 64
            || !name
                .chars()
                .next()
                .map(|first| first.is_ascii_alphabetic())
                .unwrap_or(false)
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(SDKError::Configuration(
                crate::errors::ConfigurationError::new(format!(
                    "invalid tool name '{}': expected [a-zA-Z][a-zA-Z0-9_]* with max length 64",
                    name
                )),
            ));
        }
        if !is_object_schema(&tool.definition.parameters) {
            return Err(SDKError::Configuration(
                crate::errors::ConfigurationError::new(format!(
                    "tool '{}' parameters schema root must be an object",
                    name
                )),
            ));
        }
    }
    Ok(())
}

fn to_tool_map(tools: &[Tool]) -> HashMap<String, Tool> {
    tools
        .iter()
        .map(|tool| (tool.definition.name.clone(), tool.clone()))
        .collect()
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

async fn execute_tool_calls(
    tool_map: &HashMap<String, Tool>,
    tool_calls: &[ToolCall],
    abort_signal: Option<&AbortSignal>,
) -> Vec<ToolResult> {
    let futures = tool_calls
        .iter()
        .map(|tool_call| execute_single_tool_call(tool_map, tool_call, abort_signal));
    join_all(futures).await
}

async fn execute_single_tool_call(
    tool_map: &HashMap<String, Tool>,
    tool_call: &ToolCall,
    abort_signal: Option<&AbortSignal>,
) -> ToolResult {
    if let Some(signal) = abort_signal {
        if signal.is_aborted() {
            return ToolResult {
                tool_call_id: tool_call.id.clone(),
                content: json!({ "error": "operation aborted" }),
                is_error: true,
            };
        }
    }

    let Some(tool) = tool_map.get(&tool_call.name) else {
        return ToolResult {
            tool_call_id: tool_call.id.clone(),
            content: json!({ "error": format!("unknown tool: {}", tool_call.name) }),
            is_error: true,
        };
    };

    let Some(execute) = tool.execute.clone() else {
        return ToolResult {
            tool_call_id: tool_call.id.clone(),
            content: json!({
                "error": format!("tool '{}' has no execute handler", tool_call.name)
            }),
            is_error: true,
        };
    };

    let arguments = if let Some(raw_arguments) = &tool_call.raw_arguments {
        match serde_json::from_str::<Value>(raw_arguments) {
            Ok(parsed) => parsed,
            Err(error) => {
                return ToolResult {
                    tool_call_id: tool_call.id.clone(),
                    content: json!({ "error": format!("invalid tool arguments: {}", error) }),
                    is_error: true,
                };
            }
        }
    } else {
        tool_call.arguments.clone()
    };

    if !arguments.is_object() {
        return ToolResult {
            tool_call_id: tool_call.id.clone(),
            content: json!({ "error": "tool arguments must be a JSON object" }),
            is_error: true,
        };
    }

    if let Some(error) = validate_tool_arguments(&tool.definition.parameters, &arguments) {
        return ToolResult {
            tool_call_id: tool_call.id.clone(),
            content: json!({ "error": error }),
            is_error: true,
        };
    }

    match execute(arguments).await {
        Ok(content) => ToolResult {
            tool_call_id: tool_call.id.clone(),
            content,
            is_error: false,
        },
        Err(error) => ToolResult {
            tool_call_id: tool_call.id.clone(),
            content: json!({ "error": error.message() }),
            is_error: true,
        },
    }
}

async fn complete_with_retry(
    client: Arc<Client>,
    request: Request,
    policy: &RetryPolicy,
    timeout: Option<TimeoutConfig>,
    started: tokio::time::Instant,
    abort_signal: Option<&AbortSignal>,
) -> Result<Response, SDKError> {
    let mut attempt = 0usize;

    loop {
        if let Some(signal) = abort_signal {
            if signal.is_aborted() {
                return Err(SDKError::Abort(AbortError::new("operation aborted")));
            }
        }

        let step_timeout = compute_effective_step_timeout(timeout, started)?;
        let complete_future = client.complete(request.clone());
        let attempt_result = if let Some(limit) = step_timeout {
            match tokio::time::timeout(Duration::from_secs_f64(limit), complete_future).await {
                Ok(result) => result,
                Err(_) => Err(SDKError::RequestTimeout(RequestTimeoutError::new(
                    "request timed out",
                ))),
            }
        } else {
            complete_future.await
        };

        match attempt_result {
            Ok(response) => return Ok(response),
            Err(error) => {
                if !error.retryable() || attempt >= policy.max_retries {
                    return Err(error);
                }

                let retry_after = match &error {
                    SDKError::Provider(provider_error) => provider_error.retry_after,
                    _ => None,
                };
                let Some(delay) = compute_backoff_delay(policy, attempt, retry_after) else {
                    return Err(error);
                };

                if let Some(on_retry) = &policy.on_retry {
                    on_retry(&error, attempt, delay);
                }

                tokio::time::sleep(Duration::from_secs_f64(delay)).await;
                attempt += 1;
            }
        }
    }
}

async fn stream_with_retry(
    client: Arc<Client>,
    request: Request,
    policy: &RetryPolicy,
    timeout: Option<TimeoutConfig>,
    started: tokio::time::Instant,
    abort_signal: Option<&AbortSignal>,
) -> Result<StreamEventStream, SDKError> {
    let mut attempt = 0usize;

    loop {
        if let Some(signal) = abort_signal {
            if signal.is_aborted() {
                return Err(SDKError::Abort(AbortError::new("operation aborted")));
            }
        }

        let step_timeout = compute_effective_step_timeout(timeout, started)?;
        let stream_future = client.stream(request.clone());
        let attempt_result = if let Some(limit) = step_timeout {
            match tokio::time::timeout(Duration::from_secs_f64(limit), stream_future).await {
                Ok(result) => result,
                Err(_) => Err(SDKError::RequestTimeout(RequestTimeoutError::new(
                    "stream connection timed out",
                ))),
            }
        } else {
            stream_future.await
        };

        match attempt_result {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                if !error.retryable() || attempt >= policy.max_retries {
                    return Err(error);
                }

                let retry_after = match &error {
                    SDKError::Provider(provider_error) => provider_error.retry_after,
                    _ => None,
                };
                let Some(delay) = compute_backoff_delay(policy, attempt, retry_after) else {
                    return Err(error);
                };
                if let Some(on_retry) = &policy.on_retry {
                    on_retry(&error, attempt, delay);
                }
                tokio::time::sleep(Duration::from_secs_f64(delay)).await;
                attempt += 1;
            }
        }
    }
}

fn compute_effective_step_timeout(
    timeout: Option<TimeoutConfig>,
    started: tokio::time::Instant,
) -> Result<Option<f64>, SDKError> {
    let Some(timeout) = timeout else {
        return Ok(None);
    };

    let per_step = timeout.per_step;
    let total_remaining = timeout.total.map(|total| {
        let elapsed = started.elapsed().as_secs_f64();
        total - elapsed
    });

    if let Some(remaining) = total_remaining {
        if remaining <= 0.0 {
            return Err(SDKError::RequestTimeout(RequestTimeoutError::new(
                "total timeout exceeded",
            )));
        }
    }

    let effective = match (per_step, total_remaining) {
        (Some(step), Some(remaining)) => Some(step.min(remaining)),
        (Some(step), None) => Some(step),
        (None, Some(remaining)) => Some(remaining),
        (None, None) => None,
    };
    Ok(effective.filter(|value| *value > 0.0))
}

fn validate_tool_arguments(schema: &Value, arguments: &Value) -> Option<String> {
    let object = arguments.as_object()?;
    if !is_object_schema(schema) {
        return Some("tool schema root must be an object".to_string());
    }

    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for key in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(key) {
                return Some(format!("missing required argument '{}'", key));
            }
        }
    }

    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    for (key, value) in object {
        if let Some(property) = properties.get(key) {
            if let Some(type_name) = property.get("type").and_then(Value::as_str) {
                let is_valid = match type_name {
                    "string" => value.is_string(),
                    "number" => value.is_number(),
                    "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
                    "boolean" => value.is_boolean(),
                    "array" => value.is_array(),
                    "object" => value.is_object(),
                    "null" => value.is_null(),
                    _ => true,
                };
                if !is_valid {
                    return Some(format!(
                        "argument '{}' expected type '{}' but received '{}'",
                        key,
                        type_name,
                        json_type_name(value)
                    ));
                }
            }
        }
    }

    None
}

fn json_type_name(value: &Value) -> &'static str {
    if value.is_null() {
        "null"
    } else if value.is_boolean() {
        "boolean"
    } else if value.is_string() {
        "string"
    } else if value.is_number() {
        "number"
    } else if value.is_array() {
        "array"
    } else {
        "object"
    }
}

fn parse_and_validate_object(text: &str, schema: &Value) -> Result<Value, SDKError> {
    let parsed = serde_json::from_str::<Value>(text).map_err(|error| {
        SDKError::NoObjectGenerated(NoObjectGeneratedError::new(format!(
            "failed to parse JSON object: {}",
            error
        )))
    })?;

    let object = parsed.as_object().ok_or_else(|| {
        SDKError::NoObjectGenerated(NoObjectGeneratedError::new(
            "parsed JSON output is not an object",
        ))
    })?;

    if let Some(required) = schema.get("required").and_then(|value| value.as_array()) {
        for key in required.iter().filter_map(|item| item.as_str()) {
            if !object.contains_key(key) {
                return Err(SDKError::NoObjectGenerated(NoObjectGeneratedError::new(
                    format!("missing required key '{}'", key),
                )));
            }
        }
    }

    Ok(parsed)
}

fn incremental_parse_objects(text: &str) -> Vec<Value> {
    let mut parsed = Vec::new();
    let mut last: Option<Value> = None;
    for boundary in text
        .char_indices()
        .map(|(index, _)| index)
        .skip(1)
        .chain(std::iter::once(text.len()))
    {
        if let Ok(value) = serde_json::from_str::<Value>(&text[..boundary]) {
            if last.as_ref() != Some(&value) {
                parsed.push(value.clone());
                last = Some(value);
            }
        }
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::time::{Duration, Instant};

    use crate::provider::ProviderAdapter;
    use crate::stream::StreamEventStream;
    use crate::types::{ContentPart, FinishReason, Message, Request, Role, ToolCallData};

    struct QueueAdapter {
        name: String,
        responses: Arc<Mutex<Vec<Response>>>,
        complete_calls: Arc<AtomicUsize>,
        delay_ms: u64,
    }

    impl QueueAdapter {
        fn new(name: &str, responses: Vec<Response>) -> Self {
            Self {
                name: name.to_string(),
                responses: Arc::new(Mutex::new(responses)),
                complete_calls: Arc::new(AtomicUsize::new(0)),
                delay_ms: 0,
            }
        }
    }

    struct StreamingAdapter {
        name: String,
        events: Arc<Mutex<Vec<Result<StreamEvent, SDKError>>>>,
    }

    #[async_trait]
    impl ProviderAdapter for QueueAdapter {
        fn name(&self) -> &str {
            &self.name
        }

        async fn complete(&self, _request: Request) -> Result<Response, SDKError> {
            self.complete_calls.fetch_add(1, Ordering::SeqCst);
            if self.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            }
            let mut responses = self.responses.lock().expect("responses");
            if responses.is_empty() {
                return Err(SDKError::Configuration(
                    crate::errors::ConfigurationError::new("no queued response"),
                ));
            }
            Ok(responses.remove(0))
        }

        async fn stream(&self, _request: Request) -> Result<StreamEventStream, SDKError> {
            Err(SDKError::Configuration(
                crate::errors::ConfigurationError::new("not used in tests"),
            ))
        }
    }

    #[async_trait]
    impl ProviderAdapter for StreamingAdapter {
        fn name(&self) -> &str {
            &self.name
        }

        async fn complete(&self, _request: Request) -> Result<Response, SDKError> {
            Err(SDKError::Configuration(
                crate::errors::ConfigurationError::new("not used in stream-object test"),
            ))
        }

        async fn stream(&self, _request: Request) -> Result<StreamEventStream, SDKError> {
            let items = self.events.lock().expect("events").clone();
            Ok(Box::pin(stream::iter(items.into_iter())))
        }
    }

    fn build_client(adapter: Arc<QueueAdapter>) -> Arc<Client> {
        let mut providers: HashMap<String, Arc<dyn ProviderAdapter>> = HashMap::new();
        providers.insert(adapter.name().to_string(), adapter);
        Arc::new(Client::new(providers, Some("test".to_string()), Vec::new()))
    }

    fn build_stream_client(adapter: Arc<StreamingAdapter>) -> Arc<Client> {
        let mut providers: HashMap<String, Arc<dyn ProviderAdapter>> = HashMap::new();
        providers.insert(adapter.name().to_string(), adapter);
        Arc::new(Client::new(providers, Some("test".to_string()), Vec::new()))
    }

    fn usage() -> Usage {
        Usage {
            input_tokens: 1,
            output_tokens: 1,
            total_tokens: 2,
            reasoning_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            raw: None,
        }
    }

    fn response_with_tool_calls(calls: Vec<ToolCallData>) -> Response {
        Response {
            id: "resp_tool".to_string(),
            model: "model".to_string(),
            provider: "test".to_string(),
            message: Message {
                role: Role::Assistant,
                content: calls.into_iter().map(ContentPart::tool_call).collect(),
                name: None,
                tool_call_id: None,
            },
            finish_reason: FinishReason {
                reason: "tool_calls".to_string(),
                raw: None,
            },
            usage: usage(),
            raw: None,
            warnings: vec![],
            rate_limit: None,
        }
    }

    fn response_with_text(text: &str) -> Response {
        Response {
            id: "resp_text".to_string(),
            model: "model".to_string(),
            provider: "test".to_string(),
            message: Message::assistant(text),
            finish_reason: FinishReason {
                reason: "stop".to_string(),
                raw: None,
            },
            usage: usage(),
            raw: None,
            warnings: vec![],
            rate_limit: None,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generate_rejects_prompt_and_messages_together() {
        let adapter = Arc::new(QueueAdapter::new("test", vec![response_with_text("ok")]));
        let mut options = GenerateOptions::new("model");
        options.prompt = Some("prompt".to_string());
        options.messages = Some(vec![Message::user("hello")]);
        options.client = Some(build_client(adapter));

        let error = generate(options)
            .await
            .expect_err("expected validation error");
        assert!(matches!(error, SDKError::Configuration(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn max_tool_rounds_zero_disables_tool_execution() {
        let first = response_with_tool_calls(vec![ToolCallData {
            id: "call_1".to_string(),
            name: "tool_a".to_string(),
            arguments: json!({"x": 1}),
            r#type: "function".to_string(),
        }]);
        let adapter = Arc::new(QueueAdapter::new("test", vec![first]));
        let complete_calls = adapter.complete_calls.clone();
        let mut options = GenerateOptions::new("model");
        options.prompt = Some("hello".to_string());
        options.max_tool_rounds = 0;
        options.tools = vec![Tool::with_execute(
            "tool_a",
            "tool",
            json!({"type": "object"}),
            |_args| async { Ok(json!({"ok": true})) },
        )];
        options.client = Some(build_client(adapter));

        let result = generate(options).await.expect("generate should succeed");
        assert_eq!(complete_calls.load(Ordering::SeqCst), 1);
        assert!(result.steps[0].tool_results.is_empty());
        assert_eq!(result.steps.len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn parallel_tool_calls_execute_concurrently_and_keep_call_order() {
        let first = response_with_tool_calls(vec![
            ToolCallData {
                id: "call_1".to_string(),
                name: "tool_a".to_string(),
                arguments: json!({"name":"a"}),
                r#type: "function".to_string(),
            },
            ToolCallData {
                id: "call_2".to_string(),
                name: "tool_b".to_string(),
                arguments: json!({"name":"b"}),
                r#type: "function".to_string(),
            },
            ToolCallData {
                id: "call_3".to_string(),
                name: "tool_c".to_string(),
                arguments: json!({"name":"c"}),
                r#type: "function".to_string(),
            },
        ]);
        let second = response_with_text("done");
        let adapter = Arc::new(QueueAdapter::new("test", vec![first, second]));

        let completion_order = Arc::new(Mutex::new(Vec::<String>::new()));
        let tool_a_order = completion_order.clone();
        let tool_b_order = completion_order.clone();
        let tool_c_order = completion_order.clone();

        let tools = vec![
            Tool::with_execute("tool_a", "tool", json!({"type":"object"}), move |_args| {
                let completion_order = tool_a_order.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    completion_order
                        .lock()
                        .expect("completion order")
                        .push("call_1".to_string());
                    Ok(json!({"result":"a"}))
                }
            }),
            Tool::with_execute("tool_b", "tool", json!({"type":"object"}), move |_args| {
                let completion_order = tool_b_order.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    completion_order
                        .lock()
                        .expect("completion order")
                        .push("call_2".to_string());
                    Ok(json!({"result":"b"}))
                }
            }),
            Tool::with_execute("tool_c", "tool", json!({"type":"object"}), move |_args| {
                let completion_order = tool_c_order.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(40)).await;
                    completion_order
                        .lock()
                        .expect("completion order")
                        .push("call_3".to_string());
                    Ok(json!({"result":"c"}))
                }
            }),
        ];

        let mut options = GenerateOptions::new("model");
        options.prompt = Some("hello".to_string());
        options.max_tool_rounds = 1;
        options.tools = tools;
        options.client = Some(build_client(adapter));

        let start = Instant::now();
        let result = generate(options).await.expect("generate should succeed");
        let elapsed = start.elapsed();
        let step = &result.steps[0];
        let completion_order = completion_order.lock().expect("completion order").clone();

        assert_eq!(
            step.tool_results
                .iter()
                .map(|result| result.tool_call_id.as_str())
                .collect::<Vec<_>>(),
            vec!["call_1", "call_2", "call_3"]
        );
        assert_eq!(completion_order, vec!["call_2", "call_3", "call_1"]);
        assert!(elapsed < Duration::from_millis(140));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generate_object_returns_no_object_generated_error_on_validation_failure() {
        let adapter = Arc::new(QueueAdapter::new(
            "test",
            vec![response_with_text("{\"name\":\"alice\"}")],
        ));
        let mut generate_options = GenerateOptions::new("model");
        generate_options.prompt = Some("hello".to_string());
        generate_options.client = Some(build_client(adapter));

        let options = GenerateObjectOptions {
            generate: generate_options,
            schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "age": { "type": "integer" }
                },
                "required": ["name", "age"]
            }),
            strict: false,
        };

        let error = generate_object(options)
            .await
            .expect_err("expected no-object-generated error");
        assert!(matches!(error, SDKError::NoObjectGenerated(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generate_rejects_invalid_tool_name() {
        let adapter = Arc::new(QueueAdapter::new("test", vec![response_with_text("ok")]));
        let mut options = GenerateOptions::new("model");
        options.prompt = Some("hello".to_string());
        options.client = Some(build_client(adapter));
        options.tools = vec![Tool::passive(
            "invalid-name",
            "bad name",
            json!({ "type": "object" }),
        )];

        let error = generate(options)
            .await
            .expect_err("expected invalid tool name error");
        assert!(matches!(error, SDKError::Configuration(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generate_honors_per_step_timeout() {
        let mut delayed = QueueAdapter::new("test", vec![response_with_text("ok")]);
        delayed.delay_ms = 30;
        let adapter = Arc::new(delayed);

        let mut options = GenerateOptions::new("model");
        options.prompt = Some("hello".to_string());
        options.client = Some(build_client(adapter));
        options.timeout = Some(TimeoutConfig {
            total: None,
            per_step: Some(0.01),
        });
        options.retry_policy.max_retries = 0;

        let error = generate(options).await.expect_err("expected timeout");
        assert!(matches!(error, SDKError::RequestTimeout(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stream_object_replays_real_stream_deltas_and_parses_output() {
        let response = Response {
            id: "resp_obj".to_string(),
            model: "model".to_string(),
            provider: "test".to_string(),
            message: Message::assistant("{\"name\":\"alice\"}"),
            finish_reason: FinishReason {
                reason: "stop".to_string(),
                raw: None,
            },
            usage: usage(),
            raw: None,
            warnings: vec![],
            rate_limit: None,
        };
        let events = vec![
            Ok(StreamEvent {
                event_type: StreamEventTypeOrString::Known(
                    crate::stream::StreamEventType::StreamStart,
                ),
                delta: None,
                text_id: None,
                reasoning_delta: None,
                tool_call: None,
                finish_reason: None,
                usage: None,
                response: None,
                error: None,
                raw: None,
            }),
            Ok(StreamEvent {
                event_type: StreamEventTypeOrString::Known(
                    crate::stream::StreamEventType::TextDelta,
                ),
                delta: Some("{\"name\":".to_string()),
                text_id: Some("text_0".to_string()),
                reasoning_delta: None,
                tool_call: None,
                finish_reason: None,
                usage: None,
                response: None,
                error: None,
                raw: None,
            }),
            Ok(StreamEvent {
                event_type: StreamEventTypeOrString::Known(
                    crate::stream::StreamEventType::TextDelta,
                ),
                delta: Some("\"alice\"}".to_string()),
                text_id: Some("text_0".to_string()),
                reasoning_delta: None,
                tool_call: None,
                finish_reason: None,
                usage: None,
                response: None,
                error: None,
                raw: None,
            }),
            Ok(StreamEvent {
                event_type: StreamEventTypeOrString::Known(crate::stream::StreamEventType::Finish),
                delta: None,
                text_id: None,
                reasoning_delta: None,
                tool_call: None,
                finish_reason: Some(FinishReason {
                    reason: "stop".to_string(),
                    raw: None,
                }),
                usage: Some(usage()),
                response: Some(response.clone()),
                error: None,
                raw: None,
            }),
        ];
        let adapter = Arc::new(StreamingAdapter {
            name: "test".to_string(),
            events: Arc::new(Mutex::new(events)),
        });

        let mut generate_options = GenerateOptions::new("model");
        generate_options.prompt = Some("hello".to_string());
        generate_options.client = Some(build_stream_client(adapter));
        let options = GenerateObjectOptions {
            generate: generate_options,
            schema: json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
            strict: true,
        };

        let result = stream_object(options).await.expect("stream_object");
        assert_eq!(
            result.object.get("name").and_then(Value::as_str),
            Some("alice")
        );
        assert!(!result.partial_objects.is_empty());

        let mut replay = result.stream.into_events();
        let mut text_deltas = 0usize;
        while let Some(item) = replay.next().await {
            let event = item.expect("event");
            if event.event_type
                == StreamEventTypeOrString::Known(crate::stream::StreamEventType::TextDelta)
            {
                text_deltas += 1;
            }
        }
        assert_eq!(text_deltas, 2);
    }
}
