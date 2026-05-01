use std::sync::Arc;

use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::{
    LlmCompactParams, LlmCompactReceipt, LlmCompactionArtifactKind, LlmCountTokensParams,
    LlmCountTokensReceipt, LlmFinishReason, LlmGenerateParams, LlmGenerateReceipt,
    LlmOutputEnvelope, LlmProviderCompatibility, LlmTokenCountByRef, LlmTokenCountQuality,
    LlmToolCall, LlmToolCallList, LlmToolChoice, LlmUsageDetails, LlmWindowItem, LlmWindowItemKind,
    TokenUsage,
};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use aos_kernel::Store;
use aos_llm::{
    AdapterTimeout, AnthropicAdapter, AnthropicAdapterConfig, CompactionItemKind,
    CompactionRequest, ContentPart, Message, OpenAIAdapter, OpenAIAdapterConfig,
    OpenAICompatibleAdapter, OpenAICompatibleAdapterConfig, ProviderAdapter, Request,
    ResponseFormat, Role, SDKError, TokenCountRequest, ToolCallData, ToolChoice, ToolDefinition,
    ToolResultData,
};
use async_trait::async_trait;
use serde_json::Value;

use super::traits::AsyncEffectAdapter;
use crate::config::{LlmAdapterConfig, LlmApiKind, ProviderConfig};

/// LLM adapter that resolves CAS refs and delegates provider execution to `aos-llm`.
pub struct LlmAdapter<S: Store> {
    store: Arc<S>,
    config: LlmAdapterConfig,
}

pub struct LlmCompactAdapter<S: Store> {
    store: Arc<S>,
    config: LlmAdapterConfig,
}

pub struct LlmCountTokensAdapter<S: Store> {
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

    fn resolve_api_key(
        api_key: Option<&aos_effects::builtins::TextOrSecretRef>,
    ) -> Result<String, String> {
        let Some(api_key) = api_key else {
            return Err(
                "api_key missing (use secret ref in params and kernel secret injection)".into(),
            );
        };
        match api_key {
            aos_effects::builtins::TextOrSecretRef::Literal(value) if !value.is_empty() => {
                Ok(value.clone())
            }
            aos_effects::builtins::TextOrSecretRef::Literal(_) => {
                Err("api_key literal was empty".into())
            }
            aos_effects::builtins::TextOrSecretRef::Secret(secret) => Err(format!(
                "api_key secret ref unresolved: {}@{}",
                secret.alias, secret.version
            )),
        }
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
            .store_text_blob(&msg)
            .unwrap_or_else(|_| zero_hashref());

        let receipt = LlmGenerateReceipt {
            output_ref,
            raw_output_ref: None,
            provider_response_id: None,
            provider_context_items: Vec::new(),
            finish_reason: LlmFinishReason {
                reason: "error".to_string(),
                raw: Some(msg.clone()),
            },
            token_usage: TokenUsage {
                prompt: 0,
                completion: 0,
                total: Some(0),
            },
            usage_details: None,
            warnings_ref: None,
            rate_limit_ref: None,
            cost_cents: None,
            provider_id: provider_id.to_string(),
        };

        EffectReceipt {
            intent_hash: intent.intent_hash,
            status,
            payload_cbor: serde_cbor::to_vec(&receipt)
                .expect("encode host.llm failure receipt payload"),
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

    fn load_window_item_messages(
        &self,
        items: &[LlmWindowItem],
        provider: &str,
        model: &str,
    ) -> Result<Vec<Message>, String> {
        let mut messages = Vec::new();
        for item in items {
            let Some(reference) = item.renderable_message_ref(provider, model) else {
                return Err(format!(
                    "window item '{}' is not renderable for provider '{}' model '{}'",
                    item.item_id, provider, model
                ));
            };
            match item.kind {
                LlmWindowItemKind::ProviderNativeArtifactRef
                | LlmWindowItemKind::ProviderRawWindowRef => {
                    let raw = self.load_json_blob(reference, "provider_window_item_ref")?;
                    let kind = raw
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("provider_item")
                        .to_string();
                    messages.push(Message {
                        role: Role::Assistant,
                        content: vec![ContentPart::provider_item(kind, raw)],
                        name: None,
                        tool_call_id: None,
                    });
                }
                _ => {
                    let mut loaded = self.load_message(reference)?;
                    messages.append(&mut loaded);
                }
            }
        }
        if messages.is_empty() {
            return Err("window_items empty".into());
        }
        Ok(messages)
    }

    fn render_window_item_refs(
        items: &[LlmWindowItem],
        provider: &str,
        model: &str,
    ) -> Result<Vec<HashRef>, String> {
        let mut refs = Vec::with_capacity(items.len());
        for item in items {
            let Some(ref_) = item.renderable_message_ref(provider, model) else {
                return Err(format!(
                    "window item '{}' is not renderable for provider '{}' model '{}'",
                    item.item_id, provider, model
                ));
            };
            refs.push(ref_.clone());
        }
        if refs.is_empty() {
            return Err("window_items empty".into());
        }
        Ok(refs)
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

    fn load_json_blob(&self, reference: &HashRef, field: &str) -> Result<Value, String> {
        let hash =
            Hash::from_hex_str(reference.as_str()).map_err(|e| format!("invalid {field}: {e}"))?;
        let bytes = self
            .store
            .get_blob(hash)
            .map_err(|e| format!("{field} not found: {e}"))?;
        serde_json::from_slice::<Value>(&bytes).map_err(|e| format!("{field} invalid JSON: {e}"))
    }

    fn load_response_format(&self, reference: &HashRef) -> Result<ResponseFormat, String> {
        let value = self.load_json_blob(reference, "response_format_ref")?;
        serde_json::from_value::<ResponseFormat>(value)
            .map_err(|e| format!("response_format_ref invalid shape: {e}"))
    }

    fn store_json_blob(&self, value: &Value) -> Result<HashRef, String> {
        let bytes = serde_json::to_vec(value).map_err(|e| format!("encode JSON failed: {e}"))?;
        self.store_bytes_blob(&bytes)
    }

    fn store_text_blob(&self, value: &str) -> Result<HashRef, String> {
        self.store_bytes_blob(value.as_bytes())
    }

    fn store_bytes_blob(&self, bytes: &[u8]) -> Result<HashRef, String> {
        let hash = self
            .store
            .put_blob(bytes)
            .map_err(|e| format!("store blob failed: {e}"))?;
        HashRef::new(hash.to_hex()).map_err(|e| format!("invalid blob hash: {e}"))
    }

    fn store_provider_context_items(
        &self,
        response: &aos_llm::Response,
        source_window_items: &[LlmWindowItem],
        provider_id: &str,
        model: &str,
        api_kind: LlmApiKind,
    ) -> Result<Vec<LlmWindowItem>, String> {
        let source_refs = source_window_items
            .iter()
            .map(|item| item.ref_.clone())
            .collect::<Vec<_>>();
        let api_kind = llm_api_kind_name(api_kind).to_string();
        let mut out = Vec::new();
        for (idx, part) in response.message.content.iter().enumerate() {
            let Some(raw) = part.provider_item.as_ref() else {
                continue;
            };
            let raw_ref = self.store_json_blob(raw)?;
            let artifact_type = raw
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("provider_item")
                .to_string();
            let encrypted = raw
                .get("encrypted_content")
                .and_then(Value::as_str)
                .is_some();
            out.push(LlmWindowItem {
                item_id: format!("generate:{}:provider-item:{idx}", response.id),
                kind: if artifact_type.contains("compaction") {
                    LlmWindowItemKind::ProviderNativeArtifactRef
                } else {
                    LlmWindowItemKind::ProviderRawWindowRef
                },
                ref_: raw_ref,
                lane: Some("Summary".into()),
                source_range: None,
                source_refs: source_refs.clone(),
                provider_compatibility: Some(LlmProviderCompatibility {
                    provider: provider_id.to_string(),
                    api_kind: api_kind.clone(),
                    model: Some(model.to_string()),
                    model_family: None,
                    artifact_type,
                    opaque: encrypted,
                    encrypted,
                }),
                estimated_tokens: None,
                metadata: Default::default(),
            });
        }
        Ok(out)
    }
}

impl<S: Store> LlmCompactAdapter<S> {
    pub fn new(store: Arc<S>, config: LlmAdapterConfig) -> Self {
        Self { store, config }
    }

    fn provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.config.providers.get(name)
    }

    fn load_window_item_messages(
        &self,
        items: &[LlmWindowItem],
        provider: &str,
        model: &str,
    ) -> Result<Vec<Message>, String> {
        let adapter = LlmAdapter {
            store: self.store.clone(),
            config: self.config.clone(),
        };
        adapter.load_window_item_messages(items, provider, model)
    }

    fn load_json_blob(&self, reference: &HashRef, field: &str) -> Result<Value, String> {
        let adapter = LlmAdapter {
            store: self.store.clone(),
            config: self.config.clone(),
        };
        adapter.load_json_blob(reference, field)
    }

    fn store_json_blob(&self, value: &Value) -> Result<HashRef, String> {
        let adapter = LlmAdapter {
            store: self.store.clone(),
            config: self.config.clone(),
        };
        adapter.store_json_blob(value)
    }

    async fn compact_with_provider(
        &self,
        provider: &ProviderConfig,
        api_key: String,
        request: CompactionRequest,
    ) -> Result<aos_llm::CompactionResponse, SDKError> {
        match provider.api_kind {
            LlmApiKind::Responses => {
                let mut cfg = OpenAIAdapterConfig::new(api_key);
                cfg.base_url = provider.base_url.clone();
                cfg.timeout = to_adapter_timeout(provider.timeout);
                let adapter = OpenAIAdapter::new(cfg)?;
                adapter.compact(request).await
            }
            LlmApiKind::AnthropicMessages => {
                let mut cfg = AnthropicAdapterConfig::new(api_key);
                cfg.base_url = provider.base_url.clone();
                cfg.timeout = to_adapter_timeout(provider.timeout);
                let adapter = AnthropicAdapter::new(cfg)?;
                adapter.compact(request).await
            }
            LlmApiKind::ChatCompletions => {
                Err(SDKError::Configuration(aos_llm::ConfigurationError::new(
                    "OpenAI-compatible chat completions does not support explicit compaction",
                )))
            }
        }
    }

    fn failure_receipt(
        &self,
        intent: &EffectIntent,
        params: Option<&LlmCompactParams>,
        provider_id: &str,
        status: ReceiptStatus,
        message: impl Into<String>,
    ) -> EffectReceipt {
        let warnings_ref = self
            .store_json_blob(&serde_json::json!({ "error": message.into() }))
            .ok();
        let receipt = LlmCompactReceipt {
            operation_id: params
                .map(|params| params.operation_id.clone())
                .unwrap_or_else(|| "unknown".into()),
            artifact_kind: LlmCompactionArtifactKind::AosSummary,
            artifact_refs: Vec::new(),
            source_range: params.and_then(|params| params.source_range.clone()),
            compacted_through: None,
            active_window_items: Vec::new(),
            token_usage: Some(TokenUsage {
                prompt: 0,
                completion: 0,
                total: Some(0),
            }),
            provider_metadata_ref: None,
            warnings_ref,
            provider_id: provider_id.to_string(),
        };
        EffectReceipt {
            intent_hash: intent.intent_hash,
            status,
            payload_cbor: serde_cbor::to_vec(&receipt)
                .expect("encode llm.compact failure receipt payload"),
            cost_cents: None,
            signature: vec![0; 64],
        }
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for LlmAdapter<S> {
    fn kind(&self) -> &str {
        aos_effects::effect_ops::LLM_GENERATE
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
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

        let api_key = match Self::resolve_api_key(params.api_key.as_ref()) {
            Ok(key) => key,
            Err(message) => {
                return Ok(self.failure_receipt(
                    intent,
                    &provider_id,
                    ReceiptStatus::Error,
                    message,
                ));
            }
        };

        let messages = match self.load_window_item_messages(
            &params.window_items,
            &provider_id,
            params.model.as_str(),
        ) {
            Ok(messages) => messages,
            Err(err) => {
                return Ok(self.failure_receipt(intent, &provider_id, ReceiptStatus::Error, err));
            }
        };

        let (tools, tool_choice_from_blob) =
            if let Some(tool_refs) = params.runtime.tool_refs.as_ref() {
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

        let response_format = if let Some(reference) = params.runtime.response_format_ref.as_ref() {
            match self.load_response_format(reference) {
                Ok(value) => Some(value),
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
            None
        };

        let provider_options = if let Some(reference) = params.runtime.provider_options_ref.as_ref()
        {
            match self.load_json_blob(reference, "provider_options_ref") {
                Ok(value) => Some(value),
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
            None
        };

        let request = Request {
            model: params.model.clone(),
            messages,
            provider: Some(provider_id.clone()),
            tools,
            tool_choice: params
                .runtime
                .tool_choice
                .as_ref()
                .map(tool_choice_from_params)
                .or(tool_choice_from_blob),
            response_format,
            temperature: params
                .runtime
                .temperature
                .as_ref()
                .and_then(|v| v.parse::<f64>().ok()),
            top_p: params
                .runtime
                .top_p
                .as_ref()
                .and_then(|v| v.parse::<f64>().ok()),
            max_tokens: params.runtime.max_tokens,
            stop_sequences: params.runtime.stop_sequences.clone(),
            reasoning_effort: params.runtime.reasoning_effort.clone(),
            metadata: params
                .runtime
                .metadata
                .clone()
                .map(|m| m.into_iter().collect()),
            provider_options,
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

        let mut normalized_calls: LlmToolCallList = Vec::new();
        for call in response.tool_calls() {
            let arguments_ref = match self.store_json_blob(&call.arguments) {
                Ok(reference) => reference,
                Err(err) => {
                    return Ok(self.failure_receipt(
                        intent,
                        &provider_id,
                        ReceiptStatus::Error,
                        format!("store tool call arguments failed: {err}"),
                    ));
                }
            };
            normalized_calls.push(LlmToolCall {
                call_id: call.id.clone(),
                tool_name: call.name.clone(),
                arguments_ref,
                provider_call_id: call.raw_arguments.as_ref().map(|_| call.id.clone()),
            });
        }

        let tool_calls_ref = if normalized_calls.is_empty() {
            None
        } else {
            let value = match serde_json::to_value(&normalized_calls) {
                Ok(value) => value,
                Err(err) => {
                    return Ok(self.failure_receipt(
                        intent,
                        &provider_id,
                        ReceiptStatus::Error,
                        format!("encode tool_calls failed: {err}"),
                    ));
                }
            };
            match self.store_json_blob(&value) {
                Ok(reference) => Some(reference),
                Err(err) => {
                    return Ok(self.failure_receipt(
                        intent,
                        &provider_id,
                        ReceiptStatus::Error,
                        format!("store tool_calls failed: {err}"),
                    ));
                }
            }
        };

        let reasoning_ref = if let Some(reasoning) = response.reasoning() {
            match self.store_text_blob(&reasoning) {
                Ok(reference) => Some(reference),
                Err(err) => {
                    return Ok(self.failure_receipt(
                        intent,
                        &provider_id,
                        ReceiptStatus::Error,
                        format!("store reasoning failed: {err}"),
                    ));
                }
            }
        } else {
            None
        };

        let envelope = LlmOutputEnvelope {
            assistant_text: {
                let text = response.text();
                if text.is_empty() { None } else { Some(text) }
            },
            tool_calls_ref,
            reasoning_ref,
        };
        let output_ref = match serde_json::to_value(&envelope)
            .map_err(|e| e.to_string())
            .and_then(|v| self.store_json_blob(&v))
        {
            Ok(reference) => reference,
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
            .and_then(|raw| self.store_json_blob(raw).ok());
        let provider_context_items = match self.store_provider_context_items(
            &response,
            &params.window_items,
            &provider_id,
            params.model.as_str(),
            provider.api_kind,
        ) {
            Ok(items) => items,
            Err(err) => {
                return Ok(self.failure_receipt(
                    intent,
                    &provider_id,
                    ReceiptStatus::Error,
                    format!("store provider context items failed: {err}"),
                ));
            }
        };

        let warnings_ref = if response.warnings.is_empty() {
            None
        } else {
            serde_json::to_value(&response.warnings)
                .ok()
                .and_then(|value| self.store_json_blob(&value).ok())
        };

        let rate_limit_ref = response
            .rate_limit
            .as_ref()
            .and_then(|rate_limit| serde_json::to_value(rate_limit).ok())
            .and_then(|value| self.store_json_blob(&value).ok());

        let receipt = LlmGenerateReceipt {
            output_ref,
            raw_output_ref,
            provider_response_id: Some(response.id.clone()),
            provider_context_items,
            finish_reason: LlmFinishReason {
                reason: response.finish_reason.reason.clone(),
                raw: response.finish_reason.raw.clone(),
            },
            token_usage: TokenUsage {
                prompt: response.usage.input_tokens,
                completion: response.usage.output_tokens,
                total: Some(response.usage.total_tokens),
            },
            usage_details: Some(LlmUsageDetails {
                reasoning_tokens: response.usage.reasoning_tokens,
                cache_read_tokens: response.usage.cache_read_tokens,
                cache_write_tokens: response.usage.cache_write_tokens,
            }),
            warnings_ref,
            rate_limit_ref,
            cost_cents: None,
            provider_id: provider_id.clone(),
        };

        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt)?,
            cost_cents: None,
            signature: vec![0; 64],
        })
    }
}

impl<S: Store> LlmCountTokensAdapter<S> {
    pub fn new(store: Arc<S>, config: LlmAdapterConfig) -> Self {
        Self { store, config }
    }

    fn provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.config.providers.get(name)
    }

    fn adapter(&self) -> LlmAdapter<S> {
        LlmAdapter {
            store: self.store.clone(),
            config: self.config.clone(),
        }
    }

    async fn count_with_provider(
        &self,
        provider: &ProviderConfig,
        api_key: String,
        request: TokenCountRequest,
    ) -> Result<aos_llm::TokenCountResponse, SDKError> {
        match provider.api_kind {
            LlmApiKind::Responses => {
                let mut cfg = OpenAIAdapterConfig::new(api_key);
                cfg.base_url = provider.base_url.clone();
                cfg.timeout = to_adapter_timeout(provider.timeout);
                let adapter = OpenAIAdapter::new(cfg)?;
                adapter.count_tokens(request).await
            }
            LlmApiKind::AnthropicMessages => {
                let mut cfg = AnthropicAdapterConfig::new(api_key);
                cfg.base_url = provider.base_url.clone();
                cfg.timeout = to_adapter_timeout(provider.timeout);
                let adapter = AnthropicAdapter::new(cfg)?;
                adapter.count_tokens(request).await
            }
            LlmApiKind::ChatCompletions => {
                Err(SDKError::Configuration(aos_llm::ConfigurationError::new(
                    "OpenAI-compatible chat completions does not support token counting",
                )))
            }
        }
    }

    fn failure_receipt(
        &self,
        intent: &EffectIntent,
        params: Option<&LlmCountTokensParams>,
        provider_id: &str,
        status: ReceiptStatus,
        message: impl Into<String>,
    ) -> EffectReceipt {
        let adapter = self.adapter();
        let warnings_ref = adapter
            .store_json_blob(&serde_json::json!({ "error": message.into() }))
            .ok();
        let receipt = LlmCountTokensReceipt {
            input_tokens: None,
            original_input_tokens: None,
            counts_by_ref: Vec::new(),
            tool_tokens: None,
            response_format_tokens: None,
            quality: LlmTokenCountQuality::Unknown,
            provider: provider_id.to_string(),
            model: params
                .map(|params| params.model.clone())
                .unwrap_or_else(|| "unknown".into()),
            candidate_plan_id: params.and_then(|params| params.candidate_plan_id.clone()),
            provider_metadata_ref: None,
            warnings_ref,
        };
        EffectReceipt {
            intent_hash: intent.intent_hash,
            status,
            payload_cbor: serde_cbor::to_vec(&receipt)
                .expect("encode llm.count_tokens failure receipt payload"),
            cost_cents: None,
            signature: vec![0; 64],
        }
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for LlmCountTokensAdapter<S> {
    fn kind(&self) -> &str {
        aos_effects::effect_ops::LLM_COUNT_TOKENS
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: LlmCountTokensParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => {
                return Ok(self.failure_receipt(
                    intent,
                    None,
                    "unknown",
                    ReceiptStatus::Error,
                    format!("decode LlmCountTokensParams: {err}"),
                ));
            }
        };

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
                    Some(&params),
                    &provider_id,
                    ReceiptStatus::Error,
                    format!("unknown provider {provider_id}"),
                ));
            }
        };
        let adapter = self.adapter();
        let messages = match adapter.load_window_item_messages(
            &params.window_items,
            &provider_id,
            params.model.as_str(),
        ) {
            Ok(messages) => messages,
            Err(err) => {
                return Ok(self.failure_receipt(
                    intent,
                    Some(&params),
                    &provider_id,
                    ReceiptStatus::Error,
                    err,
                ));
            }
        };
        let (tools, tool_tokens) = if let Some(reference) = params.tool_definitions_ref.as_ref() {
            match adapter.load_tools_blobs(core::slice::from_ref(reference)) {
                Ok((tools, _choice)) => {
                    let tokens =
                        estimate_json_tokens(&serde_json::to_value(&tools).unwrap_or(Value::Null));
                    (Some(tools), Some(tokens))
                }
                Err(err) => {
                    return Ok(self.failure_receipt(
                        intent,
                        Some(&params),
                        &provider_id,
                        ReceiptStatus::Error,
                        err,
                    ));
                }
            }
        } else {
            (None, None)
        };
        let response_format = if let Some(reference) = params.response_format_ref.as_ref() {
            match adapter.load_response_format(reference) {
                Ok(value) => Some(value),
                Err(err) => {
                    return Ok(self.failure_receipt(
                        intent,
                        Some(&params),
                        &provider_id,
                        ReceiptStatus::Error,
                        err,
                    ));
                }
            }
        } else {
            None
        };
        let response_format_tokens = response_format
            .as_ref()
            .map(|value| estimate_json_tokens(&serde_json::to_value(value).unwrap_or(Value::Null)));
        let provider_options = if let Some(reference) = params.provider_options_ref.as_ref() {
            match adapter.load_json_blob(reference, "provider_options_ref") {
                Ok(value) => Some(value),
                Err(err) => {
                    return Ok(self.failure_receipt(
                        intent,
                        Some(&params),
                        &provider_id,
                        ReceiptStatus::Error,
                        err,
                    ));
                }
            }
        } else {
            None
        };

        let counts_by_ref = params
            .window_items
            .iter()
            .map(|item| LlmTokenCountByRef {
                ref_: item.ref_.clone(),
                tokens: item
                    .estimated_tokens
                    .unwrap_or_else(|| estimate_text_tokens(item.item_id.as_str())),
                quality: LlmTokenCountQuality::LocalEstimate,
            })
            .collect::<Vec<_>>();

        let request = TokenCountRequest {
            model: params.model.clone(),
            messages: messages.clone(),
            provider: Some(provider_id.clone()),
            tools,
            tool_choice: None,
            response_format,
            provider_options,
        };
        let (input_tokens, original_input_tokens, quality, provider_metadata_ref, warnings_ref) =
            match LlmAdapter::<S>::resolve_api_key(params.api_key.as_ref()) {
                Ok(api_key) => match self.count_with_provider(provider, api_key, request).await {
                    Ok(response) => {
                        let provider_metadata_ref = response
                            .raw
                            .as_ref()
                            .and_then(|raw| adapter.store_json_blob(raw).ok());
                        let warnings_ref = if response.warnings.is_empty() {
                            None
                        } else {
                            serde_json::to_value(&response.warnings)
                                .ok()
                                .and_then(|value| adapter.store_json_blob(&value).ok())
                        };
                        (
                            response.input_tokens,
                            response.original_input_tokens,
                            token_count_quality(response.quality),
                            provider_metadata_ref,
                            warnings_ref,
                        )
                    }
                    Err(_) => {
                        let estimate = estimate_messages_tokens(&messages)
                            .saturating_add(tool_tokens.unwrap_or(0))
                            .saturating_add(response_format_tokens.unwrap_or(0));
                        (
                            Some(estimate),
                            Some(estimate),
                            LlmTokenCountQuality::LocalEstimate,
                            None,
                            None,
                        )
                    }
                },
                Err(_) => {
                    let estimate = estimate_messages_tokens(&messages)
                        .saturating_add(tool_tokens.unwrap_or(0))
                        .saturating_add(response_format_tokens.unwrap_or(0));
                    (
                        Some(estimate),
                        Some(estimate),
                        LlmTokenCountQuality::LocalEstimate,
                        None,
                        None,
                    )
                }
            };

        let receipt = LlmCountTokensReceipt {
            input_tokens,
            original_input_tokens,
            counts_by_ref,
            tool_tokens,
            response_format_tokens,
            quality,
            provider: provider_id,
            model: params.model,
            candidate_plan_id: params.candidate_plan_id,
            provider_metadata_ref,
            warnings_ref,
        };

        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt)?,
            cost_cents: None,
            signature: vec![0; 64],
        })
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for LlmCompactAdapter<S> {
    fn kind(&self) -> &str {
        aos_effects::effect_ops::LLM_COMPACT
    }

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: LlmCompactParams = match serde_cbor::from_slice(&intent.params_cbor) {
            Ok(params) => params,
            Err(err) => {
                return Ok(self.failure_receipt(
                    intent,
                    None,
                    "unknown",
                    ReceiptStatus::Error,
                    format!("decode LlmCompactParams: {err}"),
                ));
            }
        };

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
                    Some(&params),
                    &provider_id,
                    ReceiptStatus::Error,
                    format!("unknown provider {provider_id}"),
                ));
            }
        };
        let api_key = match LlmAdapter::<S>::resolve_api_key(params.api_key.as_ref()) {
            Ok(key) => key,
            Err(message) => {
                return Ok(self.failure_receipt(
                    intent,
                    Some(&params),
                    &provider_id,
                    ReceiptStatus::Error,
                    message,
                ));
            }
        };
        if let Err(err) = LlmAdapter::<S>::render_window_item_refs(
            &params.source_window_items,
            &provider_id,
            params.model.as_str(),
        ) {
            return Ok(self.failure_receipt(
                intent,
                Some(&params),
                &provider_id,
                ReceiptStatus::Error,
                err,
            ));
        }
        let messages = match self.load_window_item_messages(
            &params.source_window_items,
            &provider_id,
            params.model.as_str(),
        ) {
            Ok(messages) => messages,
            Err(err) => {
                return Ok(self.failure_receipt(
                    intent,
                    Some(&params),
                    &provider_id,
                    ReceiptStatus::Error,
                    err,
                ));
            }
        };
        let provider_options = if let Some(reference) = params.provider_options_ref.as_ref() {
            match self.load_json_blob(reference, "provider_options_ref") {
                Ok(value) => Some(value),
                Err(err) => {
                    return Ok(self.failure_receipt(
                        intent,
                        Some(&params),
                        &provider_id,
                        ReceiptStatus::Error,
                        err,
                    ));
                }
            }
        } else {
            None
        };

        let request = CompactionRequest {
            model: params.model.clone(),
            messages,
            provider: Some(provider_id.clone()),
            target_tokens: params.target_tokens,
            provider_options,
        };
        let response = match self.compact_with_provider(provider, api_key, request).await {
            Ok(response) => response,
            Err(err) => {
                let status = map_error_status(&err);
                return Ok(self.failure_receipt(
                    intent,
                    Some(&params),
                    &provider_id,
                    status,
                    format!("provider compact request failed: {err}"),
                ));
            }
        };

        let source_refs = params
            .source_window_items
            .iter()
            .map(|item| item.ref_.clone())
            .collect::<Vec<_>>();
        let api_kind = llm_api_kind_name(provider.api_kind).to_string();
        let mut artifact_refs = Vec::new();
        let mut active_window_items = Vec::new();
        for (idx, item) in response.output_items.iter().enumerate() {
            let raw_ref = match self.store_json_blob(&item.raw) {
                Ok(reference) => reference,
                Err(err) => {
                    return Ok(self.failure_receipt(
                        intent,
                        Some(&params),
                        &provider_id,
                        ReceiptStatus::Error,
                        format!("store compaction item failed: {err}"),
                    ));
                }
            };
            let is_artifact = matches!(item.kind, CompactionItemKind::Compaction);
            if is_artifact {
                artifact_refs.push(raw_ref.clone());
            }
            active_window_items.push(LlmWindowItem {
                item_id: item.id.clone().unwrap_or_else(|| {
                    format!("compact:{}:provider-item:{idx}", params.operation_id)
                }),
                kind: if is_artifact {
                    LlmWindowItemKind::ProviderNativeArtifactRef
                } else {
                    LlmWindowItemKind::ProviderRawWindowRef
                },
                ref_: raw_ref,
                lane: Some("Summary".into()),
                source_range: params.source_range.clone(),
                source_refs: source_refs.clone(),
                provider_compatibility: Some(LlmProviderCompatibility {
                    provider: provider_id.clone(),
                    api_kind: api_kind.clone(),
                    model: Some(response.model.clone()),
                    model_family: None,
                    artifact_type: item
                        .raw
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("provider_item")
                        .to_string(),
                    opaque: item.encrypted_content.is_some(),
                    encrypted: item.encrypted_content.is_some(),
                }),
                estimated_tokens: None,
                metadata: Default::default(),
            });
        }

        if active_window_items.is_empty() {
            active_window_items.extend(params.preserve_window_items.clone());
            active_window_items.extend(params.recent_tail_items.clone());
        }
        if artifact_refs.is_empty() {
            artifact_refs = active_window_items
                .iter()
                .map(|item| item.ref_.clone())
                .collect();
        }

        let provider_metadata_ref = response
            .raw
            .as_ref()
            .and_then(|raw| self.store_json_blob(raw).ok());
        let warnings_ref = if response.warnings.is_empty() {
            None
        } else {
            serde_json::to_value(&response.warnings)
                .ok()
                .and_then(|value| self.store_json_blob(&value).ok())
        };
        let receipt = LlmCompactReceipt {
            operation_id: params.operation_id.clone(),
            artifact_kind: LlmCompactionArtifactKind::ProviderNative,
            artifact_refs,
            source_range: params.source_range.clone(),
            compacted_through: params.source_range.as_ref().map(|range| range.end_seq),
            active_window_items,
            token_usage: Some(TokenUsage {
                prompt: response.usage.input_tokens,
                completion: response.usage.output_tokens,
                total: Some(response.usage.total_tokens),
            }),
            provider_metadata_ref,
            warnings_ref,
            provider_id,
        };

        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
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

fn llm_api_kind_name(kind: LlmApiKind) -> &'static str {
    match kind {
        LlmApiKind::Responses => "responses",
        LlmApiKind::ChatCompletions => "chat_completions",
        LlmApiKind::AnthropicMessages => "anthropic_messages",
    }
}

fn token_count_quality(value: aos_llm::TokenCountQuality) -> LlmTokenCountQuality {
    match value {
        aos_llm::TokenCountQuality::Exact => LlmTokenCountQuality::Exact,
        aos_llm::TokenCountQuality::ProviderEstimate => LlmTokenCountQuality::ProviderEstimate,
        aos_llm::TokenCountQuality::LocalEstimate => LlmTokenCountQuality::LocalEstimate,
        aos_llm::TokenCountQuality::Unknown => LlmTokenCountQuality::Unknown,
    }
}

fn estimate_messages_tokens(messages: &[Message]) -> u64 {
    messages
        .iter()
        .map(|message| {
            4_u64
                .saturating_add(estimate_text_tokens(message.text().as_str()))
                .saturating_add(estimate_json_tokens(
                    &serde_json::to_value(&message.content).unwrap_or(Value::Null),
                ))
        })
        .sum()
}

fn estimate_json_tokens(value: &Value) -> u64 {
    estimate_text_tokens(value.to_string().as_str())
}

fn estimate_text_tokens(value: &str) -> u64 {
    let chars = value.chars().count() as u64;
    let words = value.split_whitespace().count() as u64;
    words.max(chars.saturating_add(3) / 4).max(1)
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
                    "compaction" => {
                        return Ok(vec![Message {
                            role: Role::Assistant,
                            content: vec![ContentPart::provider_item(
                                "compaction",
                                Value::Object(obj.clone()),
                            )],
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
                "compaction" => Ok(Some(ContentPart::provider_item(
                    "compaction",
                    value.clone(),
                ))),
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

fn empty_object() -> Value {
    Value::Object(Default::default())
}

fn zero_hashref() -> HashRef {
    HashRef::new(format!("sha256:{}", "0".repeat(64))).expect("zero hash is valid")
}
