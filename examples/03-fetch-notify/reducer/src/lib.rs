#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::String;
use aos_air_exec::Value as AirValue;
use aos_wasm_sdk::{aos_reducer, ReduceError, Reducer, ReducerCtx, Value};
use serde::de::Error as _;
use serde::{Deserialize, Serialize};

const EVENT_SCHEMA: &str = "demo/FetchNotifyEvent@1";
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
    type Event = Value;
    type Ann = Value;

    fn reduce(
        &mut self,
        event_value: Self::Event,
        ctx: &mut ReducerCtx<Self::State>,
    ) -> Result<(), ReduceError> {
        if let Some(event) = decode_event(event_value) {
            match event {
                FetchEvent::Start { url, method } => handle_start(ctx, url, method),
                FetchEvent::NotifyComplete { status, body_preview } => {
                    handle_notify(ctx, status, body_preview)
                }
            }
        }
        Ok(())
    }
}

fn handle_start(ctx: &mut ReducerCtx<FetchState>, url: String, method: String) {
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

fn handle_notify(ctx: &mut ReducerCtx<FetchState>, status: i64, body_preview: String) {
    if ctx.state.pending_request.is_none() {
        return;
    }
    ctx.state.pending_request = None;
    ctx.state.pc = FetchPc::Done;
    ctx.state.last_status = Some(status);
    ctx.state.last_body_preview = Some(body_preview);
}

fn decode_event(value: Value) -> Option<FetchEvent> {
    let bytes = serde_cbor::to_vec(&value).ok()?;
    decode_event_bytes(&bytes).ok()
}

fn decode_event_bytes(bytes: &[u8]) -> Result<FetchEvent, serde_cbor::Error> {
    match serde_cbor::from_slice::<FetchEvent>(bytes) {
        Ok(event) => Ok(event),
        Err(_) => {
            let value: serde_cbor::Value = serde_cbor::from_slice(bytes)?;
            if let serde_cbor::Value::Map(mut map) = value {
                if let (Some(tag), Some(body)) = (
                    map.remove(&serde_cbor::Value::Text("$tag".into())),
                    map.remove(&serde_cbor::Value::Text("$value".into())),
                ) {
                    if let serde_cbor::Value::Text(name) = tag {
                        match name.as_str() {
                            "NotifyComplete" => {
                                if let serde_cbor::Value::Map(mut inner) = body {
                                    let status = match inner.remove(
                                        &serde_cbor::Value::Text("status".into()),
                                    ) {
                                        Some(serde_cbor::Value::Integer(i)) => {
                                            i64::try_from(i).unwrap_or_default()
                                        }
                                        _ => 0,
                                    };
                                    let preview = match inner.remove(
                                        &serde_cbor::Value::Text("body_preview".into()),
                                    ) {
                                        Some(serde_cbor::Value::Text(text)) => text,
                                        _ => String::new(),
                                    };
                                    return Ok(FetchEvent::NotifyComplete {
                                        status,
                                        body_preview: preview,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Err(serde_cbor::Error::custom("unsupported event variant"))
        }
    }
}
