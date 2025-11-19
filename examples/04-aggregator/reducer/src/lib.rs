#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::{format, string::String, vec, vec::Vec};
use aos_air_exec::Value;
use aos_wasm_sdk::{aos_reducer, ReduceError, Reducer, ReducerCtx, Value as CborValue};
use indexmap::IndexMap;
use serde::de::Error as _;
use serde::{Deserialize, Serialize};

const EVENT_SCHEMA: &str = "demo/AggregatorEvent@1";
const AGGREGATE_REQUEST_SCHEMA: &str = "demo/AggregateRequested@1";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AggregatorState {
    pc: AggregatorPc,
    next_request_id: u64,
    pending_request: Option<u64>,
    current_topic: Option<String>,
    pending_targets: Vec<String>,
    last_responses: Vec<AggregateResponse>,
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
struct AggregationTarget {
    name: String,
    url: String,
    method: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AggregateResponse {
    source: String,
    status: i64,
    body_preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum AggregatorEvent {
    Start {
        topic: String,
        primary: AggregationTarget,
        secondary: AggregationTarget,
        tertiary: AggregationTarget,
    },
    AggregateComplete {
        request_id: u64,
        topic: String,
        primary: AggregateResponse,
        secondary: AggregateResponse,
        tertiary: AggregateResponse,
    },
}

aos_reducer!(AggregatorSm);

#[derive(Default)]
struct AggregatorSm;

impl Reducer for AggregatorSm {
    type State = AggregatorState;
    type Event = CborValue;
    type Ann = Value;

    fn reduce(
        &mut self,
        event_value: Self::Event,
        ctx: &mut ReducerCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        if let Some(event) = decode_event(event_value) {
            match event {
                AggregatorEvent::Start {
                    topic,
                    primary,
                    secondary,
                    tertiary,
                } => handle_start(ctx, topic, primary, secondary, tertiary),
                AggregatorEvent::AggregateComplete {
                    request_id,
                    topic,
                    primary,
                    secondary,
                    tertiary,
                } => handle_complete(ctx, request_id, topic, [primary, secondary, tertiary]),
            }
        }
        Ok(())
    }
}

fn handle_start(
    ctx: &mut ReducerCtx<AggregatorState, Value>,
    topic: String,
    primary: AggregationTarget,
    secondary: AggregationTarget,
    tertiary: AggregationTarget,
) {
    if matches!(ctx.state.pc, AggregatorPc::Running) {
        return;
    }
    let request_id = ctx.state.next_request_id;
    ctx.state.next_request_id = ctx.state.next_request_id.saturating_add(1);
    ctx.state.pending_request = Some(request_id);
    ctx.state.current_topic = Some(topic.clone());
    ctx.state.pc = AggregatorPc::Running;
    ctx.state.pending_targets = vec![
        primary.name.clone(),
        secondary.name.clone(),
        tertiary.name.clone(),
    ];
    ctx.state.last_responses.clear();

    let intent_value = Value::record([
        ("request_id", Value::Nat(request_id)),
        ("topic", Value::Text(topic)),
        ("primary", target_to_value(&primary)),
        ("secondary", target_to_value(&secondary)),
        ("tertiary", target_to_value(&tertiary)),
    ]);
    let key = request_id.to_be_bytes();
    ctx.intent(AGGREGATE_REQUEST_SCHEMA)
        .key_bytes(&key)
        .payload(&intent_value)
        .send();
}

fn handle_complete(
    ctx: &mut ReducerCtx<AggregatorState, Value>,
    request_id: u64,
    topic: String,
    responses: [AggregateResponse; 3],
) {
    if !matches!(ctx.state.pending_request, Some(id) if id == request_id) {
        return;
    }
    ctx.state.pending_request = None;
    ctx.state.pc = AggregatorPc::Done;
    ctx.state.current_topic = Some(topic);
    ctx.state.pending_targets.clear();
    ctx.state.last_responses = responses.to_vec();
}

fn decode_event(value: CborValue) -> Option<AggregatorEvent> {
    let bytes = serde_cbor::to_vec(&value).ok()?;
    decode_event_bytes(&bytes).ok()
}

fn decode_event_bytes(bytes: &[u8]) -> Result<AggregatorEvent, serde_cbor::Error> {
    if let Ok(event) = serde_cbor::from_slice::<AggregatorEvent>(bytes) {
        return Ok(event);
    }
    let value: Value = serde_cbor::from_slice(bytes)?;
    match value {
        Value::Record(mut record) => {
            if let (Some(Value::Text(tag)), Some(body)) =
                (record.swap_remove("$tag"), record.swap_remove("$value"))
            {
                return parse_variant(tag, body);
            }
        }
        _ => {}
    }
    Err(serde_cbor::Error::custom("unsupported event variant"))
}

fn parse_variant(tag: String, body: Value) -> Result<AggregatorEvent, serde_cbor::Error> {
    match tag.as_str() {
        "Start" => parse_start_value(body),
        "AggregateComplete" => parse_complete_value(body),
        other => Err(serde_cbor::Error::custom(format!("unknown event tag {other}"))),
    }
}

fn parse_start_value(body: Value) -> Result<AggregatorEvent, serde_cbor::Error> {
    if let Value::Record(mut record) = body {
        let topic = extract_text_value(&mut record, "topic");
        let primary = extract_target_value(&mut record, "primary");
        let secondary = extract_target_value(&mut record, "secondary");
        let tertiary = extract_target_value(&mut record, "tertiary");
        return Ok(AggregatorEvent::Start {
            topic,
            primary,
            secondary,
            tertiary,
        });
    }
    Err(serde_cbor::Error::custom("Start body must be record"))
}

fn parse_complete_value(body: Value) -> Result<AggregatorEvent, serde_cbor::Error> {
    if let Value::Record(mut record) = body {
        let request_id = extract_nat_value(&mut record, "request_id");
        let topic = extract_text_value(&mut record, "topic");
        let primary = extract_response_value(&mut record, "primary");
        let secondary = extract_response_value(&mut record, "secondary");
        let tertiary = extract_response_value(&mut record, "tertiary");
        return Ok(AggregatorEvent::AggregateComplete {
            request_id,
            topic,
            primary,
            secondary,
            tertiary,
        });
    }
    Err(serde_cbor::Error::custom("invalid AggregateComplete body"))
}

fn target_to_value(target: &AggregationTarget) -> Value {
    Value::record([
        ("name", Value::Text(target.name.clone())),
        ("url", Value::Text(target.url.clone())),
        ("method", Value::Text(target.method.clone())),
    ])
}

fn extract_target_value(record: &mut IndexMap<String, Value>, key: &str) -> AggregationTarget {
    if let Some(Value::Record(mut target)) = record.swap_remove(key) {
        return AggregationTarget {
            name: extract_text_value(&mut target, "name"),
            url: extract_text_value(&mut target, "url"),
            method: extract_text_value(&mut target, "method"),
        };
    }
    AggregationTarget {
        name: String::new(),
        url: String::new(),
        method: String::new(),
    }
}

fn extract_response_value(record: &mut IndexMap<String, Value>, key: &str) -> AggregateResponse {
    if let Some(Value::Record(mut response)) = record.swap_remove(key) {
        return AggregateResponse {
            source: extract_text_value(&mut response, "source"),
            status: extract_int_value(&mut response, "status"),
            body_preview: extract_text_value(&mut response, "body_preview"),
        };
    }
    AggregateResponse {
        source: String::new(),
        status: 0,
        body_preview: String::new(),
    }
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
