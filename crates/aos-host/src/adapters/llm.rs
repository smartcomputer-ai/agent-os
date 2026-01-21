use std::sync::Arc;

use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::{LlmGenerateParams, LlmGenerateReceipt, LlmToolChoice, TokenUsage};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use reqwest::Client;
use serde::Deserialize;

use super::traits::AsyncEffectAdapter;
use crate::config::{LlmAdapterConfig, LlmApiKind, ProviderConfig};
use aos_store::Store;

/// LLM adapter that targets OpenAI-compatible chat/completions and responses APIs.
pub struct LlmAdapter<S: Store> {
    client: Client,
    store: Arc<S>,
    config: LlmAdapterConfig,
}

impl<S: Store> LlmAdapter<S> {
    pub fn new(store: Arc<S>, config: LlmAdapterConfig) -> Self {
        let client = Client::builder().build().expect("build http client");
        Self {
            client,
            store,
            config,
        }
    }

    fn provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.config.providers.get(name)
    }

    fn error_receipt(
        &self,
        intent: &EffectIntent,
        provider: &str,
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
            output_ref,
            token_usage: TokenUsage {
                prompt: 0,
                completion: 0,
            },
            cost_cents: None,
            provider_id: provider.to_string(),
        };
        EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: format!("host.llm.{provider}"),
            status: ReceiptStatus::Error,
            payload_cbor: serde_cbor::to_vec(&receipt).unwrap_or_default(),
            cost_cents: None,
            signature: vec![0; 64],
        }
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
            Some(p) => p,
            None => {
                return Ok(self.error_receipt(
                    intent,
                    &provider_id,
                    format!("unknown provider {provider_id}"),
                ));
            }
        };

        // Resolve API key: must come from params (literal or secret-ref resolved upstream).
        let api_key = match params.api_key.clone() {
            Some(key) if !key.is_empty() => key,
            _ => return Ok(self.error_receipt(intent, &provider_id, "api_key missing")),
        };

        if params.message_refs.is_empty() {
            return Ok(self.error_receipt(
                intent,
                &provider_id,
                "message_refs empty",
            ));
        }

        let messages = match self.load_messages(&params.message_refs, provider.api_kind) {
            Ok(messages) => messages,
            Err(err) => return Ok(self.error_receipt(intent, &provider_id, err)),
        };

        let temperature: f64 = params.temperature.parse().unwrap_or(0.7);

        let (url, mut body) = match provider.api_kind {
            LlmApiKind::ChatCompletions => {
                let body = serde_json::json!({
                    "model": params.model,
                    "messages": messages,
                    "max_tokens": params.max_tokens,
                    "temperature": temperature,
                });
                let url = format!(
                    "{}/chat/completions",
                    provider.base_url.trim_end_matches('/')
                );
                (url, body)
            }
            LlmApiKind::Responses => {
                let body = serde_json::json!({
                    "model": params.model,
                    "input": messages,
                    "max_output_tokens": params.max_tokens,
                    "temperature": temperature,
                });
                let url = format!("{}/responses", provider.base_url.trim_end_matches('/'));
                (url, body)
            }
        };

        if let Some(tool_refs) = params.tool_refs.as_ref() {
            match self.load_tools_blobs(tool_refs) {
                Ok((tools, tool_choice_from_blob)) => {
                    if let Some(tools) = tools {
                        let tools = if provider.api_kind == LlmApiKind::Responses {
                            match normalize_responses_tools(&tools) {
                                Ok(value) => value,
                                Err(err) => {
                                    return Ok(self.error_receipt(intent, &provider_id, err))
                                }
                            }
                        } else {
                            tools
                        };
                        body["tools"] = tools;
                    }
                    if tool_choice_from_blob.is_some() && params.tool_choice.is_none() {
                        let choice = if provider.api_kind == LlmApiKind::Responses {
                            match normalize_responses_tool_choice(tool_choice_from_blob.unwrap()) {
                                Ok(value) => value,
                                Err(err) => {
                                    return Ok(self.error_receipt(intent, &provider_id, err))
                                }
                            }
                        } else {
                            tool_choice_from_blob.unwrap()
                        };
                        body["tool_choice"] = choice;
                    }
                }
                Err(err) => return Ok(self.error_receipt(intent, &provider_id, err)),
            }
        }

        if let Some(tool_choice) = params.tool_choice.as_ref() {
            body["tool_choice"] = if provider.api_kind == LlmApiKind::Responses {
                tool_choice_json_for_responses(tool_choice)
            } else {
                tool_choice_json(tool_choice)
            };
        }

        let response = self
            .client
            .post(url)
            .timeout(provider.timeout)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await;

        let response = match response {
            Ok(r) => r,
            Err(e) => {
                return Ok(self.error_receipt(
                    intent,
                    &provider_id,
                    format!("request failed: {e}"),
                ));
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Ok(self.error_receipt(
                intent,
                &provider_id,
                format!("provider error {}: {}", status, text),
            ));
        }

        let (output_value, usage) = match provider.api_kind {
            LlmApiKind::ChatCompletions => {
                let api_response: OpenAiChatResponse = match response.json().await {
                    Ok(r) => r,
                    Err(e) => {
                        return Ok(
                            self.error_receipt(intent, &provider_id, format!("parse error: {e}")),
                        );
                    }
                };
                let content = api_response
                    .choices
                    .first()
                    .and_then(|c| c.message.content.clone())
                    .unwrap_or_default();
                let output_value = serde_json::json!({
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": content }
                    ]
                });
                let usage = TokenUsage {
                    prompt: api_response.usage.prompt_tokens,
                    completion: api_response.usage.completion_tokens,
                };
                (output_value, usage)
            }
            LlmApiKind::Responses => {
                let api_response: serde_json::Value = match response.json().await {
                    Ok(r) => r,
                    Err(e) => {
                        return Ok(
                            self.error_receipt(intent, &provider_id, format!("parse error: {e}")),
                        );
                    }
                };
                let usage = extract_responses_usage(&api_response);
                let output_value = api_response
                    .get("output")
                    .cloned()
                    .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
                (output_value, usage)
            }
        };

        let output_bytes = match serde_json::to_vec(&output_value) {
            Ok(bytes) => bytes,
            Err(e) => {
                return Ok(self.error_receipt(
                    intent,
                    &provider_id,
                    format!("encode output message failed: {e}"),
                ));
            }
        };

        let output_ref = match self.store.put_blob(&output_bytes) {
            Ok(h) => match HashRef::new(h.to_hex()) {
                Ok(hr) => hr,
                Err(e) => {
                    return Ok(self.error_receipt(
                        intent,
                        &provider_id,
                        format!("invalid output hash: {e}"),
                    ));
                }
            },
            Err(e) => {
                return Ok(self.error_receipt(
                    intent,
                    &provider_id,
                    format!("store output failed: {e}"),
                ));
            }
        };

        let receipt = LlmGenerateReceipt {
            output_ref,
            token_usage: usage.clone(),
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

impl<S: Store> LlmAdapter<S> {
    fn load_messages(
        &self,
        refs: &[HashRef],
        api_kind: LlmApiKind,
    ) -> Result<Vec<serde_json::Value>, String> {
        let mut messages = Vec::new();
        for reference in refs {
            let mut loaded = self.load_message(reference, api_kind)?;
            messages.append(&mut loaded);
        }
        Ok(messages)
    }

    fn load_message(
        &self,
        reference: &HashRef,
        api_kind: LlmApiKind,
    ) -> Result<Vec<serde_json::Value>, String> {
        let hash =
            Hash::from_hex_str(reference.as_str()).map_err(|e| format!("invalid message_ref: {e}"))?;
        let bytes = self
            .store
            .get_blob(hash)
            .map_err(|e| format!("message_ref not found: {e}"))?;

        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            match value {
                serde_json::Value::Array(items) => {
                    let mut messages = Vec::with_capacity(items.len());
                    for item in items {
                        if api_kind == LlmApiKind::Responses
                            && item
                                .as_object()
                                .and_then(|obj| obj.get("type"))
                                .is_some()
                        {
                            messages.push(item);
                        } else {
                            messages.push(normalize_message(item, self.store.as_ref(), api_kind)?);
                        }
                    }
                    return Ok(messages);
                }
                serde_json::Value::Object(_) => {
                    return Ok(vec![normalize_message(value, self.store.as_ref(), api_kind)?]);
                }
                _ => {}
            }
        }

        let text = String::from_utf8(bytes)
            .map_err(|e| format!("message blob is not utf8 or JSON: {e}"))?;
        Ok(vec![serde_json::json!({
            "role": "user",
            "content": [ { "type": content_text_type(api_kind, "user"), "text": text } ]
        })])
    }

    fn load_tools_blobs(
        &self,
        references: &[HashRef],
    ) -> Result<(Option<serde_json::Value>, Option<serde_json::Value>), String> {
        let mut merged_tools: Vec<serde_json::Value> = Vec::new();
        let mut merged_choice: Option<serde_json::Value> = None;

        for reference in references {
            let hash = Hash::from_hex_str(reference.as_str())
                .map_err(|e| format!("invalid tool_ref: {e}"))?;
            let bytes = self
                .store
                .get_blob(hash)
                .map_err(|e| format!("tool_ref not found: {e}"))?;
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|e| format!("tool_ref invalid JSON: {e}"))?;

            match value {
                serde_json::Value::Array(items) => {
                    for item in items {
                        merged_tools.push(item);
                    }
                }
                serde_json::Value::Object(map) => {
                    if let Some(tools) = map.get("tools").and_then(|v| v.as_array()) {
                        for tool in tools {
                            merged_tools.push(tool.clone());
                        }
                    }
                    if let Some(choice) = map.get("tool_choice") {
                        merged_choice = Some(choice.clone());
                    }
                }
                _ => return Err("tool_ref must be JSON array or object".to_string()),
            }
        }

        let tools_value = if merged_tools.is_empty() {
            None
        } else {
            Some(serde_json::Value::Array(merged_tools))
        };

        Ok((tools_value, merged_choice))
    }

}

fn normalize_message<S: Store>(
    value: serde_json::Value,
    store: &S,
    api_kind: LlmApiKind,
) -> Result<serde_json::Value, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "message blob must be a JSON object".to_string())?;
    let role = obj
        .get("role")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "message missing role".to_string())?;
    let content = obj
        .get("content")
        .ok_or_else(|| "message missing content".to_string())?;

    let content = normalize_content(content, store, api_kind, role)?;
    let mut msg = serde_json::json!({
        "role": role,
        "content": content,
    });
    if api_kind == LlmApiKind::Responses {
        msg["type"] = serde_json::Value::String("message".into());
    }

    if let Some(tool_calls) = obj.get("tool_calls") {
        msg["tool_calls"] = tool_calls.clone();
    }
    if let Some(name) = obj.get("name") {
        msg["name"] = name.clone();
    }

    Ok(msg)
}

fn normalize_content<S: Store>(
    value: &serde_json::Value,
    store: &S,
    api_kind: LlmApiKind,
    role: &str,
) -> Result<serde_json::Value, String> {
    match value {
        serde_json::Value::String(text) => Ok(serde_json::json!([{
            "type": content_text_type(api_kind, role),
            "text": text
        }])),
        serde_json::Value::Array(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                parts.push(normalize_part(item, store, api_kind, role)?);
            }
            Ok(serde_json::Value::Array(parts))
        }
        _ => Err("message content must be string or list".to_string()),
    }
}

fn normalize_part<S: Store>(
    value: &serde_json::Value,
    store: &S,
    api_kind: LlmApiKind,
    role: &str,
) -> Result<serde_json::Value, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "content part must be an object".to_string())?;
    let part_type = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "content part missing type".to_string())?;

    match part_type {
        "text" => {
            let text = obj
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "text part missing text".to_string())?;
            Ok(serde_json::json!({
                "type": content_text_type(api_kind, role),
                "text": text
            }))
        }
        "input_text" => {
            let text = obj
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "input_text part missing text".to_string())?;
            Ok(serde_json::json!({
                "type": content_text_type(api_kind, role),
                "text": text
            }))
        }
        "output_text" => {
            let text = obj
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "output_text part missing text".to_string())?;
            Ok(serde_json::json!({
                "type": content_text_type(api_kind, role),
                "text": text
            }))
        }
        "image" => {
            let mime = obj
                .get("mime")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "image part missing mime".to_string())?;
            let bytes_ref = obj
                .get("bytes_ref")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "image part missing bytes_ref".to_string())?;
            let hash = Hash::from_hex_str(bytes_ref)
                .map_err(|e| format!("invalid image bytes_ref: {e}"))?;
            let bytes = store
                .get_blob(hash)
                .map_err(|e| format!("image bytes_ref not found: {e}"))?;
            let data_url = format!("data:{};base64,{}", mime, B64.encode(bytes));
            match api_kind {
                LlmApiKind::Responses => Ok(serde_json::json!({
                    "type": "input_image",
                    "image_url": { "url": data_url }
                })),
                LlmApiKind::ChatCompletions => Ok(serde_json::json!({
                    "type": "image_url",
                    "image_url": { "url": data_url }
                })),
            }
        }
        "input_image" | "image_url" => {
            let image_url = obj
                .get("image_url")
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| "image_url part missing url".to_string())?;
            let target_type = match api_kind {
                LlmApiKind::Responses => "input_image",
                LlmApiKind::ChatCompletions => "image_url",
            };
            Ok(serde_json::json!({
                "type": target_type,
                "image_url": { "url": image_url }
            }))
        }
        "audio" => {
            let mime = obj
                .get("mime")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "audio part missing mime".to_string())?;
            let bytes_ref = obj
                .get("bytes_ref")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "audio part missing bytes_ref".to_string())?;
            let hash = Hash::from_hex_str(bytes_ref)
                .map_err(|e| format!("invalid audio bytes_ref: {e}"))?;
            let bytes = store
                .get_blob(hash)
                .map_err(|e| format!("audio bytes_ref not found: {e}"))?;
            let format = audio_format_from_mime(mime)
                .ok_or_else(|| format!("unsupported audio mime '{mime}'"))?;
            let audio = serde_json::json!({
                "data": B64.encode(bytes),
                "format": format
            });
            Ok(serde_json::json!({
                "type": "input_audio",
                "input_audio": audio
            }))
        }
        "input_audio" => {
            let input_audio = obj
                .get("input_audio")
                .ok_or_else(|| "input_audio part missing input_audio".to_string())?;
            Ok(serde_json::json!({
                "type": "input_audio",
                "input_audio": input_audio
            }))
        }
        other => Err(format!("unsupported content part type '{other}'")),
    }
}

fn audio_format_from_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "audio/wav" | "audio/wave" | "audio/x-wav" => Some("wav"),
        "audio/mpeg" => Some("mp3"),
        "audio/mp4" => Some("m4a"),
        "audio/ogg" => Some("ogg"),
        _ => None,
    }
}

#[derive(Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<Choice>,
    usage: OpenAiUsage,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    content: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    #[serde(rename = "total_tokens")]
    _total_tokens: u64,
}


fn extract_responses_usage(value: &serde_json::Value) -> TokenUsage {
    let prompt = value
        .get("usage")
        .and_then(|v| v.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion = value
        .get("usage")
        .and_then(|v| v.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    TokenUsage { prompt, completion }
}

fn content_text_type(api_kind: LlmApiKind, role: &str) -> &'static str {
    match api_kind {
        LlmApiKind::Responses => {
            if role == "assistant" {
                "output_text"
            } else {
                "input_text"
            }
        }
        LlmApiKind::ChatCompletions => "text",
    }
}

fn tool_choice_json(choice: &LlmToolChoice) -> serde_json::Value {
    match choice {
        LlmToolChoice::Auto => serde_json::json!("auto"),
        LlmToolChoice::NoneChoice => serde_json::json!("none"),
        LlmToolChoice::Required => serde_json::json!("required"),
        LlmToolChoice::Tool { name } => serde_json::json!({
            "type": "function",
            "function": { "name": name }
        }),
    }
}

fn tool_choice_json_for_responses(choice: &LlmToolChoice) -> serde_json::Value {
    match choice {
        LlmToolChoice::Auto => serde_json::json!("auto"),
        LlmToolChoice::NoneChoice => serde_json::json!("none"),
        LlmToolChoice::Required => serde_json::json!("required"),
        LlmToolChoice::Tool { name } => serde_json::json!({
            "type": "function",
            "name": name
        }),
    }
}

fn normalize_responses_tools(value: &serde_json::Value) -> Result<serde_json::Value, String> {
    let array = value
        .as_array()
        .ok_or_else(|| "tools must be a list".to_string())?;
    let mut tools = Vec::with_capacity(array.len());
    for tool in array {
        let obj = tool
            .as_object()
            .ok_or_else(|| "tool must be an object".to_string())?;
        let tool_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or_default();
        if tool_type == "function" && obj.contains_key("function") {
            let function = obj
                .get("function")
                .and_then(|v| v.as_object())
                .ok_or_else(|| "function tool missing function object".to_string())?;
            let name = function
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "function tool missing name".to_string())?;
            let mut out = serde_json::Map::new();
            out.insert("type".into(), serde_json::Value::String("function".into()));
            out.insert("name".into(), serde_json::Value::String(name.to_string()));
            if let Some(desc) = function.get("description") {
                out.insert("description".into(), desc.clone());
            }
            if let Some(params) = function.get("parameters") {
                out.insert("parameters".into(), params.clone());
            }
            if let Some(strict) = function.get("strict") {
                out.insert("strict".into(), strict.clone());
            }
            tools.push(serde_json::Value::Object(out));
        } else {
            tools.push(serde_json::Value::Object(obj.clone()));
        }
    }
    Ok(serde_json::Value::Array(tools))
}

fn normalize_responses_tool_choice(value: serde_json::Value) -> Result<serde_json::Value, String> {
    if let Some(name) = value
        .get("function")
        .and_then(|v| v.get("name"))
        .and_then(|v| v.as_str())
    {
        return Ok(serde_json::json!({
            "type": "function",
            "name": name
        }));
    }
    Ok(value)
}


fn zero_hashref() -> HashRef {
    HashRef::new("sha256:0000000000000000000000000000000000000000000000000000000000000000")
        .expect("static zero hashref")
}
