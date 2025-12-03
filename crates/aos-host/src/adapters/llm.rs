use std::sync::Arc;

use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::{LlmGenerateParams, LlmGenerateReceipt, TokenUsage};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use crate::config::{LlmAdapterConfig, ProviderConfig};
use super::traits::AsyncEffectAdapter;
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
                ))
            }
        };

        // Resolve API key: must come from params (literal or secret-ref resolved upstream).
        let api_key = match params.api_key.clone() {
            Some(key) if !key.is_empty() => key,
            _ => {
                return Ok(self.error_receipt(
                    intent,
                    &provider_id,
                    "api_key missing",
                ))
            }
        };

        // Load prompt/messages from CAS
        let input_hash = match Hash::from_hex_str(params.input_ref.as_str()) {
            Ok(h) => h,
            Err(e) => {
                return Ok(self.error_receipt(
                    intent,
                    &provider_id,
                    format!("invalid input_ref: {e}"),
                ))
            }
        };
        let input_bytes = match self.store.get_blob(input_hash) {
            Ok(b) => b,
            Err(e) => {
                return Ok(self.error_receipt(
                    intent,
                    &provider_id,
                    format!("input_ref not found: {e}"),
                ))
            }
        };

        let messages: serde_json::Value = match serde_json::from_slice(&input_bytes) {
            Ok(v) => v,
            Err(e) => {
                return Ok(self.error_receipt(
                    intent,
                    &provider_id,
                    format!("invalid input JSON: {e}"),
                ))
            }
        };

        let temperature: f64 = params.temperature.parse().unwrap_or(0.7);

        let mut body = serde_json::json!({
            "model": params.model,
            "messages": messages,
            "max_tokens": params.max_tokens,
            "temperature": temperature,
        });

        if !params.tools.is_empty() {
            body["tools"] = serde_json::json!(params.tools);
        }

        let url = format!("{}/chat/completions", provider.base_url.trim_end_matches('/'));

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
                ))
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
                return Ok(self.error_receipt(
                    intent,
                    &provider_id,
                    format!("parse error: {e}"),
                ))
            }
        };

        let content = api_response
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        let output_ref = match self.store.put_blob(content.as_bytes()) {
            Ok(h) => match HashRef::new(h.to_hex()) {
                Ok(hr) => hr,
                Err(e) => {
                    return Ok(self.error_receipt(
                        intent,
                        &provider_id,
                        format!("invalid output hash: {e}"),
                    ))
                }
            },
            Err(e) => {
                return Ok(self.error_receipt(
                    intent,
                    &provider_id,
                    format!("store output failed: {e}"),
                ))
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
    _total_tokens: u64,
}

fn zero_hashref() -> HashRef {
    HashRef::new("sha256:0000000000000000000000000000000000000000000000000000000000000000")
        .expect("static zero hashref")
}
