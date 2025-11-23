use anyhow::{Context, Result, anyhow};
use aos_air_exec::{Value as ExprValue, ValueKey};
use aos_effects::builtins::{HeaderMap, HttpRequestParams};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::Kernel;
use aos_store::{FsStore, Store};
use serde::Deserialize;
use serde::Serialize;
use serde_cbor;

const HTTP_ADAPTER_ID: &str = "http.mock";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequestContext {
    pub intent: EffectIntent,
    pub params: HttpRequestParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockHttpResponse {
    pub status: i64,
    pub headers: HeaderMap,
    pub body: String,
}

impl MockHttpResponse {
    pub fn json(status: i64, body: impl Into<String>) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type".into(),
            "application/json; charset=utf-8".into(),
        );
        Self {
            status,
            headers,
            body: body.into(),
        }
    }
}

pub struct HttpHarness;

impl HttpHarness {
    pub fn new() -> Self {
        Self
    }

    pub fn collect_requests(
        &mut self,
        kernel: &mut Kernel<FsStore>,
    ) -> Result<Vec<HttpRequestContext>> {
        let mut out = Vec::new();
        loop {
            let intents = kernel.drain_effects();
            if intents.is_empty() {
                break;
            }
            for intent in intents {
                match intent.kind.as_str() {
                    EffectKind::HTTP_REQUEST => {
                        let params: HttpRequestParams = serde_cbor::from_slice(&intent.params_cbor)
                            .context("decode http request params")?;
                        out.push(HttpRequestContext { intent, params });
                    }
                    other => {
                        return Err(anyhow!("unexpected effect kind {other}"));
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn respond_with(
        &self,
        kernel: &mut Kernel<FsStore>,
        ctx: HttpRequestContext,
        response: MockHttpResponse,
    ) -> Result<()> {
        self.respond_with_body(kernel, None, ctx, response)
    }

    pub fn respond_with_body(
        &self,
        kernel: &mut Kernel<FsStore>,
        store: Option<&FsStore>,
        ctx: HttpRequestContext,
        response: MockHttpResponse,
    ) -> Result<()> {
        let receipt_value =
            build_http_receipt_value(response.status, &response.headers, response.body, store)?;
        let receipt = EffectReceipt {
            intent_hash: ctx.intent.intent_hash,
            adapter_id: HTTP_ADAPTER_ID.into(),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt_value)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        };
        kernel.handle_receipt(receipt)?;
        kernel.tick_until_idle()?;
        Ok(())
    }
}

fn build_http_receipt_value(
    status: i64,
    headers: &HeaderMap,
    body: String,
    store: Option<&impl Store>,
) -> Result<ExprValue> {
    let mut record = indexmap::IndexMap::new();
    record.insert("status".into(), ExprValue::Int(status));
    record.insert("headers".into(), headers_to_value(&redact_headers(headers)));
    record.insert("body_preview".into(), ExprValue::Text(body.clone()));
    if let Some(store) = store {
        let hash = store
            .put_blob(body.as_bytes())
            .context("store http response body")?;
        record.insert("body_ref".into(), ExprValue::Text(hash.to_hex()));
    }
    let mut timings = indexmap::IndexMap::new();
    timings.insert("start_ns".into(), ExprValue::Nat(10));
    timings.insert("end_ns".into(), ExprValue::Nat(20));
    record.insert("timings".into(), ExprValue::Record(timings));
    record.insert("adapter_id".into(), ExprValue::Text(HTTP_ADAPTER_ID.into()));
    Ok(ExprValue::Record(record))
}

fn headers_to_value(headers: &HeaderMap) -> ExprValue {
    let mut map = aos_air_exec::ValueMap::new();
    for (key, value) in headers {
        map.insert(ValueKey::Text(key.clone()), ExprValue::Text(value.clone()));
    }
    ExprValue::Map(map)
}

fn redact_headers(headers: &HeaderMap) -> HeaderMap {
    let mut redacted = HeaderMap::new();
    for (k, v) in headers {
        let lower = k.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "authorization" | "proxy-authorization" | "x-api-key" | "api-key"
        ) {
            redacted.insert(k.clone(), "<redacted>".into());
        } else {
            redacted.insert(k.clone(), v.clone());
        }
    }
    redacted
}
