#![allow(improper_ctypes_definitions)]

use aos_air_exec::Value;
use aos_wasm_abi::{DomainEvent, ReducerInput, ReducerOutput};
use indexmap::IndexMap;
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
    if let Ok(event) = serde_cbor::from_slice::<AggregatorEvent>(bytes) {
        return Ok(event);
    }
    let value: Value = serde_cbor::from_slice(bytes)?;
    match value {
        Value::Record(mut record) => {
            if let (Some(Value::Text(tag)), Some(body)) = (record.swap_remove("$tag"), record.swap_remove("$value")) {
                return parse_variant(tag, body);
            }
        }
        _ => {}
    }
    Err(serde_cbor::Error::custom("unsupported event variant"))
}

fn parse_variant(tag: String, body: Value) -> Result<AggregatorEvent, serde_cbor::Error> {
    match tag.as_str() {
        "Start" => {
            if let Value::Record(mut record) = body {
                let topic = extract_text_value(&mut record, "topic");
                Ok(AggregatorEvent::Start { topic })
            } else {
                Err(serde_cbor::Error::custom("Start body must be record"))
            }
        }
        "AggregateComplete" => parse_complete_value(body),
        other => Err(serde_cbor::Error::custom(format!("unknown event tag {other}"))),
    }
}

fn parse_complete_value(body: Value) -> Result<AggregatorEvent, serde_cbor::Error> {
    if let Value::Record(mut record) = body {
        let request_id = extract_nat_value(&mut record, "request_id");
        let topic = extract_text_value(&mut record, "topic");
        let status_a = extract_int_value(&mut record, "status_a");
        let status_b = extract_int_value(&mut record, "status_b");
        let status_c = extract_int_value(&mut record, "status_c");
        let body_a = extract_text_value(&mut record, "body_a");
        let body_b = extract_text_value(&mut record, "body_b");
        let body_c = extract_text_value(&mut record, "body_c");
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

fn extract_int_value(record: &mut IndexMap<String, Value>, key: &str) -> i64 {
    match record.swap_remove(key) {
        Some(Value::Int(v)) => v,
        Some(Value::Nat(v)) => v as i64,
        _ => 0,
    }
}

fn extract_nat_value(record: &mut IndexMap<String, Value>, key: &str) -> u64 {
    match record.swap_remove(key) {
        Some(Value::Nat(v)) => v,
        Some(Value::Int(v)) if v >= 0 => v as u64,
        _ => 0,
    }
}

fn extract_text_value(record: &mut IndexMap<String, Value>, key: &str) -> String {
    match record.swap_remove(key) {
        Some(Value::Text(text)) => text,
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
