#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::{collections::BTreeMap, format, string::String};
use aos_wasm_sdk::{
    EffectReceiptEnvelope, ReduceError, Reducer, ReducerCtx, aos_reducer, aos_variant,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const HTTP_REQUEST_EFFECT: &str = "http.request";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ChainState {
    phase: ChainPhase,
    next_request_id: u64,
    current_saga: Option<SagaState>,
}

aos_variant! {
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

aos_variant! {
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

#[derive(Debug, Clone, Copy)]
enum SagaStep {
    Charge,
    Reserve,
    Notify,
    Refund,
}

aos_reducer!(ChainCompSm);

#[derive(Default)]
struct ChainCompSm;

impl Reducer for ChainCompSm {
    type State = ChainState;
    type Event = ChainEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
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
                ctx,
                order_id,
                customer_id,
                amount_cents,
                reserve_sku,
                charge,
                reserve,
                notify,
                refund,
            )?,
            ChainEvent::Receipt(envelope) => handle_receipt(ctx, envelope)?,
        }
        Ok(())
    }
}

fn handle_start(
    ctx: &mut ReducerCtx<ChainState, ()>,
    order_id: String,
    customer_id: String,
    amount_cents: u64,
    reserve_sku: String,
    charge: ChainHttpTarget,
    reserve: ChainHttpTarget,
    notify: ChainHttpTarget,
    refund: ChainHttpTarget,
) -> Result<(), ReduceError> {
    match ctx.state.phase {
        ChainPhase::Idle | ChainPhase::Completed | ChainPhase::Refunded => {}
        _ => return Ok(()),
    }
    let request_id = ctx.state.next_request_id;
    ctx.state.next_request_id = ctx.state.next_request_id.saturating_add(1);
    ctx.state.phase = ChainPhase::Charging;
    ctx.state.current_saga = Some(SagaState {
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
    emit_for_step(ctx, SagaStep::Charge)
}

fn handle_receipt(
    ctx: &mut ReducerCtx<ChainState, ()>,
    envelope: EffectReceiptEnvelope,
) -> Result<(), ReduceError> {
    if envelope.effect_kind != HTTP_REQUEST_EFFECT {
        return Ok(());
    }

    let Some(step) = step_for_phase(&ctx.state.phase) else {
        return Ok(());
    };

    let expected_hash = {
        let Some(saga) = ctx.state.current_saga.as_ref() else {
            return Ok(());
        };
        hash_request_params(&params_for_step(saga, step))?
    };

    if envelope.params_hash.as_deref() != Some(expected_hash.as_str()) {
        return Ok(());
    }

    let receipt: HttpRequestReceipt = envelope
        .decode_receipt_payload()
        .map_err(|_| ReduceError::new("invalid http.request receipt payload"))?;

    let mut next_emit: Option<SagaStep> = None;
    {
        let Some(saga) = ctx.state.current_saga.as_mut() else {
            return Ok(());
        };
        match step {
            SagaStep::Charge => {
                saga.charge_status = Some(receipt.status as i64);
                saga.last_error = None;
                ctx.state.phase = ChainPhase::Reserving;
                next_emit = Some(SagaStep::Reserve);
            }
            SagaStep::Reserve => {
                saga.reserve_status = Some(receipt.status as i64);
                if receipt.status < 400 {
                    saga.last_error = None;
                    ctx.state.phase = ChainPhase::Notifying;
                    next_emit = Some(SagaStep::Notify);
                } else {
                    saga.last_error = Some(reserve_failure_message(&receipt));
                    ctx.state.phase = ChainPhase::Refunding;
                    next_emit = Some(SagaStep::Refund);
                }
            }
            SagaStep::Notify => {
                saga.notify_status = Some(receipt.status as i64);
                saga.last_error = None;
                ctx.state.phase = ChainPhase::Completed;
            }
            SagaStep::Refund => {
                saga.refund_status = Some(receipt.status as i64);
                ctx.state.phase = ChainPhase::Refunded;
            }
        }
    }

    if let Some(next_step) = next_emit {
        emit_for_step(ctx, next_step)?;
    }

    Ok(())
}

fn emit_for_step(ctx: &mut ReducerCtx<ChainState, ()>, step: SagaStep) -> Result<(), ReduceError> {
    let Some(saga) = ctx.state.current_saga.as_ref() else {
        return Ok(());
    };
    let params = params_for_step(saga, step);
    ctx.effects()
        .emit_raw(HTTP_REQUEST_EFFECT, &params, Some("default"));
    Ok(())
}

fn params_for_step(saga: &SagaState, step: SagaStep) -> HttpRequestParams {
    let target = match step {
        SagaStep::Charge => &saga.charge_target,
        SagaStep::Reserve => &saga.reserve_target,
        SagaStep::Notify => &saga.notify_target,
        SagaStep::Refund => &saga.refund_target,
    };
    HttpRequestParams {
        method: target.method.clone(),
        url: target.url.clone(),
        headers: BTreeMap::new(),
        body_ref: None,
    }
}

fn step_for_phase(phase: &ChainPhase) -> Option<SagaStep> {
    match phase {
        ChainPhase::Charging => Some(SagaStep::Charge),
        ChainPhase::Reserving => Some(SagaStep::Reserve),
        ChainPhase::Notifying => Some(SagaStep::Notify),
        ChainPhase::Refunding => Some(SagaStep::Refund),
        _ => None,
    }
}

fn reserve_failure_message(receipt: &HttpRequestReceipt) -> String {
    match receipt.body_ref.as_ref() {
        Some(body_ref) => format!("reserve failed: status={} body_ref={body_ref}", receipt.status),
        None => format!("reserve failed: status={}", receipt.status),
    }
}

fn hash_request_params(params: &HttpRequestParams) -> Result<String, ReduceError> {
    let bytes = serde_cbor::to_vec(params)
        .map_err(|_| ReduceError::new("failed to encode http.request params"))?;
    let digest = Sha256::digest(&bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push(nibble_to_hex((byte >> 4) & 0x0f));
        hex.push(nibble_to_hex(byte & 0x0f));
    }
    Ok(format!("sha256:{hex}"))
}

fn nibble_to_hex(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'a' + (nibble - 10)) as char,
    }
}
