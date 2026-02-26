#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::{ReduceError, Reducer, ReducerCtx, aos_reducer, aos_variant};
use serde::{Deserialize, Serialize};
use serde_cbor::Value as CborValue;

const HTTP_REQUEST_EFFECT: &str = "http.request";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FetchState {
    pc: FetchPc,
    next_request_id: u64,
    pending_request: Option<u64>,
    last_status: Option<i64>,
    last_body_preview: Option<String>,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum FetchPc {
        Idle,
        Fetching,
        Done,
    }
}

impl Default for FetchPc {
    fn default() -> Self {
        FetchPc::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartEvent {
    url: String,
    method: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EffectReceiptEnvelope {
    origin_module_id: String,
    origin_instance_key: Option<serde_bytes::ByteBuf>,
    intent_id: String,
    effect_kind: String,
    params_hash: Option<String>,
    receipt_payload: serde_bytes::ByteBuf,
    status: String,
    emitted_at_seq: u64,
    adapter_id: String,
    cost_cents: Option<u64>,
    signature: serde_bytes::ByteBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HttpRequestParams {
    method: String,
    url: String,
    headers: BTreeMap<String, String>,
    body_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct HttpRequestReceiptPayload {
    status: i64,
    body_preview: Option<String>,
}

aos_reducer!(FetchNotifySm);

#[derive(Default)]
struct FetchNotifySm;

impl Reducer for FetchNotifySm {
    type State = FetchState;
    type Event = CborValue;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        let Some((tag, payload)) = decode_tagged_event(event) else {
            return Ok(());
        };
        match tag.as_str() {
            "Start" | "start" => {
                let start: StartEvent = serde_cbor::value::from_value(payload)
                    .map_err(|_| ReduceError::new("invalid Start payload"))?;
                handle_start(ctx, start.url, start.method)?;
            }
            "Receipt" | "receipt" => {
                let envelope: EffectReceiptEnvelope = serde_cbor::value::from_value(payload)
                    .map_err(|_| ReduceError::new("invalid Receipt payload"))?;
                handle_receipt(ctx, envelope)?;
            }
            _ => {}
        }
        Ok(())
    }
}

fn handle_start(
    ctx: &mut ReducerCtx<FetchState, ()>,
    url: String,
    method: String,
) -> Result<(), ReduceError> {
    if matches!(ctx.state.pc, FetchPc::Fetching) {
        return Ok(());
    }
    let request_id = ctx.state.next_request_id;
    ctx.state.next_request_id = ctx.state.next_request_id.saturating_add(1);
    ctx.state.pending_request = Some(request_id);
    ctx.state.pc = FetchPc::Fetching;
    ctx.state.last_status = None;
    ctx.state.last_body_preview = None;

    let params = HttpRequestParams {
        method,
        url,
        headers: BTreeMap::new(),
        body_ref: None,
    };
    ctx.effects()
        .emit_raw(HTTP_REQUEST_EFFECT, &params, Some("default"));
    Ok(())
}

fn handle_receipt(
    ctx: &mut ReducerCtx<FetchState, ()>,
    envelope: EffectReceiptEnvelope,
) -> Result<(), ReduceError> {
    if ctx.state.pending_request.is_none() {
        return Ok(());
    }
    if envelope.effect_kind != HTTP_REQUEST_EFFECT {
        return Ok(());
    }
    let receipt = decode_http_receipt_payload(&envelope.receipt_payload).unwrap_or(
        HttpRequestReceiptPayload {
            status: 0,
            body_preview: None,
        },
    );
    ctx.state.pending_request = None;
    ctx.state.pc = FetchPc::Done;
    ctx.state.last_status = Some(receipt.status);
    ctx.state.last_body_preview = receipt.body_preview;
    Ok(())
}

fn decode_http_receipt_payload(payload: &[u8]) -> Result<HttpRequestReceiptPayload, ReduceError> {
    let value: CborValue =
        serde_cbor::from_slice(payload).map_err(|_| ReduceError::new("invalid http.request receipt payload"))?;
    let status = extract_status(&value).ok_or(ReduceError::new("missing http status"))?;
    let body_preview = extract_body_preview(&value);
    Ok(HttpRequestReceiptPayload {
        status,
        body_preview,
    })
}

fn root_record(value: &CborValue) -> Option<&BTreeMap<CborValue, CborValue>> {
    let CborValue::Map(map) = value else {
        return None;
    };
    if let Some(record) = map.get(&CborValue::Text("Record".into())) {
        let CborValue::Map(record_map) = record else {
            return None;
        };
        return Some(record_map);
    }
    Some(map)
}

fn map_get_text<'a>(
    map: &'a BTreeMap<CborValue, CborValue>,
    key: &str,
) -> Option<&'a CborValue> {
    map.get(&CborValue::Text(key.into()))
}

fn decode_value_int(value: &CborValue) -> Option<i64> {
    match value {
        CborValue::Integer(i) => i64::try_from(*i).ok(),
        CborValue::Map(map) => {
            let wrapped = map_get_text(map, "Int").or_else(|| map_get_text(map, "Nat"))?;
            match wrapped {
                CborValue::Integer(i) => i64::try_from(*i).ok(),
                _ => None,
            }
        }
        _ => None,
    }
}

fn decode_value_text(value: &CborValue) -> Option<String> {
    match value {
        CborValue::Text(text) => Some(text.clone()),
        CborValue::Map(map) => {
            let wrapped = map_get_text(map, "Text")?;
            match wrapped {
                CborValue::Text(text) => Some(text.clone()),
                _ => None,
            }
        }
        _ => None,
    }
}

fn extract_status(value: &CborValue) -> Option<i64> {
    let root = root_record(value)?;
    let status_value = map_get_text(root, "status")?;
    decode_value_int(status_value)
}

fn extract_body_preview(value: &CborValue) -> Option<String> {
    let root = root_record(value)?;
    let preview = map_get_text(root, "body_preview")?;
    decode_value_text(preview)
}

fn decode_tagged_event(value: CborValue) -> Option<(String, CborValue)> {
    let CborValue::Map(map) = value else {
        return None;
    };
    let tag = map_get_text(&map, "$tag")?;
    let tag = decode_value_text(tag)?;
    let payload = map
        .get(&CborValue::Text("$value".into()))
        .cloned()
        .unwrap_or(CborValue::Null);
    Some((tag, payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_wasm_abi::{ABI_VERSION, DomainEvent, ReducerContext, ReducerInput, ReducerOutput};
    use aos_wasm_sdk::step_bytes;
    use alloc::string::ToString;
    use alloc::vec;

    fn context_bytes() -> Vec<u8> {
        let ctx = ReducerContext {
            now_ns: 1,
            logical_now_ns: 2,
            journal_height: 3,
            entropy: vec![0x11; 64],
            event_hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            manifest_hash:
                "sha256:1111111111111111111111111111111111111111111111111111111111111111".into(),
            reducer: "demo/FetchNotify@1".into(),
            key: None,
            cell_mode: false,
        };
        serde_cbor::to_vec(&ctx).expect("context bytes")
    }

    fn step_with(state: Option<Vec<u8>>, event: CborValue) -> Result<ReducerOutput, String> {
        let input = ReducerInput {
            version: ABI_VERSION,
            state,
            event: DomainEvent::new(
                "demo/FetchNotifyEvent@1",
                serde_cbor::to_vec(&event).expect("event bytes"),
            ),
            ctx: Some(context_bytes()),
        };
        let input_bytes = input.encode().expect("encode input");
        let output_bytes = step_bytes::<FetchNotifySm>(&input_bytes).map_err(|e| e.to_string())?;
        ReducerOutput::decode(&output_bytes).map_err(|e| e.to_string())
    }

    fn tagged(tag: &str, payload: CborValue) -> CborValue {
        CborValue::Map(BTreeMap::from([
            (CborValue::Text("$tag".into()), CborValue::Text(tag.into())),
            (CborValue::Text("$value".into()), payload),
        ]))
    }

    #[test]
    fn start_then_receipt_round_trip() {
        let start_payload = CborValue::Map(BTreeMap::from([
            (
                CborValue::Text("url".into()),
                CborValue::Text("https://example.com/data.json".into()),
            ),
            (
                CborValue::Text("method".into()),
                CborValue::Text("GET".into()),
            ),
        ]));
        let start_output = step_with(None, tagged("Start", start_payload)).expect("start step");
        assert_eq!(start_output.effects.len(), 1);
        let state_after_start = start_output.state.expect("state after start");

        let receipt_payload = serde_cbor::to_vec(&CborValue::Map(BTreeMap::from([
            (
                CborValue::Text("status".into()),
                CborValue::Integer(200.into()),
            ),
            (
                CborValue::Text("body_preview".into()),
                CborValue::Text("{\"ok\":true}".into()),
            ),
        ])))
        .expect("receipt payload");
        let receipt_envelope = CborValue::Map(BTreeMap::from([
            (
                CborValue::Text("origin_module_id".into()),
                CborValue::Text("demo/FetchNotify@1".into()),
            ),
            (CborValue::Text("origin_instance_key".into()), CborValue::Null),
            (
                CborValue::Text("intent_id".into()),
                CborValue::Text(
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                ),
            ),
            (
                CborValue::Text("effect_kind".into()),
                CborValue::Text("http.request".into()),
            ),
            (CborValue::Text("params_hash".into()), CborValue::Null),
            (
                CborValue::Text("receipt_payload".into()),
                CborValue::Bytes(receipt_payload),
            ),
            (CborValue::Text("status".into()), CborValue::Text("ok".into())),
            (
                CborValue::Text("emitted_at_seq".into()),
                CborValue::Integer(1.into()),
            ),
            (
                CborValue::Text("adapter_id".into()),
                CborValue::Text("http.mock".into()),
            ),
            (CborValue::Text("cost_cents".into()), CborValue::Null),
            (
                CborValue::Text("signature".into()),
                CborValue::Bytes(vec![0; 64]),
            ),
        ]));
        let receipt_output = step_with(
            Some(state_after_start),
            tagged("Receipt", receipt_envelope),
        )
        .expect("receipt step");
        let state_bytes = receipt_output.state.expect("state after receipt");
        let state: FetchState = serde_cbor::from_slice(&state_bytes).expect("decode state");
        assert!(matches!(state.pc, FetchPc::Done));
        assert_eq!(state.last_status, Some(200));
        assert_eq!(state.last_body_preview.as_deref(), Some("{\"ok\":true}"));
    }
}
