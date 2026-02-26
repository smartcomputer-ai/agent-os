#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::{collections::BTreeMap, string::String};
use aos_wasm_sdk::{
    EffectReceiptEnvelope, ReduceError, Reducer, ReducerCtx, aos_reducer, aos_variant,
};
use serde::{Deserialize, Serialize};

const HTTP_REQUEST_EFFECT: &str = "http.request";

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
            FetchEvent::Start { url, method } => handle_start(ctx, url, method),
            FetchEvent::Receipt(envelope) => handle_receipt(ctx, envelope)?,
        }
        Ok(())
    }
}

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

aos_variant! {
#[derive(Debug, Clone, Serialize, Deserialize)]
enum FetchEvent {
    Start { url: String, method: String },
    Receipt(EffectReceiptEnvelope),
}
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

fn handle_start(ctx: &mut ReducerCtx<FetchState, ()>, url: String, method: String) {
    if matches!(ctx.state.pc, FetchPc::Fetching) {
        return;
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

    match envelope.status.as_str() {
        "ok" => {
            let receipt: HttpRequestReceipt = envelope
                .decode_receipt_payload()
                .map_err(|_| ReduceError::new("invalid http.request receipt payload"))?;
            ctx.state.last_status = Some(receipt.status as i64);
            ctx.state.last_body_ref = receipt.body_ref;
        }
        "timeout" => {
            ctx.state.last_status = Some(-2);
            ctx.state.last_body_ref = None;
        }
        "error" => {
            ctx.state.last_status = Some(-1);
            ctx.state.last_body_ref = None;
        }
        _ => {
            return Err(ReduceError::new("unknown receipt status for http.request"));
        }
    }

    ctx.state.pending_request = None;
    ctx.state.pc = FetchPc::Done;
    Ok(())
}
