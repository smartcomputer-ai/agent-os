# P3: HTTP + LLM Adapters

**Goal:** Ship real HTTP and LLM adapters that use the canonical AIR types from `aos-effects::builtins`, with CAS-based body/prompt/output handling, and integrate with WorldHost/daemon.

## Status (2025-12-03)

- ✅ HTTP adapter implemented with canonical params/receipt, CAS body_ref in/out, monotonic timings, size cap, and error receipts for network/timeout/invalid refs.
- ✅ LLM adapter implemented (OpenAI-compatible) with canonical params/receipt, CAS input/output, token_usage; cost_cents currently left `None` to avoid nondeterministic estimates; provider map (openai default). API keys must come from params (secret/literal); no host-side fallback.
- ✅ Feature gates `adapter-http` / `adapter-llm` added (default on); registry wires real adapters, falls back to stubs if disabled.
- ✅ HostConfig extended with `http` and `llm`; default constructed from env for URLs/timeouts (no keys).
- ⚠️ Tests/smoke: adapter-specific tests and example runs (03-fetch-notify, 07-llm-summarizer) still to run/regress.
- ⚠️ Docs below kept for design; “CLI auto-registers with OPENAI_API_KEY” no longer true—adapter registers regardless, key must be provided in params via secrets.

## Design Principles

1. **Use canonical types**: Import `HttpRequestParams`, `HttpRequestReceipt`, `LlmGenerateParams`, `LlmGenerateReceipt` from `aos-effects::builtins`. Do not redefine.
2. **CAS everywhere**: Request/response bodies and LLM prompts/outputs are stored in the content-addressed store via `body_ref`, `input_ref`, `output_ref`.
3. **Error → Receipt**: Adapter-level failures (HTTP 5xx, rate limits, missing API key, timeout) become `ReceiptStatus::Error` receipts with structured payloads. Host errors are reserved for "can't reach adapter" scenarios.
4. **No host-level guards**: Enforcement lives in AIR via CapGrants/policy (validated at enqueue time). Keep worlds portable/deterministic by avoiding node-local allowlists.

## HTTP Adapter

### Implementation

```rust
// adapters/http.rs
use aos_effects::builtins::{HttpRequestParams, HttpRequestReceipt, RequestTimings, HeaderMap};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus, AdapterError};
use aos_store::Store;
use reqwest::Client;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct HttpAdapter<S: Store> {
    client: Client,
    store: Arc<S>,
    config: HttpAdapterConfig,
}

pub struct HttpAdapterConfig {
    /// Default timeout for requests
    pub timeout: Duration,
    /// Maximum response body size
    pub max_body_size: usize,
}

impl Default for HttpAdapterConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_body_size: 10 * 1024 * 1024, // 10MB
        }
    }
}

impl<S: Store + Send + Sync + 'static> HttpAdapter<S> {
    pub fn new(store: Arc<S>, config: HttpAdapterConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("build http client");
        Self { client, store, config }
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for HttpAdapter<S> {
    fn kind(&self) -> &str { "http.request" }

    async fn execute(&self, intent: &EffectIntent) -> Result<EffectReceipt, AdapterError> {
        // Parse canonical params from CBOR
        let params: HttpRequestParams = serde_cbor::from_slice(&intent.params_cbor)
            .map_err(|e| AdapterError::InvalidParams(e.to_string()))?;

        // Capability/policy enforcement already happened at enqueue time (manifest/AIR).

        // Read request body from CAS if present
        let body_bytes = match &params.body_ref {
            Some(hash_ref) => {
                let hash = match aos_cbor::Hash::from_hex_str(hash_ref.as_str()) {
                    Ok(h) => h,
                    Err(e) => return Ok(self.error_receipt(
                        intent, "invalid_body_ref", format!("invalid body_ref hash: {}", e)
                    )),
                };
                match self.store.get_blob(hash) {
                    Ok(bytes) => Some(bytes),
                    Err(e) => return Ok(self.error_receipt(
                        intent, "body_ref_not_found", format!("body_ref not in CAS: {}", e)
                    )),
                }
            }
            None => None,
        };

        // Build request
        let mut req = match params.method.to_uppercase().as_str() {
            "GET" => self.client.get(&params.url),
            "POST" => self.client.post(&params.url),
            "PUT" => self.client.put(&params.url),
            "DELETE" => self.client.delete(&params.url),
            "PATCH" => self.client.patch(&params.url),
            "HEAD" => self.client.head(&params.url),
            _ => return Ok(self.error_receipt(
                intent,
                "unsupported_method",
                format!("unsupported method: {}", params.method),
            )),
        };

        // Add headers
        for (key, value) in &params.headers {
            req = req.header(key, value);
        }

        // Add body if present
        if let Some(body) = body_bytes {
            req = req.body(body);
        }

        // Execute request with timing
        let start = Instant::now();
        let start_ns = now_monotonic_ns();

        let response = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                return Ok(self.error_receipt(
                    intent,
                    "request_failed",
                    e.to_string(),
                ));
            }
        };

        let end_ns = now_monotonic_ns();

        let status = response.status().as_u16() as i32;
        let headers: HeaderMap = response.headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str().ok().map(|v| (k.to_string(), v.to_string()))
            })
            .collect();

        // Read response body
        let body = match response.bytes().await {
            Ok(b) => b,
            Err(e) => {
                return Ok(self.error_receipt(
                    intent,
                    "body_read_failed",
                    e.to_string(),
                ));
            }
        };

        // Check body size limit
        if body.len() > self.config.max_body_size {
            return Ok(self.error_receipt(
                intent,
                "body_too_large",
                format!("response body {} bytes exceeds limit {}", body.len(), self.config.max_body_size),
            ));
        }

        // Write response body to CAS
        let body_ref = if !body.is_empty() {
            match self.store.put_blob(&body) {
                Ok(hash) => Some(HashRef::new(hash.to_hex()).unwrap_or_else(|_| unreachable!())),
                Err(e) => return Ok(self.error_receipt(
                    intent, "cas_write_failed", format!("failed to write body to CAS: {}", e)
                )),
            }
        } else {
            None
        };

        // Build canonical receipt
        let receipt_payload = HttpRequestReceipt {
            status,
            headers,
            body_ref,
            timings: RequestTimings { start_ns, end_ns },
            adapter_id: "host.http".into(),
        };

        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "host.http".into(),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt_payload)?,
            cost_cents: None,
            signature: vec![0; 64], // TODO: real signing
        })
    }
}

impl<S: Store> HttpAdapter<S> {
    /// Build an error receipt with structured payload.
    /// Adapter failures → ReceiptStatus::Error (not host errors).
    fn error_receipt(&self, intent: &EffectIntent, code: &str, message: String) -> EffectReceipt {
        let payload = HttpErrorPayload { code: code.into(), message };
        EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "host.http".into(),
            status: ReceiptStatus::Error,
            payload_cbor: serde_cbor::to_vec(&payload).unwrap_or_default(),
            cost_cents: None,
            signature: vec![0; 64],
        }
    }
}

#[derive(Serialize, Deserialize)]
struct HttpErrorPayload {
    code: String,
    message: String,
}

use std::sync::OnceLock;

static START: OnceLock<Instant> = OnceLock::new();

fn now_monotonic_ns() -> u64 {
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_nanos() as u64
}
```

## LLM Adapter

### Implementation

```rust
// adapters/llm.rs
use aos_effects::builtins::{LlmGenerateParams, LlmGenerateReceipt, TokenUsage};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus, AdapterError};
use aos_store::Store;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

pub struct LlmAdapter<S: Store> {
    client: Client,
    store: Arc<S>,
    config: LlmAdapterConfig,
}

/// Per-provider configuration (no keys here; keys come from params as TextOrSecretRef)
pub struct ProviderConfig {
    /// OpenAI-compatible API base URL
    pub base_url: String,
    /// Request timeout
    pub timeout: Duration,
}

pub struct LlmAdapterConfig {
    /// Provider configs keyed by provider id (e.g., "openai", "anthropic", "local")
    pub providers: HashMap<String, ProviderConfig>,
    /// Default provider if not specified in params
    pub default_provider: String,
}

impl LlmAdapterConfig {
    pub fn from_env() -> Result<Self, AdapterError> {
        let base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".into());

        let mut providers = HashMap::new();
        providers.insert("openai".into(), ProviderConfig {
            base_url,
            timeout: Duration::from_secs(120),
        });

        Ok(Self {
            providers,
            default_provider: "openai".into(),
        })
    }

    pub fn get_provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.get(name)
    }
}

impl<S: Store + Send + Sync + 'static> LlmAdapter<S> {
    pub fn new(store: Arc<S>, config: LlmAdapterConfig) -> Self {
        let client = Client::builder()
            .build()
            .expect("build http client");
        Self { client, store, config }
    }

    pub fn from_env(store: Arc<S>) -> Result<Self, AdapterError> {
        let config = LlmAdapterConfig::from_env()?;
        Ok(Self::new(store, config))
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for LlmAdapter<S> {
    fn kind(&self) -> &str { "llm.generate" }

    async fn execute(&self, intent: &EffectIntent) -> Result<EffectReceipt, AdapterError> {
        // Parse canonical params from CBOR
        let params: LlmGenerateParams = serde_cbor::from_slice(&intent.params_cbor)
            .map_err(|e| AdapterError::InvalidParams(e.to_string()))?;

        // Resolve provider config
        let provider_id = &params.provider;
        let provider_config = match self.config.get_provider(provider_id) {
            Some(cfg) => cfg,
            None => return Ok(self.error_receipt(
                intent, "unknown_provider", format!("unknown provider: {}", provider_id)
            )),
        };

        // Resolve API key: must come from params (TextOrSecretRef), not host config
        let api_key = match params.api_key.as_ref() {
            Some(key) => key,
            None => {
                return Ok(self.error_receipt(intent, "api_key_missing", "API key not provided".into()));
            }
        };

        // Fetch prompt/messages from CAS via input_ref
        let input_hash = match aos_cbor::Hash::from_hex_str(params.input_ref.as_str()) {
            Ok(h) => h,
            Err(e) => return Ok(self.error_receipt(
                intent, "invalid_input_ref", format!("invalid input_ref hash: {}", e)
            )),
        };
        let input_bytes = match self.store.get_blob(input_hash) {
            Ok(bytes) => bytes,
            Err(e) => return Ok(self.error_receipt(
                intent, "input_ref_not_found", format!("input_ref not in CAS: {}", e)
            )),
        };

        // Parse input as JSON (messages array)
        let messages: Vec<serde_json::Value> = match serde_json::from_slice(&input_bytes) {
            Ok(m) => m,
            Err(e) => return Ok(self.error_receipt(
                intent, "invalid_input_json", format!("input_ref content is not valid JSON: {}", e)
            )),
        };

        // Parse temperature from decimal string
        let temperature: f64 = params.temperature.parse()
            .unwrap_or(0.7);

        // Build OpenAI-compatible request
        let mut request_body = serde_json::json!({
            "model": params.model,
            "messages": messages,
            "max_tokens": params.max_tokens,
            "temperature": temperature,
        });

        // Add tools if specified
        if !params.tools.is_empty() {
            // Tools are tool names; actual definitions would be looked up from manifest
            // For now, pass through as-is (OpenAI expects full tool definitions)
            request_body["tools"] = serde_json::json!(params.tools);
        }

        // Reuse adapter's client, set per-request timeout
        let response = self.client
            .post(format!("{}/chat/completions", provider_config.base_url))
            .timeout(provider_config.timeout)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await;

        let response = match response {
            Ok(r) => r,
            Err(e) => {
                return Ok(self.error_receipt(intent, "request_failed", e.to_string()));
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Ok(self.error_receipt(
                intent,
                &format!("api_error_{}", status.as_u16()),
                format!("LLM API error {}: {}", status, body),
            ));
        }

        let api_response: OpenAiResponse = match response.json().await {
            Ok(r) => r,
            Err(e) => {
                return Ok(self.error_receipt(intent, "parse_error", e.to_string()));
            }
        };

        // Extract response content
        let content = api_response.choices.first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        // Write output to CAS
        let output_bytes = content.as_bytes();
        let output_ref = match self.store.put_blob(output_bytes) {
            Ok(hash) => HashRef::new(hash.to_hex()).unwrap_or_else(|_| unreachable!()),
            Err(e) => return Ok(self.error_receipt(
                intent, "cas_write_failed", format!("failed to write output to CAS: {}", e)
            )),
        };

        // Build canonical receipt
        let receipt_payload = LlmGenerateReceipt {
            output_ref,
            token_usage: TokenUsage {
                prompt: api_response.usage.prompt_tokens,
                completion: api_response.usage.completion_tokens,
            },
            cost_cents: Some(estimate_cost(&params.model, &api_response.usage)),
            provider_id: provider_id.clone(),
        };

        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: format!("host.llm.{}", provider_id),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt_payload)?,
            cost_cents: receipt_payload.cost_cents,
            signature: vec![0; 64], // TODO: real signing
        })
    }
}

impl<S: Store> LlmAdapter<S> {
    /// Build an error receipt with structured payload.
    fn error_receipt(&self, intent: &EffectIntent, code: &str, message: String) -> EffectReceipt {
        let payload = LlmErrorPayload { code: code.into(), message };
        EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "host.llm".into(),
            status: ReceiptStatus::Error,
            payload_cbor: serde_cbor::to_vec(&payload).unwrap_or_default(),
            cost_cents: None,
            signature: vec![0; 64],
        }
    }
}

#[derive(Serialize, Deserialize)]
struct LlmErrorPayload {
    code: String,
    message: String,
}

/// Estimate cost in cents based on model and token usage.
/// This is approximate; actual pricing varies by provider and model.
fn estimate_cost(model: &str, usage: &OpenAiUsage) -> u64 {
    let total = usage.prompt_tokens + usage.completion_tokens;
    // Rough estimate: $0.01 per 1K tokens for gpt-4o-mini
    // Adjust based on model
    match model {
        m if m.contains("gpt-4o-mini") => (total as u64 + 99) / 100,
        m if m.contains("gpt-4o") => (total as u64 + 9) / 10,
        m if m.contains("gpt-4") => total as u64 / 5,
        _ => (total as u64 + 99) / 100,
    }
}

// OpenAI API response types
#[derive(Deserialize)]
struct OpenAiResponse {
    model: String,
    choices: Vec<Choice>,
    usage: OpenAiUsage,
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
struct OpenAiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}
```

## Configuration

```rust
// config.rs additions
pub struct HostConfig {
    // ... existing fields ...
    pub http: HttpAdapterConfig,
    pub llm: Option<LlmAdapterConfig>,
}

impl HostConfig {
    pub fn from_env() -> Result<Self, HostError> {
        Ok(Self {
            http: HttpAdapterConfig::default(),
            llm: LlmAdapterConfig::from_env().ok(),
        })
    }
}
```

## Integration with WorldHost

```rust
// In WorldHost or daemon initialization
impl<S: Store + Send + Sync + 'static> WorldHost<S> {
    pub fn register_default_adapters(&mut self, store: Arc<S>, config: &HostConfig) {
        // HTTP adapter (always available)
        self.adapters.register(Box::new(HttpAdapter::new(
            store.clone(),
            config.http.clone(),
        )));

        // LLM adapter (only if configured)
        if let Some(llm_config) = &config.llm {
            self.adapters.register(Box::new(LlmAdapter::new(
                store.clone(),
                llm_config.clone(),
            )));
        }
    }
}
```

## Tasks

1. Add `reqwest`, `url` deps; feature-gate adapters behind `adapter-http`/`adapter-llm` (default on).
2. Implement HTTP adapter using canonical `HttpRequestParams`/`HttpRequestReceipt` from `aos-effects::builtins`:
   - Read request body from CAS via `body_ref`
   - Write response body to CAS, return `body_ref`
   - Populate `timings` and `adapter_id`
   - Map errors to error receipts
3. Implement LLM adapter using canonical `LlmGenerateParams`/`LlmGenerateReceipt`:
   - Fetch prompt/messages from CAS via `input_ref`
   - Write output to CAS, return `output_ref`
   - Fill `token_usage`, `cost_cents`, `provider_id`
   - Honor `api_key` from params (literal or secret), no host fallback
   - Support multiple providers via `HostConfig.llm.providers` map
4. Wire adapters into WorldHost with store access for CAS operations.
5. CLI hints: LLM adapter is registered when feature is enabled; callers must supply `api_key` in params/secret (no auto-env key).
6. Smoke-test with examples that use HTTP/LLM effects. (Pending)

## Dependencies (additions)

```toml
reqwest = { version = "0.11", features = ["json"] }
url = "2"
```

## Error Handling

Keep payloads canonical: always emit `HttpRequestReceipt` / `LlmGenerateReceipt`; use `ReceiptStatus` to signal failure. This keeps `await_receipt` decoding stable and replay deterministic.

| Situation | Receipt Status | Payload (canonical schema) |
|-----------|----------------|----------------------------|
| Request succeeds | `Ok` | `HttpRequestReceipt` / `LlmGenerateReceipt` with normal fields |
| HTTP error (4xx/5xx) | `Ok` (preferred) | Receipt with status code, headers/body_ref set as available; caller decides error |
| Network failure | `Error` | `HttpRequestReceipt` with status sentinel (e.g., 599), empty headers, `body_ref` to CAS blob containing error text; timings best-effort |
| Body too large | `Error` | `HttpRequestReceipt` with status sentinel, empty headers, `body_ref` to CAS blob describing limit exceeded |
| Invalid body_ref/input_ref hash | `Error` | `HttpRequestReceipt` / `LlmGenerateReceipt` with `body_ref`/`output_ref` pointing to CAS blob describing the decode error; status sentinel or keep last known status |
| body_ref/input_ref not in CAS | `Error` | Same as above: canonical receipt with `body_ref`/`output_ref` to CAS error text |
| CAS write failed | `Error` | Canonical receipt with `body_ref`/`output_ref` empty, `token_usage` zeros, status sentinel; cost None |
| Unknown provider | `Error` | `LlmGenerateReceipt` with `output_ref` to CAS error text, `token_usage` zeros, `cost_cents` None |
| Missing API key | `Error` | Same as above |
| Invalid input JSON | `Error` | Same as above |
| LLM API error | `Error` (or `Ok` with status code if you prefer) | `LlmGenerateReceipt` with `output_ref` to CAS error text; token_usage zeros |
| Timeout | `Timeout` | Canonical receipt with minimal fields (status sentinel), empty headers/output_ref optional; best-effort timings; cost None |

If richer error metadata is needed, extend the canonical schemas with optional `error`/`error_ref` fields rather than changing payload shapes.

## Success Criteria

- HTTP adapter makes real requests, uses CAS for bodies
- LLM adapter calls OpenAI API, uses CAS for input/output
- Both adapters use canonical types from `aos-effects::builtins`
- Error receipts for failures (not panics or host errors)
- `examples/03-fetch-notify` works with real HTTP
- `examples/07-llm-summarizer` works with real LLM (API key required)
- Replay semantics preserved: same inputs → same receipts (including errors)
