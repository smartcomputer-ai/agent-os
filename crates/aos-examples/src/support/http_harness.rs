use anyhow::{Context, Result, anyhow};
use aos_air_exec::{Value as ExprValue, ValueKey};
use aos_effects::builtins::{HeaderMap, HttpRequestParams};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::Kernel;
use aos_store::FsStore;
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
                        let params_value: ExprValue =
                            serde_cbor::from_slice(&intent.params_cbor)
                                .context("decode http request params value")?;
                        let params = http_params_from_value(params_value)?;
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
        let receipt_value =
            build_http_receipt_value(response.status, &response.headers, response.body);
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

fn http_params_from_value(value: ExprValue) -> Result<HttpRequestParams> {
    let record = match value {
        ExprValue::Record(map) => map,
        other => return Err(anyhow!("http params must be a record, got {:?}", other)),
    };
    let method = record_text(&record, "method")?;
    let url = record_text(&record, "url")?;
    let headers = match record.get("headers") {
        Some(value) => value_to_headers(value)?,
        None => HeaderMap::new(),
    };
    Ok(HttpRequestParams {
        method,
        url,
        headers,
        body_ref: None,
    })
}

fn record_text(record: &indexmap::IndexMap<String, ExprValue>, field: &str) -> Result<String> {
    match record.get(field) {
        Some(ExprValue::Text(text)) => Ok(text.clone()),
        Some(other) => Err(anyhow!("field '{field}' must be text, got {:?}", other)),
        None => Err(anyhow!("field '{field}' missing from http params")),
    }
}

fn value_to_headers(value: &ExprValue) -> Result<HeaderMap> {
    match value {
        ExprValue::Map(map) => {
            let mut headers = HeaderMap::new();
            for (key, entry) in map {
                let name = match key {
                    ValueKey::Text(text) => text.clone(),
                    other => return Err(anyhow!("header key must be text, got {:?}", other)),
                };
                let val = match entry {
                    ExprValue::Text(text) => text.clone(),
                    other => return Err(anyhow!("header value must be text, got {:?}", other)),
                };
                headers.insert(name, val);
            }
            Ok(headers)
        }
        ExprValue::Null | ExprValue::Unit => Ok(HeaderMap::new()),
        other => Err(anyhow!("headers must be a map, got {:?}", other)),
    }
}

fn build_http_receipt_value(status: i64, headers: &HeaderMap, body: String) -> ExprValue {
    let mut record = indexmap::IndexMap::new();
    record.insert("status".into(), ExprValue::Int(status));
    record.insert("headers".into(), headers_to_value(&redact_headers(headers)));
    record.insert("body_preview".into(), ExprValue::Text(body));
    let mut timings = indexmap::IndexMap::new();
    timings.insert("start_ns".into(), ExprValue::Nat(10));
    timings.insert("end_ns".into(), ExprValue::Nat(20));
    record.insert("timings".into(), ExprValue::Record(timings));
    record.insert("adapter_id".into(), ExprValue::Text(HTTP_ADAPTER_ID.into()));
    ExprValue::Record(record)
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
