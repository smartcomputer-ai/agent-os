#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::String;
use aos_air_exec::Value as AirValue;
use aos_wasm_sdk::{aos_reducer, ReduceError, Reducer, ReducerCtx};
use serde::{Deserialize, Serialize};

const FETCH_REQUEST_SCHEMA: &str = "demo/FetchRequest@1";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FetchState {
    pc: FetchPc,
    next_request_id: u64,
    pending_request: Option<u64>,
    last_status: Option<i64>,
    last_body_preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum FetchPc {
    Idle,
    Fetching,
    Done,
}

impl Default for FetchPc {
    fn default() -> Self {
        FetchPc::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum FetchEvent {
    Start { url: String, method: String },
    NotifyComplete { status: i64, body_preview: String },
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
            FetchEvent::Start { url, method } => handle_start(ctx, url, method),
            FetchEvent::NotifyComplete { status, body_preview } => {
                handle_notify(ctx, status, body_preview)
            }
        }
        Ok(())
    }
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
    ctx.state.last_body_preview = None;

    let intent_value = AirValue::record([
        ("request_id", AirValue::Nat(request_id)),
        ("url", AirValue::Text(url)),
        ("method", AirValue::Text(method)),
    ]);
    let key = request_id.to_be_bytes();
    ctx.intent(FETCH_REQUEST_SCHEMA)
        .key_bytes(&key)
        .payload(&intent_value)
        .send();
}

fn handle_notify(ctx: &mut ReducerCtx<FetchState, ()>, status: i64, body_preview: String) {
    if ctx.state.pending_request.is_none() {
        return;
    }
    ctx.state.pending_request = None;
    ctx.state.pc = FetchPc::Done;
    ctx.state.last_status = Some(status);
    ctx.state.last_body_preview = Some(body_preview);
}
