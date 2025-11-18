#![allow(improper_ctypes_definitions)]

use aos_air_exec::Value;
use aos_wasm_abi::{DomainEvent, ReducerInput, ReducerOutput};
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use std::alloc::{alloc as host_alloc, Layout};
use std::slice;

const EVENT_SCHEMA: &str = "demo/AggregatorEvent@1";
const AGGREGATE_REQUEST_SCHEMA: &str = "demo/AggregateRequested@1";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AggregatorState {
    pc: AggregatorPc,
    next_request_id: u64,
    pending_request: Option<u64>,
    current_topic: Option<String>,
    last_statuses: Vec<i64>,
    last_previews: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum AggregatorPc {
    Idle,
    Running,
    Done,
}

impl Default for AggregatorPc {
    fn default() -> Self {
        AggregatorPc::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum AggregatorEvent {
    Start { topic: String },
    AggregateComplete {
        request_id: u64,
        topic: String,
        status_a: i64,
        status_b: i64,
        status_c: i64,
        body_a: String,
        body_b: String,
        body_c: String,
    },
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
        .map(|bytes| serde_cbor::from_slice::<AggregatorState>(&bytes).expect("state"))
        .unwrap_or_default();

    let mut domain_events = Vec::new();
    if input.event.schema == EVENT_SCHEMA {
        if let Ok(event) = decode_event(&input.event.value) {
            match event {
                AggregatorEvent::Start { topic } => handle_start(&mut state, topic, &mut domain_events),
                AggregatorEvent::AggregateComplete {
                    request_id,
                    topic,
                    status_a,
                    status_b,
                    status_c,
                    body_a,
                    body_b,
                    body_c,
                } => handle_complete(
                    &mut state,
                    request_id,
                    topic,
                    [status_a, status_b, status_c],
                    [body_a, body_b, body_c],
                ),
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

fn handle_start(state: &mut AggregatorState, topic: String, domain_events: &mut Vec<DomainEvent>) {
    if matches!(state.pc, AggregatorPc::Running) {
        return;
    }
    let request_id = state.next_request_id;
    state.next_request_id = state.next_request_id.saturating_add(1);
    state.pending_request = Some(request_id);
    state.current_topic = Some(topic.clone());
    state.pc = AggregatorPc::Running;
    state.last_statuses.clear();
    state.last_previews.clear();

    let intent_value = Value::record([
        ("request_id", Value::Nat(request_id)),
        ("topic", Value::Text(topic)),
    ]);
    let value = serde_cbor::to_vec(&intent_value).expect("intent");
    let key = request_id.to_be_bytes().to_vec();
    domain_events.push(DomainEvent::with_key(
        AGGREGATE_REQUEST_SCHEMA,
        value,
        key,
    ));
}

fn handle_complete(
    state: &mut AggregatorState,
    request_id: u64,
    topic: String,
    statuses: [i64; 3],
    bodies: [String; 3],
) {
    if !matches!(state.pending_request, Some(id) if id == request_id) {
        return;
    }
    state.pending_request = None;
    state.pc = AggregatorPc::Done;
    state.current_topic = Some(topic);
    state.last_statuses = statuses.to_vec();
    state.last_previews = bodies.into();
}

fn decode_event(bytes: &[u8]) -> Result<AggregatorEvent, serde_cbor::Error> {
    match serde_cbor::from_slice::<AggregatorEvent>(bytes) {
        Ok(event) => Ok(event),
        Err(_) => {
            let value: serde_cbor::Value = serde_cbor::from_slice(bytes)?;
            if let serde_cbor::Value::Map(mut map) = value {
                let tag_key = serde_cbor::Value::Text("$tag".into());
                let value_key = serde_cbor::Value::Text("$value".into());
                if let (Some(tag), Some(body)) = (map.remove(&tag_key), map.remove(&value_key)) {
                    if let serde_cbor::Value::Text(name) = tag {
                        if name == "AggregateComplete" {
                            return parse_complete(body);
                        }
                    }
                }
            }
            Err(serde_cbor::Error::custom("unsupported event variant"))
        }
    }
}

fn parse_complete(body: serde_cbor::Value) -> Result<AggregatorEvent, serde_cbor::Error> {
    if let serde_cbor::Value::Map(mut map) = body {
        let request_id = match map.remove(&serde_cbor::Value::Text("request_id".into())) {
            Some(serde_cbor::Value::Integer(i)) => i as u64,
            Some(serde_cbor::Value::Unsigned(u)) => u,
            _ => 0,
        };
        let topic = match map.remove(&serde_cbor::Value::Text("topic".into())) {
            Some(serde_cbor::Value::Text(text)) => text,
            _ => String::new(),
        };
        let status_a = extract_int(&mut map, "status_a");
        let status_b = extract_int(&mut map, "status_b");
        let status_c = extract_int(&mut map, "status_c");
        let body_a = extract_text(&mut map, "body_a");
        let body_b = extract_text(&mut map, "body_b");
        let body_c = extract_text(&mut map, "body_c");
        return Ok(AggregatorEvent::AggregateComplete {
            request_id,
            topic,
            status_a,
            status_b,
            status_c,
            body_a,
            body_b,
            body_c,
        });
    }
    Err(serde_cbor::Error::custom("invalid AggregateComplete body"))
}

fn extract_int(map: &mut serde_cbor::Map<serde_cbor::Value, serde_cbor::Value>, key: &str) -> i64 {
    match map.remove(&serde_cbor::Value::Text(key.into())) {
        Some(serde_cbor::Value::Integer(i)) => i,
        Some(serde_cbor::Value::Unsigned(u)) => u as i64,
        _ => 0,
    }
}

fn extract_text(map: &mut serde_cbor::Map<serde_cbor::Value, serde_cbor::Value>, key: &str) -> String {
    match map.remove(&serde_cbor::Value::Text(key.into())) {
        Some(serde_cbor::Value::Text(text)) => text,
        _ => String::new(),
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
