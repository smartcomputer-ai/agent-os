use std::sync::Arc;

use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::{LlmGenerateParams, LlmGenerateReceipt, TokenUsage};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use reqwest::Client;
use serde::Deserialize;

use super::traits::AsyncEffectAdapter;
use crate::config::{LlmAdapterConfig, ProviderConfig};
use aos_store::Store;

/// LLM adapter that targets OpenAI-compatible chat/completions API.
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

        let messages = match self.load_messages(&params.message_refs) {
            Ok(messages) => messages,
            Err(err) => return Ok(self.error_receipt(intent, &provider_id, err)),
        };

        let temperature: f64 = params.temperature.parse().unwrap_or(0.7);

        let mut body = serde_json::json!({
            "model": params.model,
            "messages": messages,
            "max_tokens": params.max_tokens,
            "temperature": temperature,
        });

        if let Some(tools) = params.tools.as_ref() {
            if !tools.is_empty() {
                body["tools"] = serde_json::json!(tools);
            }
        }

        let url = format!(
            "{}/chat/completions",
            provider.base_url.trim_end_matches('/')
        );

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

        let api_response: OpenAiResponse = match response.json().await {
            Ok(r) => r,
            Err(e) => {
                return Ok(self.error_receipt(intent, &provider_id, format!("parse error: {e}")));
            }
        };

        let content = api_response
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        let output_message = serde_json::json!({
            "role": "assistant",
            "content": [
                { "type": "text", "text": content }
            ]
        });
        let output_bytes = match serde_json::to_vec(&output_message) {
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

        let usage = TokenUsage {
            prompt: api_response.usage.prompt_tokens,
            completion: api_response.usage.completion_tokens,
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
    fn load_messages(&self, refs: &[HashRef]) -> Result<Vec<serde_json::Value>, String> {
        let mut messages = Vec::new();
        for reference in refs {
            let mut loaded = self.load_message(reference)?;
            messages.append(&mut loaded);
        }
        Ok(messages)
    }

    fn load_message(&self, reference: &HashRef) -> Result<Vec<serde_json::Value>, String> {
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
                        messages.push(normalize_message(item, self.store.as_ref())?);
                    }
                    return Ok(messages);
                }
                serde_json::Value::Object(_) => {
                    return Ok(vec![normalize_message(value, self.store.as_ref())?]);
                }
                _ => {}
            }
        }

        let text = String::from_utf8(bytes)
            .map_err(|e| format!("message blob is not utf8 or JSON: {e}"))?;
        Ok(vec![serde_json::json!({
            "role": "user",
            "content": [ { "type": "text", "text": text } ]
        })])
    }
}

fn normalize_message<S: Store>(
    value: serde_json::Value,
    store: &S,
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

    let content = normalize_content(content, store)?;
    let mut msg = serde_json::json!({
        "role": role,
        "content": content,
    });

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
) -> Result<serde_json::Value, String> {
    match value {
        serde_json::Value::String(text) => Ok(serde_json::json!([{ "type": "text", "text": text }])),
        serde_json::Value::Array(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                parts.push(normalize_part(item, store)?);
            }
            Ok(serde_json::Value::Array(parts))
        }
        _ => Err("message content must be string or list".to_string()),
    }
}

fn normalize_part<S: Store>(
    value: &serde_json::Value,
    store: &S,
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
            Ok(serde_json::json!({ "type": "text", "text": text }))
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
            Ok(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": data_url }
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
            Ok(serde_json::json!({
                "type": "input_audio",
                "input_audio": { "data": B64.encode(bytes), "format": format }
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
struct OpenAiResponse {
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

fn zero_hashref() -> HashRef {
    HashRef::new("sha256:0000000000000000000000000000000000000000000000000000000000000000")
        .expect("static zero hashref")
}
