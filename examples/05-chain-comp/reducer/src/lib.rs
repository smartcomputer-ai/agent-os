#![allow(improper_ctypes_definitions)]

use aos_air_exec::{Value, ValueKey};
use aos_wasm_abi::{DomainEvent, ReducerInput, ReducerOutput};
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use std::alloc::{alloc as host_alloc, Layout};
use std::collections::BTreeMap;
use std::slice;

const EVENT_SCHEMA: &str = "demo/ChainEvent@1";
const CHARGE_REQUEST_SCHEMA: &str = "demo/ChargeRequested@1";
const RESERVE_REQUEST_SCHEMA: &str = "demo/ReserveRequested@1";
const NOTIFY_REQUEST_SCHEMA: &str = "demo/NotifyRequested@1";
const REFUND_REQUEST_SCHEMA: &str = "demo/RefundRequested@1";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ChainState {
    phase: ChainPhase,
    next_request_id: u64,
    current_saga: Option<SagaState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ChainPhase {
    Idle,
    Charging,
    Reserving,
    Notifying,
    Refunding,
    Completed,
    Refunded,
}

impl Default for ChainPhase {
    fn default() -> Self {
        ChainPhase::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SagaState {
    request_id: u64,
    order_id: String,
    customer_id: String,
    amount_cents: u64,
    reserve_sku: String,
    charge_status: Option<i64>,
    reserve_status: Option<i64>,
    notify_status: Option<i64>,
    refund_status: Option<i64>,
    last_error: Option<String>,
    charge_target: ChainHttpTarget,
    reserve_target: ChainHttpTarget,
    notify_target: ChainHttpTarget,
    refund_target: ChainHttpTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChainHttpTarget {
    name: String,
    method: String,
    url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ChainEvent {
    Start {
        order_id: String,
        customer_id: String,
        amount_cents: u64,
        reserve_sku: String,
        charge: ChainHttpTarget,
        reserve: ChainHttpTarget,
        notify: ChainHttpTarget,
        refund: ChainHttpTarget,
    },
    ChargeCompleted {
        request_id: u64,
        status: i64,
        body_preview: String,
    },
    ReserveCompleted {
        request_id: u64,
        status: i64,
        body_preview: String,
    },
    ReserveFailed {
        request_id: u64,
        status: i64,
        body_preview: String,
    },
    NotifyCompleted {
        request_id: u64,
        status: i64,
    },
    RefundCompleted {
        request_id: u64,
        status: i64,
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
        .map(|bytes| serde_cbor::from_slice::<ChainState>(&bytes).expect("state"))
        .unwrap_or_default();

    let mut domain_events = Vec::new();
    if input.event.schema == EVENT_SCHEMA {
        if let Ok(event) = decode_event(&input.event.value) {
            match event {
                ChainEvent::Start {
                    order_id,
                    customer_id,
                    amount_cents,
                    reserve_sku,
                    charge,
                    reserve,
                    notify,
                    refund,
                } => handle_start(
                    &mut state,
                    order_id,
                    customer_id,
                    amount_cents,
                    reserve_sku,
                    charge,
                    reserve,
                    notify,
                    refund,
                    &mut domain_events,
                ),
                ChainEvent::ChargeCompleted {
                    request_id,
                    status,
                    body_preview,
                } => handle_charge_completed(
                    &mut state,
                    request_id,
                    status,
                    body_preview,
                    &mut domain_events,
                ),
                ChainEvent::ReserveCompleted {
                    request_id,
                    status,
                    body_preview,
                } => handle_reserve_completed(
                    &mut state,
                    request_id,
                    status,
                    body_preview,
                    &mut domain_events,
                ),
                ChainEvent::ReserveFailed {
                    request_id,
                    status,
                    body_preview,
                } => handle_reserve_failed(
                    &mut state,
                    request_id,
                    status,
                    body_preview,
                    &mut domain_events,
                ),
                ChainEvent::NotifyCompleted { request_id, status } => {
                    handle_notify_completed(&mut state, request_id, status)
                }
                ChainEvent::RefundCompleted { request_id, status } => {
                    handle_refund_completed(&mut state, request_id, status)
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
    state: &mut ChainState,
    order_id: String,
    customer_id: String,
    amount_cents: u64,
    reserve_sku: String,
    charge: ChainHttpTarget,
    reserve: ChainHttpTarget,
    notify: ChainHttpTarget,
    refund: ChainHttpTarget,
    domain_events: &mut Vec<DomainEvent>,
) {
    match state.phase {
        ChainPhase::Idle | ChainPhase::Completed | ChainPhase::Refunded => {}
        _ => return,
    }
    let request_id = state.next_request_id;
    state.next_request_id = state.next_request_id.saturating_add(1);
    state.phase = ChainPhase::Charging;
    state.current_saga = Some(SagaState {
        request_id,
        order_id,
        customer_id,
        amount_cents,
        reserve_sku,
        charge_status: None,
        reserve_status: None,
        notify_status: None,
        refund_status: None,
        last_error: None,
        charge_target: charge,
        reserve_target: reserve,
        notify_target: notify,
        refund_target: refund,
    });
    emit_charge_intent(state, domain_events);
}

fn handle_charge_completed(
    state: &mut ChainState,
    request_id: u64,
    status: i64,
    body_preview: String,
    domain_events: &mut Vec<DomainEvent>,
) {
    let Some(saga) = state.current_saga.as_mut() else {
        return;
    };
    if saga.request_id != request_id {
        return;
    }
    saga.charge_status = Some(status);
    saga.last_error = None;
    state.phase = ChainPhase::Reserving;
    emit_reserve_intent(state, &body_preview, domain_events);
}

fn handle_reserve_completed(
    state: &mut ChainState,
    request_id: u64,
    status: i64,
    body_preview: String,
    domain_events: &mut Vec<DomainEvent>,
) {
    let Some(saga) = state.current_saga.as_mut() else {
        return;
    };
    if saga.request_id != request_id {
        return;
    }
    saga.reserve_status = Some(status);
    saga.last_error = None;
    state.phase = ChainPhase::Notifying;
    emit_notify_intent(state, format!("reserved: {body_preview}"), domain_events);
}

fn handle_reserve_failed(
    state: &mut ChainState,
    request_id: u64,
    status: i64,
    body_preview: String,
    domain_events: &mut Vec<DomainEvent>,
) {
    let Some(saga) = state.current_saga.as_mut() else {
        return;
    };
    if saga.request_id != request_id {
        return;
    }
    saga.reserve_status = Some(status);
    saga.last_error = Some(body_preview.clone());
    state.phase = ChainPhase::Refunding;
    emit_refund_intent(state, domain_events);
}

fn handle_notify_completed(state: &mut ChainState, request_id: u64, status: i64) {
    let Some(saga) = state.current_saga.as_mut() else {
        return;
    };
    if saga.request_id != request_id {
        return;
    }
    saga.notify_status = Some(status);
    state.phase = ChainPhase::Completed;
}

fn handle_refund_completed(state: &mut ChainState, request_id: u64, status: i64) {
    let Some(saga) = state.current_saga.as_mut() else {
        return;
    };
    if saga.request_id != request_id {
        return;
    }
    saga.refund_status = Some(status);
    state.phase = ChainPhase::Refunded;
}

fn emit_charge_intent(state: &ChainState, domain_events: &mut Vec<DomainEvent>) {
    let Some(saga) = state.current_saga.as_ref() else {
        return;
    };
    let value = Value::record([
        ("request_id", Value::Nat(saga.request_id)),
        ("order_id", Value::Text(saga.order_id.clone())),
        ("amount_cents", Value::Nat(saga.amount_cents)),
        ("customer_id", Value::Text(saga.customer_id.clone())),
        ("target", target_to_value(&saga.charge_target)),
    ]);
    domain_events.push(into_intent(CHARGE_REQUEST_SCHEMA, saga.request_id, value));
}

fn emit_reserve_intent(
    state: &ChainState,
    reserve_summary: &str,
    domain_events: &mut Vec<DomainEvent>,
) {
    let Some(saga) = state.current_saga.as_ref() else {
        return;
    };
    let value = Value::record([
        ("request_id", Value::Nat(saga.request_id)),
        ("order_id", Value::Text(saga.order_id.clone())),
        (
            "sku",
            Value::Text(format!("{}:{reserve_summary}", saga.reserve_sku)),
        ),
        ("target", target_to_value(&saga.reserve_target)),
    ]);
    domain_events.push(into_intent(RESERVE_REQUEST_SCHEMA, saga.request_id, value));
}

fn emit_notify_intent(
    state: &ChainState,
    status_text: String,
    domain_events: &mut Vec<DomainEvent>,
) {
    let Some(saga) = state.current_saga.as_ref() else {
        return;
    };
    let value = Value::record([
        ("request_id", Value::Nat(saga.request_id)),
        ("order_id", Value::Text(saga.order_id.clone())),
        ("status_text", Value::Text(status_text)),
        ("target", target_to_value(&saga.notify_target)),
    ]);
    domain_events.push(into_intent(NOTIFY_REQUEST_SCHEMA, saga.request_id, value));
}

fn emit_refund_intent(state: &ChainState, domain_events: &mut Vec<DomainEvent>) {
    let Some(saga) = state.current_saga.as_ref() else {
        return;
    };
    let value = Value::record([
        ("request_id", Value::Nat(saga.request_id)),
        ("order_id", Value::Text(saga.order_id.clone())),
        ("amount_cents", Value::Nat(saga.amount_cents)),
        ("target", target_to_value(&saga.refund_target)),
    ]);
    domain_events.push(into_intent(REFUND_REQUEST_SCHEMA, saga.request_id, value));
}

fn into_intent(schema: &str, request_id: u64, value: Value) -> DomainEvent {
    let payload = serde_cbor::to_vec(&value).expect("intent value");
    let key = request_id.to_be_bytes().to_vec();
    DomainEvent::with_key(schema, payload, key)
}

fn target_to_value(target: &ChainHttpTarget) -> Value {
    Value::record([
        ("name", Value::Text(target.name.clone())),
        ("method", Value::Text(target.method.clone())),
        ("url", Value::Text(target.url.clone())),
    ])
}

fn decode_event(bytes: &[u8]) -> Result<ChainEvent, serde_cbor::Error> {
    if let Ok(event) = serde_cbor::from_slice::<ChainEvent>(bytes) {
        return Ok(event);
    }
    let value: Value = serde_cbor::from_slice(bytes)?;
    match value {
        Value::Record(mut record) => {
            if let (Some(Value::Text(tag)), Some(body)) = (
                record.swap_remove("$tag"),
                record.swap_remove("$value"),
            ) {
                return parse_variant(tag, body);
            }
        }
        _ => {}
    }
    Err(serde_cbor::Error::custom("unsupported event variant"))
}

fn parse_variant(tag: String, body: Value) -> Result<ChainEvent, serde_cbor::Error> {
    let cbor_body = value_to_cbor_value(body)?;
    let mut map = BTreeMap::new();
    map.insert(serde_cbor::Value::Text(tag.clone()), cbor_body);
    let wrapped = serde_cbor::Value::Map(map);
    serde_cbor::value::from_value(wrapped).map_err(|err| {
        serde_cbor::Error::custom(format!("{tag} variant decode error: {err}"))
    })
}

fn value_to_cbor_value(value: Value) -> Result<serde_cbor::Value, serde_cbor::Error> {
    Ok(match value {
        Value::Unit | Value::Null => serde_cbor::Value::Null,
        Value::Bool(value) => serde_cbor::Value::Bool(value),
        Value::Int(value) => serde_cbor::Value::Integer(value as i128),
        Value::Nat(value) => serde_cbor::Value::Integer(value as i128),
        Value::Dec128(value) => serde_cbor::Value::Text(value),
        Value::Bytes(bytes) => serde_cbor::Value::Bytes(bytes.into()),
        Value::Text(text) => serde_cbor::Value::Text(text),
        Value::TimeNs(value) => serde_cbor::Value::Integer(value as i128),
        Value::DurationNs(value) => serde_cbor::Value::Integer(value as i128),
        Value::Hash(hash) => serde_cbor::Value::Text(hash.as_str().to_owned()),
        Value::Uuid(uuid) => serde_cbor::Value::Text(uuid),
        Value::List(values) => serde_cbor::Value::Array(
            values
                .into_iter()
                .map(value_to_cbor_value)
                .collect::<Result<_, _>>()?,
        ),
        Value::Set(values) => serde_cbor::Value::Array(
            values
                .into_iter()
                .map(key_to_cbor_value)
                .collect::<Result<_, _>>()?,
        ),
        Value::Map(entries) => serde_cbor::Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| {
                    Ok((key_to_cbor_value(key)?, value_to_cbor_value(value)?))
                })
                .collect::<Result<_, serde_cbor::Error>>()?,
        ),
        Value::Record(fields) => serde_cbor::Value::Map(
            fields
                .into_iter()
                .map(|(key, value)| {
                    Ok((serde_cbor::Value::Text(key), value_to_cbor_value(value)?))
                })
                .collect::<Result<_, serde_cbor::Error>>()?,
        ),
    })
}

fn key_to_cbor_value(key: ValueKey) -> Result<serde_cbor::Value, serde_cbor::Error> {
    Ok(match key {
        ValueKey::Int(value) => serde_cbor::Value::Integer(value as i128),
        ValueKey::Nat(value) => serde_cbor::Value::Integer(value as i128),
        ValueKey::Text(value) => serde_cbor::Value::Text(value),
        ValueKey::Hash(value) => serde_cbor::Value::Text(value),
        ValueKey::Uuid(value) => serde_cbor::Value::Text(value),
    })
}

fn write_back(bytes: &[u8]) -> (i32, i32) {
    let len = bytes.len() as i32;
    let ptr = wasm_alloc(len);
    unsafe {
        let dst = slice::from_raw_parts_mut(ptr as *mut u8, len as usize);
        dst.copy_from_slice(bytes);
    }
    (ptr, len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_variant_handles_plan_encoded_events() {
        let body = Value::record([
            ("request_id", Value::Nat(42)),
            ("status", Value::Int(201)),
            ("body_preview", Value::Text("ok".into())),
        ]);
        let event = parse_variant("ChargeCompleted".into(), body).expect("decode variant");
        match event {
            ChainEvent::ChargeCompleted {
                request_id,
                status,
                body_preview,
            } => {
                assert_eq!(request_id, 42);
                assert_eq!(status, 201);
                assert_eq!(body_preview, "ok");
            }
            other => panic!("unexpected event parsed: {other:?}"),
        }
    }
}
