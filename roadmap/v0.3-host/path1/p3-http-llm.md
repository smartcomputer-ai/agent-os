# P3: HTTP + LLM Adapters

**Goal:** Real effect adapters for external I/O.

## HTTP Adapter

### Implementation

```rust
// adapters/http.rs
use reqwest::Client;

pub struct HttpAdapter {
    client: Client,
    config: HttpAdapterConfig,
}

pub struct HttpAdapterConfig {
    /// Default timeout for requests
    pub timeout: Duration,
    /// Maximum response body size
    pub max_body_size: usize,
    /// Allowed hosts (if empty, all allowed)
    pub allowed_hosts: Vec<String>,
}

impl Default for HttpAdapterConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_body_size: 10 * 1024 * 1024, // 10MB
            allowed_hosts: vec![],
        }
    }
}

impl HttpAdapter {
    pub fn new(config: HttpAdapterConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("build http client");
        Self { client, config }
    }
}

#[async_trait]
impl AsyncEffectAdapter for HttpAdapter {
    fn kind(&self) -> &str { "http.request" }

    async fn execute(&self, intent: &EffectIntent) -> Result<EffectReceipt, AdapterError> {
        // Parse params from CBOR
        let params: HttpRequestParams = serde_cbor::from_slice(&intent.params_cbor)
            .map_err(|e| AdapterError::InvalidParams(e.to_string()))?;

        // Validate allowed hosts
        if !self.config.allowed_hosts.is_empty() {
            let url = url::Url::parse(&params.url)
                .map_err(|e| AdapterError::InvalidParams(e.to_string()))?;
            let host = url.host_str().unwrap_or("");
            if !self.config.allowed_hosts.iter().any(|h| h == host) {
                return Err(AdapterError::ExecutionFailed(
                    format!("host '{}' not in allowed list", host)
                ));
            }
        }

        // Build request
        let mut req = match params.method.to_uppercase().as_str() {
            "GET" => self.client.get(&params.url),
            "POST" => self.client.post(&params.url),
            "PUT" => self.client.put(&params.url),
            "DELETE" => self.client.delete(&params.url),
            "PATCH" => self.client.patch(&params.url),
            _ => return Err(AdapterError::InvalidParams(
                format!("unsupported method: {}", params.method)
            )),
        };

        // Add headers
        for (key, value) in &params.headers {
            req = req.header(key, value);
        }

        // Add body if present
        if let Some(body) = &params.body {
            req = req.body(body.clone());
        }

        // Execute request
        let response = req.send().await
            .map_err(|e| AdapterError::ExecutionFailed(e.to_string()))?;

        let status = response.status().as_u16();
        let headers: HashMap<String, String> = response.headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str().ok().map(|v| (k.to_string(), v.to_string()))
            })
            .collect();

        let body = response.bytes().await
            .map_err(|e| AdapterError::ExecutionFailed(e.to_string()))?;

        // Check body size
        if body.len() > self.config.max_body_size {
            return Err(AdapterError::ExecutionFailed(
                format!("response body too large: {} bytes", body.len())
            ));
        }

        // Build receipt
        let receipt_payload = HttpResponseReceipt {
            status,
            headers,
            body: body.to_vec(),
        };

        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "host.http".into(),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt_payload)?,
            cost_cents: None,
            signature: vec![0; 64],
        })
    }
}
```

### Param/Receipt Types

```rust
// These should align with spec/defs/builtin-schemas.air.json

#[derive(Serialize, Deserialize)]
pub struct HttpRequestParams {
    pub url: String,
    pub method: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub body: Option<Vec<u8>>,
}

#[derive(Serialize, Deserialize)]
pub struct HttpResponseReceipt {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}
```

## LLM Adapter

### Implementation

```rust
// adapters/llm.rs
use reqwest::Client;

pub struct LlmAdapter {
    client: Client,
    config: LlmAdapterConfig,
}

pub struct LlmAdapterConfig {
    /// OpenAI-compatible API base URL
    pub base_url: String,
    /// API key (from env or config)
    pub api_key: String,
    /// Default model
    pub default_model: String,
    /// Request timeout
    pub timeout: Duration,
    /// Max tokens (if not specified in request)
    pub default_max_tokens: u32,
}

impl LlmAdapter {
    pub fn new(config: LlmAdapterConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("build http client");
        Self { client, config }
    }

    pub fn from_env() -> Result<Self, AdapterError> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| AdapterError::InvalidParams("OPENAI_API_KEY not set".into()))?;

        let base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".into());

        let model = std::env::var("OPENAI_MODEL")
            .unwrap_or_else(|_| "gpt-4o-mini".into());

        Ok(Self::new(LlmAdapterConfig {
            base_url,
            api_key,
            default_model: model,
            timeout: Duration::from_secs(120),
            default_max_tokens: 4096,
        }))
    }
}

#[async_trait]
impl AsyncEffectAdapter for LlmAdapter {
    fn kind(&self) -> &str { "llm.generate" }

    async fn execute(&self, intent: &EffectIntent) -> Result<EffectReceipt, AdapterError> {
        // Parse params
        let params: LlmGenerateParams = serde_cbor::from_slice(&intent.params_cbor)
            .map_err(|e| AdapterError::InvalidParams(e.to_string()))?;

        let model = params.model.as_ref()
            .unwrap_or(&self.config.default_model);

        let max_tokens = params.max_tokens
            .unwrap_or(self.config.default_max_tokens);

        // Build OpenAI-compatible request
        let request_body = serde_json::json!({
            "model": model,
            "messages": params.messages,
            "max_tokens": max_tokens,
            "temperature": params.temperature.unwrap_or(0.7),
        });

        let response = self.client
            .post(format!("{}/chat/completions", self.config.base_url))
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AdapterError::ExecutionFailed(
                format!("LLM API error {}: {}", status, body)
            ));
        }

        let api_response: OpenAiResponse = response.json().await
            .map_err(|e| AdapterError::ExecutionFailed(e.to_string()))?;

        // Extract response
        let content = api_response.choices.first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        let receipt_payload = LlmGenerateReceipt {
            content,
            model: api_response.model,
            prompt_tokens: api_response.usage.prompt_tokens,
            completion_tokens: api_response.usage.completion_tokens,
            total_tokens: api_response.usage.total_tokens,
            finish_reason: api_response.choices.first()
                .map(|c| c.finish_reason.clone()),
        };

        // Estimate cost (rough: $0.01 per 1K tokens for gpt-4o-mini)
        let cost_cents = Some((api_response.usage.total_tokens as u64 + 99) / 100);

        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "host.llm".into(),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt_payload)?,
            cost_cents,
            signature: vec![0; 64],
        })
    }
}

// OpenAI API response types
#[derive(Deserialize)]
struct OpenAiResponse {
    model: String,
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
    finish_reason: String,
}

#[derive(Deserialize)]
struct Message {
    content: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}
```

### Param/Receipt Types

```rust
#[derive(Serialize, Deserialize)]
pub struct LlmGenerateParams {
    pub messages: Vec<LlmMessage>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,      // "system", "user", "assistant"
    pub content: String,
}

#[derive(Serialize, Deserialize)]
pub struct LlmGenerateReceipt {
    pub content: String,
    pub model: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub finish_reason: Option<String>,
}
```

## Configuration

```rust
// config.rs additions
pub struct RuntimeConfig {
    // ... existing fields ...
    pub http: HttpAdapterConfig,
    pub llm: Option<LlmAdapterConfig>,
}

// Environment-based config
impl RuntimeConfig {
    pub fn from_env() -> Result<Self, HostError> {
        Ok(Self {
            http: HttpAdapterConfig::default(),
            llm: LlmAdapter::from_env().ok().map(|a| a.config),
        })
    }
}
```

## Tasks

1. Add `reqwest` dependency with JSON feature
2. Implement `HttpAdapter` with reqwest
3. Implement `LlmAdapter` with OpenAI-compatible API
4. Add param/receipt types matching spec schemas
5. Add host validation (allowed hosts, body size limits)
6. Add environment variable config for API keys
7. Test with `examples/03-fetch-notify` (HTTP)
8. Test with `examples/07-llm-summarizer` (LLM)

## Dependencies (additions)

```toml
reqwest = { version = "0.11", features = ["json"] }
url = "2"
```

## Success Criteria

- HTTP adapter makes real requests to external URLs
- LLM adapter calls OpenAI API successfully
- `examples/03-fetch-notify` works with real HTTP
- `examples/07-llm-summarizer` works with real LLM (API key required)
- Error handling for network failures, timeouts, API errors
- Cost tracking in receipts (at least for LLM)
