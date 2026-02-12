use std::sync::Arc;

use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::{LlmGenerateParams, LlmGenerateReceipt, LlmToolChoice, TokenUsage};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use aos_llm::{
    AdapterTimeout, AnthropicAdapter, AnthropicAdapterConfig, ContentPart, Message, OpenAIAdapter,
    OpenAIAdapterConfig, OpenAICompatibleAdapter, OpenAICompatibleAdapterConfig, ProviderAdapter,
    Request, Role, SDKError, ToolCallData, ToolChoice, ToolDefinition, ToolResultData,
};
use aos_store::Store;
use async_trait::async_trait;
use serde_json::Value;

use super::traits::AsyncEffectAdapter;
use crate::config::{LlmAdapterConfig, LlmApiKind, ProviderConfig};

/// LLM adapter that resolves CAS refs and delegates provider execution to `aos-llm`.
pub struct LlmAdapter<S: Store> {
    store: Arc<S>,
    config: LlmAdapterConfig,
}

impl<S: Store> LlmAdapter<S> {
    pub fn new(store: Arc<S>, config: LlmAdapterConfig) -> Self {
        Self { store, config }
    }

    fn provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.config.providers.get(name)
    }

    fn failure_receipt(
        &self,
        intent: &EffectIntent,
        provider_id: &str,
        status: ReceiptStatus,
        message: impl Into<String>,
    ) -> EffectReceipt {
        let msg = message.into();
        let output_ref = self
            .store
            .put_blob(msg.as_bytes())
            .ok()
            .and_then(|h| HashRef::new(h.to_hex()).ok())
            .unwrap_or_else(zero_hashref);

        let receipt = LlmGenerateReceipt {
            output_ref: output_ref.clone(),
            raw_output_ref: None,
            token_usage: TokenUsage {
                prompt: 0,
                completion: 0,
            },
            cost_cents: None,
            provider_id: provider_id.to_string(),
        };

        EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: format!("host.llm.{provider_id}"),
            status,
            payload_cbor: serde_cbor::to_vec(&receipt).unwrap_or_default(),
            cost_cents: None,
            signature: vec![0; 64],
        }
    }

    async fn complete_with_provider(
        &self,
        provider: &ProviderConfig,
        api_key: String,
        request: Request,
    ) -> Result<aos_llm::Response, SDKError> {
        match provider.api_kind {
            LlmApiKind::Responses => {
                let mut cfg = OpenAIAdapterConfig::new(api_key);
                cfg.base_url = provider.base_url.clone();
                cfg.timeout = to_adapter_timeout(provider.timeout);
                let adapter = OpenAIAdapter::new(cfg)?;
                adapter.complete(request).await
            }
            LlmApiKind::ChatCompletions => {
                let mut cfg = OpenAICompatibleAdapterConfig::new(api_key, &provider.base_url);
                cfg.timeout = to_adapter_timeout(provider.timeout);
                let adapter = OpenAICompatibleAdapter::new(cfg)?;
                adapter.complete(request).await
            }
            LlmApiKind::AnthropicMessages => {
                let mut cfg = AnthropicAdapterConfig::new(api_key);
                cfg.base_url = provider.base_url.clone();
                cfg.timeout = to_adapter_timeout(provider.timeout);
                let adapter = AnthropicAdapter::new(cfg)?;
                adapter.complete(request).await
            }
        }
    }

    fn load_messages(&self, refs: &[HashRef]) -> Result<Vec<Message>, String> {
        let mut messages = Vec::new();
        for reference in refs {
            let mut loaded = self.load_message(reference)?;
            messages.append(&mut loaded);
        }
        Ok(messages)
    }

    fn load_message(&self, reference: &HashRef) -> Result<Vec<Message>, String> {
        let hash = Hash::from_hex_str(reference.as_str())
            .map_err(|e| format!("invalid message_ref: {e}"))?;
        let bytes = self
            .store
            .get_blob(hash)
            .map_err(|e| format!("message_ref not found: {e}"))?;

        if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
            return parse_message_value(value);
        }

        let text = String::from_utf8(bytes)
            .map_err(|e| format!("message blob is not utf8 or JSON: {e}"))?;
        Ok(vec![Message::user(text)])
    }

    fn load_tools_blobs(
        &self,
        references: &[HashRef],
    ) -> Result<(Vec<ToolDefinition>, Option<ToolChoice>), String> {
        let mut tools: Vec<ToolDefinition> = Vec::new();
        let mut tool_choice: Option<ToolChoice> = None;

        for reference in references {
            let hash = Hash::from_hex_str(reference.as_str())
                .map_err(|e| format!("invalid tool_ref: {e}"))?;
            let bytes = self
                .store
                .get_blob(hash)
                .map_err(|e| format!("tool_ref not found: {e}"))?;
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|e| format!("tool_ref invalid JSON: {e}"))?;

            match value {
                Value::Array(items) => {
                    for item in items {
                        tools.push(parse_tool_definition(item)?);
                    }
                }
                Value::Object(map) => {
                    if let Some(items) = map.get("tools").and_then(Value::as_array) {
                        for item in items {
                            tools.push(parse_tool_definition(item.clone())?);
                        }
                    } else if map.contains_key("name")
                        || map.contains_key("function")
                        || map.contains_key("parameters")
                        || map.contains_key("input_schema")
                    {
                        tools.push(parse_tool_definition(Value::Object(map.clone()))?);
                    }

                    if let Some(choice) = map.get("tool_choice") {
                        tool_choice = Some(parse_tool_choice_json(choice.clone())?);
                    }
                }
                _ => return Err("tool_ref must be JSON array or object".to_string()),
            }
        }

        Ok((tools, tool_choice))
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for LlmAdapter<S> {
    fn kind(&self) -> &str {
        aos_effects::EffectKind::LLM_GENERATE
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: LlmGenerateParams = serde_cbor::from_slice(&intent.params_cbor)
            .map_err(|e| anyhow::anyhow!("decode LlmGenerateParams: {e}"))?;

        let provider_id = if params.provider.is_empty() {
            self.config.default_provider.clone()
        } else {
            params.provider.clone()
        };

        let provider = match self.provider(&provider_id) {
            Some(provider) => provider,
            None => {
                return Ok(self.failure_receipt(
                    intent,
                    &provider_id,
                    ReceiptStatus::Error,
                    format!("unknown provider {provider_id}"),
                ));
            }
        };

        let api_key = match params.api_key.clone() {
            Some(key) if !key.is_empty() => key,
            _ => {
                return Ok(self.failure_receipt(
                    intent,
                    &provider_id,
                    ReceiptStatus::Error,
                    "api_key missing",
                ));
            }
        };

        if params.message_refs.is_empty() {
            return Ok(self.failure_receipt(
                intent,
                &provider_id,
                ReceiptStatus::Error,
                "message_refs empty",
            ));
        }

        let messages = match self.load_messages(&params.message_refs) {
            Ok(messages) => messages,
            Err(err) => {
                return Ok(self.failure_receipt(intent, &provider_id, ReceiptStatus::Error, err));
            }
        };

        let (tools, tool_choice_from_blob) = if let Some(tool_refs) = params.tool_refs.as_ref() {
            match self.load_tools_blobs(tool_refs) {
                Ok((tools, choice)) => (Some(tools), choice),
                Err(err) => {
                    return Ok(self.failure_receipt(
                        intent,
                        &provider_id,
                        ReceiptStatus::Error,
                        err,
                    ));
                }
            }
        } else {
            (None, None)
        };

        let request = Request {
            model: params.model.clone(),
            messages,
            provider: Some(provider_id.clone()),
            tools,
            tool_choice: params
                .tool_choice
                .as_ref()
                .map(tool_choice_from_params)
                .or(tool_choice_from_blob),
            response_format: None,
            temperature: params.temperature.parse::<f64>().ok(),
            top_p: None,
            max_tokens: params.max_tokens,
            stop_sequences: None,
            reasoning_effort: None,
            metadata: None,
            provider_options: None,
        };

        let response = match self
            .complete_with_provider(provider, api_key, request)
            .await
        {
            Ok(response) => response,
            Err(err) => {
                let status = map_error_status(&err);
                return Ok(self.failure_receipt(
                    intent,
                    &provider_id,
                    status,
                    format!("provider request failed: {err}"),
                ));
            }
        };

        let normalized_output = normalized_output(provider.api_kind, &response);
        let output_bytes = match serde_json::to_vec(&normalized_output) {
            Ok(bytes) => bytes,
            Err(err) => {
                return Ok(self.failure_receipt(
                    intent,
                    &provider_id,
                    ReceiptStatus::Error,
                    format!("encode normalized output failed: {err}"),
                ));
            }
        };

        let output_ref = match self.store.put_blob(&output_bytes) {
            Ok(hash) => match HashRef::new(hash.to_hex()) {
                Ok(hash_ref) => hash_ref,
                Err(err) => {
                    return Ok(self.failure_receipt(
                        intent,
                        &provider_id,
                        ReceiptStatus::Error,
                        format!("invalid normalized output hash: {err}"),
                    ));
                }
            },
            Err(err) => {
                return Ok(self.failure_receipt(
                    intent,
                    &provider_id,
                    ReceiptStatus::Error,
                    format!("store normalized output failed: {err}"),
                ));
            }
        };

        let raw_output_ref = response
            .raw
            .as_ref()
            .and_then(|raw| serde_json::to_vec(raw).ok())
            .and_then(|bytes| self.store.put_blob(&bytes).ok())
            .and_then(|hash| HashRef::new(hash.to_hex()).ok());

        let receipt = LlmGenerateReceipt {
            output_ref: output_ref.clone(),
            raw_output_ref,
            token_usage: TokenUsage {
                prompt: response.usage.input_tokens,
                completion: response.usage.output_tokens,
            },
            cost_cents: None,
            provider_id: provider_id.clone(),
        };

        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: format!("host.llm.{provider_id}"),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt)?,
            cost_cents: None,
            signature: vec![0; 64],
        })
    }
}

fn to_adapter_timeout(timeout: std::time::Duration) -> AdapterTimeout {
    let request = timeout.as_secs_f64();
    if request <= 0.0 {
        return AdapterTimeout::default();
    }
    AdapterTimeout {
        connect: request.min(10.0),
        request,
        stream_read: request.min(30.0),
    }
}

fn map_error_status(error: &SDKError) -> ReceiptStatus {
    match error {
        SDKError::RequestTimeout(_) => ReceiptStatus::Timeout,
        SDKError::Network(net) if net.info.message.to_lowercase().contains("timed out") => {
            ReceiptStatus::Timeout
        }
        _ => ReceiptStatus::Error,
    }
}

fn tool_choice_from_params(choice: &LlmToolChoice) -> ToolChoice {
    match choice {
        LlmToolChoice::Auto => ToolChoice {
            mode: "auto".to_string(),
            tool_name: None,
        },
        LlmToolChoice::NoneChoice => ToolChoice {
            mode: "none".to_string(),
            tool_name: None,
        },
        LlmToolChoice::Required => ToolChoice {
            mode: "required".to_string(),
            tool_name: None,
        },
        LlmToolChoice::Tool { name } => ToolChoice {
            mode: "named".to_string(),
            tool_name: Some(name.clone()),
        },
    }
}

fn parse_tool_choice_json(value: Value) -> Result<ToolChoice, String> {
    match value {
        Value::String(mode) => Ok(ToolChoice {
            mode,
            tool_name: None,
        }),
        Value::Object(map) => {
            if let Some(mode) = map.get("mode").and_then(Value::as_str) {
                let tool_name = map
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string());
                return Ok(ToolChoice {
                    mode: mode.to_string(),
                    tool_name,
                });
            }

            if let Some(kind) = map.get("type").and_then(Value::as_str) {
                if kind.eq_ignore_ascii_case("function") {
                    let tool_name = map
                        .get("name")
                        .and_then(Value::as_str)
                        .or_else(|| {
                            map.get("function")
                                .and_then(Value::as_object)
                                .and_then(|f| f.get("name"))
                                .and_then(Value::as_str)
                        })
                        .map(|name| name.to_string());
                    return Ok(ToolChoice {
                        mode: if tool_name.is_some() {
                            "named".to_string()
                        } else {
                            "required".to_string()
                        },
                        tool_name,
                    });
                }
            }

            if let Some(name) = map.get("name").and_then(Value::as_str) {
                return Ok(ToolChoice {
                    mode: "named".to_string(),
                    tool_name: Some(name.to_string()),
                });
            }

            Err("tool_choice object must contain mode or function name".to_string())
        }
        _ => Err("tool_choice must be string or object".to_string()),
    }
}

fn parse_tool_definition(item: Value) -> Result<ToolDefinition, String> {
    let obj = item
        .as_object()
        .ok_or_else(|| "tool definition must be a JSON object".to_string())?;

    if let Some(function) = obj.get("function").and_then(Value::as_object) {
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "tool function missing name".to_string())?
            .to_string();
        let description = function
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let parameters = function
            .get("parameters")
            .cloned()
            .unwrap_or_else(empty_object);
        return Ok(ToolDefinition {
            name,
            description,
            parameters,
        });
    }

    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "tool definition missing name".to_string())?
        .to_string();
    let description = obj
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let parameters = obj
        .get("parameters")
        .or_else(|| obj.get("input_schema"))
        .cloned()
        .unwrap_or_else(empty_object);

    Ok(ToolDefinition {
        name,
        description,
        parameters,
    })
}

fn parse_message_value(value: Value) -> Result<Vec<Message>, String> {
    match value {
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.extend(parse_message_value(item)?);
            }
            Ok(out)
        }
        Value::Object(obj) => {
            if let Some(item_type) = obj.get("type").and_then(Value::as_str) {
                match item_type {
                    "function_call_output" => {
                        let call_id = obj
                            .get("call_id")
                            .or_else(|| obj.get("tool_call_id"))
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                "function_call_output missing call_id/tool_call_id".to_string()
                            })?
                            .to_string();
                        let output = obj.get("output").cloned().unwrap_or(Value::Null);
                        return Ok(vec![Message::tool_result(call_id, output, false)]);
                    }
                    "function_call" | "tool_call" => {
                        let call = parse_tool_call_object(&Value::Object(obj.clone()))?;
                        return Ok(vec![Message {
                            role: Role::Assistant,
                            content: vec![ContentPart::tool_call(call)],
                            name: None,
                            tool_call_id: None,
                        }]);
                    }
                    _ => {}
                }
            }

            if let Some(output) = obj.get("output").cloned() {
                return parse_message_value(output);
            }

            let role = obj
                .get("role")
                .and_then(Value::as_str)
                .map(parse_role)
                .unwrap_or(Role::User);

            let mut content = Vec::new();
            if let Some(raw_content) = obj.get("content") {
                match raw_content {
                    Value::String(text) => content.push(ContentPart::text(text.to_string())),
                    Value::Array(items) => {
                        for item in items {
                            if let Some(part) = parse_content_part(item)? {
                                content.push(part);
                            }
                        }
                    }
                    Value::Object(_) => {
                        if let Some(part) = parse_content_part(raw_content)? {
                            content.push(part);
                        }
                    }
                    _ => {}
                }
            }

            if let Some(tool_calls) = obj.get("tool_calls").and_then(Value::as_array) {
                for tool_call in tool_calls {
                    let parsed = parse_tool_call_object(tool_call)?;
                    content.push(ContentPart::tool_call(parsed));
                }
            }

            Ok(vec![Message {
                role,
                content,
                name: obj
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string()),
                tool_call_id: obj
                    .get("tool_call_id")
                    .or_else(|| obj.get("call_id"))
                    .and_then(Value::as_str)
                    .map(|s| s.to_string()),
            }])
        }
        _ => Err("message blob must be a JSON object or array".to_string()),
    }
}

fn parse_content_part(value: &Value) -> Result<Option<ContentPart>, String> {
    match value {
        Value::String(text) => Ok(Some(ContentPart::text(text.to_string()))),
        Value::Object(obj) => {
            let part_type = obj
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("text")
                .to_lowercase();
            match part_type.as_str() {
                "text" | "input_text" | "output_text" => {
                    if let Some(text) = obj.get("text").and_then(Value::as_str) {
                        return Ok(Some(ContentPart::text(text.to_string())));
                    }
                    Ok(None)
                }
                "function_call" | "tool_call" => {
                    let call = parse_tool_call_object(value)?;
                    Ok(Some(ContentPart::tool_call(call)))
                }
                "function_call_output" | "tool_result" => {
                    let tool_call_id = obj
                        .get("call_id")
                        .or_else(|| obj.get("tool_call_id"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| "tool_result part missing call_id/tool_call_id".to_string())?
                        .to_string();
                    let content = obj.get("output").cloned().unwrap_or(Value::Null);
                    let is_error = obj
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    Ok(Some(ContentPart::tool_result(ToolResultData {
                        tool_call_id,
                        content,
                        is_error,
                        image_data: None,
                        image_media_type: None,
                    })))
                }
                _ => {
                    if let Some(text) = obj.get("text").and_then(Value::as_str) {
                        Ok(Some(ContentPart::text(text.to_string())))
                    } else {
                        Ok(None)
                    }
                }
            }
        }
        _ => Ok(None),
    }
}

fn parse_tool_call_object(value: &Value) -> Result<ToolCallData, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "tool call must be a JSON object".to_string())?;

    if let Some(function) = obj.get("function").and_then(Value::as_object) {
        let id = obj
            .get("id")
            .or_else(|| obj.get("call_id"))
            .and_then(Value::as_str)
            .unwrap_or("call_unknown")
            .to_string();
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "tool call function missing name".to_string())?
            .to_string();
        let arguments = function
            .get("arguments")
            .map(parse_arguments)
            .unwrap_or_else(empty_object);
        return Ok(ToolCallData {
            id,
            name,
            arguments,
            r#type: "function".to_string(),
        });
    }

    let id = obj
        .get("id")
        .or_else(|| obj.get("call_id"))
        .and_then(Value::as_str)
        .unwrap_or("call_unknown")
        .to_string();
    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "tool call missing name".to_string())?
        .to_string();
    let arguments = obj
        .get("arguments")
        .or_else(|| obj.get("arguments_json"))
        .map(parse_arguments)
        .unwrap_or_else(empty_object);

    Ok(ToolCallData {
        id,
        name,
        arguments,
        r#type: "function".to_string(),
    })
}

fn parse_arguments(value: &Value) -> Value {
    match value {
        Value::String(raw) => {
            serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.clone()))
        }
        other => other.clone(),
    }
}

fn parse_role(role: &str) -> Role {
    match role.to_ascii_lowercase().as_str() {
        "system" => Role::System,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        "developer" => Role::Developer,
        _ => Role::User,
    }
}

fn normalized_output(api_kind: LlmApiKind, response: &aos_llm::Response) -> Value {
    if matches!(api_kind, LlmApiKind::Responses) {
        if let Some(raw_output) = response
            .raw
            .as_ref()
            .and_then(|raw| raw.get("output"))
            .cloned()
        {
            return raw_output;
        }
    }

    let mut message = serde_json::Map::new();
    message.insert(
        "role".into(),
        Value::String(role_to_string(&response.message.role)),
    );

    let mut content_parts: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    for part in &response.message.content {
        if let Some(text) = part.text.as_ref() {
            content_parts.push(serde_json::json!({
                "type": "text",
                "text": text,
            }));
        }
        if let Some(call) = part.tool_call.as_ref() {
            let args_text = call
                .arguments
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_string())
                });
            tool_calls.push(serde_json::json!({
                "id": call.id,
                "type": "function",
                "function": {
                    "name": call.name,
                    "arguments": args_text,
                }
            }));
        }
    }

    message.insert("content".into(), Value::Array(content_parts));
    if !tool_calls.is_empty() {
        message.insert("tool_calls".into(), Value::Array(tool_calls));
    }

    Value::Object(message)
}

fn role_to_string(role: &Role) -> String {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
        Role::Developer => "developer",
    }
    .to_string()
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

fn zero_hashref() -> HashRef {
    HashRef::new(format!("sha256:{}", "0".repeat(64))).expect("zero hash is valid")
}
