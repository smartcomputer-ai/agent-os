#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::{ReduceError, Reducer, ReducerCtx, aos_reducer, aos_variant};
use serde::{Deserialize, Serialize};

const HTTP_REQUEST_EFFECT: &str = "http.request";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FetchState {
    pc: FetchPc,
    next_request_id: u64,
    pending_request: Option<u64>,
    last_status: Option<i64>,
    last_body_ref: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none", with = "serde_bytes_opt")]
    origin_instance_key: Option<Vec<u8>>,
    intent_id: String,
    effect_kind: String,
    params_hash: Option<String>,
    #[serde(with = "serde_bytes")]
    receipt_payload: Vec<u8>,
    status: String,
    emitted_at_seq: u64,
    adapter_id: String,
    cost_cents: Option<u64>,
    #[serde(with = "serde_bytes")]
    signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HttpRequestParams {
    method: String,
    url: String,
    headers: BTreeMap<String, String>,
    body_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RequestTimings {
    start_ns: u64,
    end_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HttpRequestReceipt {
    status: i32,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    body_ref: Option<String>,
    timings: RequestTimings,
    adapter_id: String,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum FetchEvent {
        Start(StartEvent),
        Receipt(EffectReceiptEnvelope)
    }
}

aos_reducer!(FetchNotifySm);

#[derive(Default)]
struct FetchNotifySm;

impl Reducer for FetchNotifySm {
    type State = FetchState;
    type Event = FetchEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        match event {
            FetchEvent::Start(start) => handle_start(ctx, start.url, start.method)?,
            FetchEvent::Receipt(envelope) => handle_receipt(ctx, envelope)?,
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
    ctx.state.last_body_ref = None;

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

    let receipt: HttpRequestReceipt = serde_cbor::from_slice(&envelope.receipt_payload)
        .map_err(|_| ReduceError::new("invalid http.request receipt payload"))?;

    ctx.state.pending_request = None;
    ctx.state.pc = FetchPc::Done;
    ctx.state.last_status = Some(receipt.status as i64);
    ctx.state.last_body_ref = receipt.body_ref;
    Ok(())
}

mod serde_bytes_opt {
    use alloc::vec::Vec;
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_bytes::{ByteBuf, Bytes};

    pub fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(bytes) => serializer.serialize_some(Bytes::new(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<ByteBuf>::deserialize(deserializer).map(|opt| opt.map(|buf| buf.into_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_wasm_abi::{ABI_VERSION, DomainEvent, ReducerContext, ReducerInput, ReducerOutput};
    use aos_wasm_sdk::step_bytes;
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

    fn step_with(state: Option<Vec<u8>>, event: &FetchEvent) -> ReducerOutput {
        let input = ReducerInput {
            version: ABI_VERSION,
            state,
            event: DomainEvent::new(
                "demo/FetchNotifyEvent@1",
                serde_cbor::to_vec(event).expect("event bytes"),
            ),
            ctx: Some(context_bytes()),
        };
        let input_bytes = input.encode().expect("encode input");
        let output_bytes = step_bytes::<FetchNotifySm>(&input_bytes).expect("step");
        ReducerOutput::decode(&output_bytes).expect("decode output")
    }

    #[test]
    fn start_then_receipt_round_trip() {
        let start = FetchEvent::Start(StartEvent {
            url: "https://example.com/data.json".into(),
            method: "GET".into(),
        });
        let start_out = step_with(None, &start);
        assert_eq!(start_out.effects.len(), 1);
        let state_after_start = start_out.state.expect("state");

        let receipt_payload = HttpRequestReceipt {
            status: 200,
            headers: BTreeMap::new(),
            body_ref: Some(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            ),
            timings: RequestTimings {
                start_ns: 10,
                end_ns: 20,
            },
            adapter_id: "http.mock".into(),
        };
        let receipt = FetchEvent::Receipt(EffectReceiptEnvelope {
            origin_module_id: "demo/FetchNotify@1".into(),
            origin_instance_key: None,
            intent_id: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .into(),
            effect_kind: "http.request".into(),
            params_hash: None,
            receipt_payload: serde_cbor::to_vec(&receipt_payload).expect("payload"),
            status: "ok".into(),
            emitted_at_seq: 1,
            adapter_id: "http.mock".into(),
            cost_cents: Some(0),
            signature: vec![0; 64],
        });
        let receipt_out = step_with(Some(state_after_start), &receipt);
        let state_bytes = receipt_out.state.expect("state after receipt");
        let state: FetchState = serde_cbor::from_slice(&state_bytes).expect("decode state");
        assert!(matches!(state.pc, FetchPc::Done));
        assert_eq!(state.last_status, Some(200));
        assert!(state.last_body_ref.is_some());
    }
}
