#![allow(improper_ctypes_definitions)]

use aos_air_exec::Value;
use aos_wasm_abi::{DomainEvent, ReducerInput, ReducerOutput};
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use std::alloc::{alloc as host_alloc, Layout};
use std::slice;

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

#[cfg_attr(target_arch = "wasm32", unsafe(export_name = "alloc"))]
pub extern "C" fn wasm_alloc(len: i32) -> i32 {
    if len <= 0 {
        return 0;
    }
    let layout = Layout::from_size_align(len as usize, 8).expect("layout");
    unsafe { host_alloc(layout) as i32 }
}

#[cfg_attr(target_arch = "wasm32", unsafe(export_name = "step"))]
pub extern "C" fn wasm_step(ptr: i32, len: i32) -> (i32, i32) {
    let input_bytes = unsafe { slice::from_raw_parts(ptr as *const u8, len as usize) };
    let input = ReducerInput::decode(input_bytes).expect("valid reducer input");

    let mut state = input
        .state
        .map(|bytes| serde_cbor::from_slice::<FetchState>(&bytes).expect("state"))
        .unwrap_or_default();

    let mut domain_events = Vec::new();
    if input.event.schema == EVENT_SCHEMA {
        if let Ok(event) = decode_event(&input.event.value) {
            match event {
                FetchEvent::Start { url, method } => {
                    handle_start(&mut state, url, method, &mut domain_events)
                }
                FetchEvent::NotifyComplete { status, body_preview } => {
                    handle_notify(&mut state, status, body_preview)
                }
            }
        }
    }

    let state_bytes = serde_cbor::to_vec(&state).expect("encode state");
    let output = ReducerOutput {
        state: Some(state_bytes),
        domain_events,
        effects: Vec::new(),
        ann: None,
    };
    let output_bytes = output.encode().expect("encode output");
    write_back(&output_bytes)
}

fn handle_start(
    state: &mut FetchState,
    url: String,
    method: String,
    domain_events: &mut Vec<DomainEvent>,
) {
    if matches!(state.pc, FetchPc::Fetching) {
        return;
    }
    let request_id = state.next_request_id;
    state.next_request_id = state.next_request_id.saturating_add(1);
    state.pending_request = Some(request_id);
    state.pc = FetchPc::Fetching;
    state.last_status = None;
    state.last_body_preview = None;

    let intent_value = Value::record([
        ("request_id", Value::Nat(request_id)),
        ("url", Value::Text(url)),
        ("method", Value::Text(method)),
    ]);
    let value = serde_cbor::to_vec(&intent_value).expect("intent");
    let key = request_id.to_be_bytes().to_vec();
    domain_events.push(DomainEvent::with_key(
        FETCH_REQUEST_SCHEMA,
        value,
        key,
    ));
}

fn handle_notify(state: &mut FetchState, status: i64, body_preview: String) {
    if state.pending_request.is_none() {
        return;
    }
    state.pending_request = None;
    state.pc = FetchPc::Done;
    state.last_status = Some(status);
    state.last_body_preview = Some(body_preview);
}

fn decode_event(bytes: &[u8]) -> Result<FetchEvent, serde_cbor::Error> {
    match serde_cbor::from_slice::<FetchEvent>(bytes) {
        Ok(event) => Ok(event),
        Err(_) => {
            let value: serde_cbor::Value = serde_cbor::from_slice(bytes)?;
            if let serde_cbor::Value::Map(mut map) = value {
                if let (Some(tag), Some(body)) = (map.remove(&serde_cbor::Value::Text("$tag".into())), map.remove(&serde_cbor::Value::Text("$value".into()))) {
                    if let serde_cbor::Value::Text(name) = tag {
                        match name.as_str() {
                            "NotifyComplete" => {
                                if let serde_cbor::Value::Map(mut inner) = body {
                                    let status = match inner.remove(&serde_cbor::Value::Text("status".into())) {
                                        Some(serde_cbor::Value::Integer(i)) => i as i64,
                                        _ => 0,
                                    };
                                    let preview = match inner.remove(&serde_cbor::Value::Text("body_preview".into())) {
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

fn write_back(bytes: &[u8]) -> (i32, i32) {
    let len = bytes.len() as i32;
    let ptr = wasm_alloc(len);
    unsafe {
        let out = slice::from_raw_parts_mut(ptr as *mut u8, len as usize);
        out.copy_from_slice(bytes);
    }
    (ptr, len)
}
