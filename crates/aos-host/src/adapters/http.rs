use once_cell::sync::OnceCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::{HeaderMap, HttpRequestParams, HttpRequestReceipt, RequestTimings};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use async_trait::async_trait;
use reqwest::Client;
use reqwest::header::{HeaderName, HeaderValue};
use tokio::time::timeout;

use super::traits::AsyncEffectAdapter;
use crate::config::HttpAdapterConfig;
use aos_store::Store;

/// HTTP adapter that executes real outbound requests and stores bodies in CAS.
pub struct HttpAdapter<S: Store> {
    client: Client,
    store: Arc<S>,
    config: HttpAdapterConfig,
}

impl<S: Store> HttpAdapter<S> {
    pub fn new(store: Arc<S>, config: HttpAdapterConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("build http client");
        Self {
            client,
            store,
            config,
        }
    }

    fn error_receipt(
        &self,
        intent: &EffectIntent,
        status: i32,
        message: impl Into<String>,
        timings: Option<RequestTimings>,
    ) -> EffectReceipt {
        let msg = message.into();
        let body_ref = self.put_error_blob(&msg);
        let receipt = HttpRequestReceipt {
            status,
            headers: HeaderMap::new(),
            body_ref,
            timings: timings.unwrap_or_else(default_timings),
            adapter_id: "host.http".into(),
        };
        EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "host.http".into(),
            status: ReceiptStatus::Error,
            payload_cbor: serde_cbor::to_vec(&receipt).unwrap_or_default(),
            cost_cents: None,
            signature: vec![0; 64], // TODO: real signing
        }
    }

    fn put_error_blob(&self, msg: &str) -> Option<HashRef> {
        self.store
            .put_blob(msg.as_bytes())
            .ok()
            .and_then(|h| HashRef::new(h.to_hex()).ok())
    }

    fn timeout_receipt(
        &self,
        intent: &EffectIntent,
        timings: Option<RequestTimings>,
    ) -> EffectReceipt {
        let receipt_payload = HttpRequestReceipt {
            status: 598,
            headers: HeaderMap::new(),
            body_ref: None,
            timings: timings.unwrap_or_else(default_timings),
            adapter_id: "host.http".into(),
        };
        EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "host.http".into(),
            status: ReceiptStatus::Timeout,
            payload_cbor: serde_cbor::to_vec(&receipt_payload).unwrap_or_default(),
            cost_cents: None,
            signature: vec![0; 64],
        }
    }
}

#[async_trait]
impl<S: Store + Send + Sync + 'static> AsyncEffectAdapter for HttpAdapter<S> {
    fn kind(&self) -> &str {
        aos_effects::EffectKind::HTTP_REQUEST
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let params: HttpRequestParams = serde_cbor::from_slice(&intent.params_cbor)
            .map_err(|e| anyhow::anyhow!("decode HttpRequestParams: {e}"))?;

        let body_bytes = if let Some(body_ref) = params.body_ref.as_ref() {
            let hash = match Hash::from_hex_str(body_ref.as_str()) {
                Ok(h) => h,
                Err(e) => {
                    return Ok(self.error_receipt(
                        intent,
                        599,
                        format!("invalid body_ref: {e}"),
                        None,
                    ));
                }
            };
            match self.store.get_blob(hash) {
                Ok(bytes) => Some(bytes),
                Err(e) => {
                    return Ok(self.error_receipt(intent, 599, e.to_string(), None));
                }
            }
        } else {
            None
        };

        let mut req = match params.method.to_uppercase().as_str() {
            "GET" => self.client.get(&params.url),
            "POST" => self.client.post(&params.url),
            "PUT" => self.client.put(&params.url),
            "DELETE" => self.client.delete(&params.url),
            "PATCH" => self.client.patch(&params.url),
            "HEAD" => self.client.head(&params.url),
            other => {
                return Ok(self.error_receipt(
                    intent,
                    405,
                    format!("unsupported method {other}"),
                    None,
                ));
            }
        };

        for (k, v) in params.headers.iter() {
            match (
                HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                (Ok(name), Ok(val)) => {
                    req = req.header(name, val);
                }
                _ => {
                    return Ok(self.error_receipt(intent, 400, format!("invalid header {k}"), None));
                }
            }
        }

        if let Some(body) = body_bytes {
            req = req.body(body);
        }

        let start = Instant::now();
        let start_ns = monotonic_ns();

        // Respect host-level timeout as a hard cap.
        let response = match timeout(self.config.timeout, req.send()).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                if e.is_timeout() {
                    return Ok(self.timeout_receipt(
                        intent,
                        Some(timings_from(start_ns, start.elapsed())),
                    ));
                }
                return Ok(self.error_receipt(
                    intent,
                    599,
                    format!("request failed: {e}"),
                    Some(timings_from(start_ns, start.elapsed())),
                ));
            }
            Err(_) => {
                return Ok(
                    self.timeout_receipt(intent, Some(timings_from(start_ns, self.config.timeout)))
                );
            }
        };

        let status = response.status().as_u16() as i32;
        let mut headers = HeaderMap::new();
        for (name, value) in response.headers().iter() {
            if let Ok(v) = value.to_str() {
                headers.insert(name.to_string(), v.to_string());
            }
        }

        let body = match response.bytes().await {
            Ok(b) => b,
            Err(e) => {
                return Ok(self.error_receipt(
                    intent,
                    599,
                    format!("read body failed: {e}"),
                    Some(timings_from(start_ns, start.elapsed())),
                ));
            }
        };

        if body.len() > self.config.max_body_size {
            return Ok(self.error_receipt(
                intent,
                599,
                format!(
                    "response body {} bytes exceeds limit {}",
                    body.len(),
                    self.config.max_body_size
                ),
                Some(timings_from(start_ns, start.elapsed())),
            ));
        }

        let body_ref = if !body.is_empty() {
            let hash = match self.store.put_blob(&body) {
                Ok(h) => h,
                Err(e) => return Ok(self.error_receipt(intent, 599, e.to_string(), None)),
            };
            match HashRef::new(hash.to_hex()) {
                Ok(href) => Some(href),
                Err(e) => return Ok(self.error_receipt(intent, 599, e.to_string(), None)),
            }
        } else {
            None
        };

        let receipt_payload = HttpRequestReceipt {
            status,
            headers,
            body_ref,
            timings: timings_from(start_ns, start.elapsed()),
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

fn timings_from(start_ns: u64, elapsed: Duration) -> RequestTimings {
    RequestTimings {
        start_ns,
        end_ns: start_ns + elapsed.as_nanos() as u64,
    }
}

fn default_timings() -> RequestTimings {
    let now = monotonic_ns();
    RequestTimings {
        start_ns: now,
        end_ns: now,
    }
}

// Monotonic nanoseconds since first call.
fn monotonic_ns() -> u64 {
    static START: OnceCell<Instant> = OnceCell::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_nanos() as u64
}
