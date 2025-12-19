#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::{
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use aos_wasm_sdk::{aos_reducer, aos_variant, ReduceError, Reducer, ReducerCtx};
use serde::{Deserialize, Serialize};

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChargeRequest {
    request_id: u64,
    order_id: String,
    amount_cents: u64,
    customer_id: String,
    target: ChainHttpTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReserveRequest {
    request_id: u64,
    order_id: String,
    sku: String,
    target: ChainHttpTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NotifyRequest {
    request_id: u64,
    order_id: String,
    status_text: String,
    target: ChainHttpTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RefundRequest {
    request_id: u64,
    order_id: String,
    amount_cents: u64,
    target: ChainHttpTarget,
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
            ),
            ChainEvent::ChargeCompleted {
                request_id,
                status,
                body_preview,
            } => handle_charge_completed(ctx, request_id, status, body_preview),
            ChainEvent::ReserveCompleted {
                request_id,
                status,
                body_preview,
            } => handle_reserve_completed(ctx, request_id, status, body_preview),
            ChainEvent::ReserveFailed {
                request_id,
                status,
                body_preview,
            } => handle_reserve_failed(ctx, request_id, status, body_preview),
            ChainEvent::NotifyCompleted { request_id, status } => {
                handle_notify_completed(ctx, request_id, status)
            }
            ChainEvent::RefundCompleted { request_id, status } => {
                handle_refund_completed(ctx, request_id, status)
            }
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
) {
    match ctx.state.phase {
        ChainPhase::Idle | ChainPhase::Completed | ChainPhase::Refunded => {}
        _ => return,
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
    emit_charge_intent(ctx);
}

fn handle_charge_completed(
    ctx: &mut ReducerCtx<ChainState, ()>,
    request_id: u64,
    status: i64,
    body_preview: String,
) {
    let Some(saga) = ctx.state.current_saga.as_mut() else {
        return;
    };
    if saga.request_id != request_id {
        return;
    }
    saga.charge_status = Some(status);
    saga.last_error = None;
    ctx.state.phase = ChainPhase::Reserving;
    emit_reserve_intent(ctx, &body_preview);
}

fn handle_reserve_completed(
    ctx: &mut ReducerCtx<ChainState, ()>,
    request_id: u64,
    status: i64,
    body_preview: String,
) {
    let Some(saga) = ctx.state.current_saga.as_mut() else {
        return;
    };
    if saga.request_id != request_id {
        return;
    }
    saga.reserve_status = Some(status);
    saga.last_error = None;
    ctx.state.phase = ChainPhase::Notifying;
    emit_notify_intent(ctx, format!("reserved: {body_preview}"));
}

fn handle_reserve_failed(
    ctx: &mut ReducerCtx<ChainState, ()>,
    request_id: u64,
    status: i64,
    body_preview: String,
) {
    let Some(saga) = ctx.state.current_saga.as_mut() else {
        return;
    };
    if saga.request_id != request_id {
        return;
    }
    saga.reserve_status = Some(status);
    saga.last_error = Some(body_preview);
    ctx.state.phase = ChainPhase::Refunding;
    emit_refund_intent(ctx);
}

fn handle_notify_completed(ctx: &mut ReducerCtx<ChainState, ()>, request_id: u64, status: i64) {
    let Some(saga) = ctx.state.current_saga.as_mut() else {
        return;
    };
    if saga.request_id != request_id {
        return;
    }
    saga.notify_status = Some(status);
    ctx.state.phase = ChainPhase::Completed;
}

fn handle_refund_completed(ctx: &mut ReducerCtx<ChainState, ()>, request_id: u64, status: i64) {
    let Some(saga) = ctx.state.current_saga.as_mut() else {
        return;
    };
    if saga.request_id != request_id {
        return;
    }
    saga.refund_status = Some(status);
    ctx.state.phase = ChainPhase::Refunded;
}

fn emit_charge_intent(ctx: &mut ReducerCtx<ChainState, ()>) {
    let Some(saga) = ctx.state.current_saga.as_ref() else {
        return;
    };
    let payload = ChargeRequest {
        request_id: saga.request_id,
        order_id: saga.order_id.clone(),
        amount_cents: saga.amount_cents,
        customer_id: saga.customer_id.clone(),
        target: saga.charge_target.clone(),
    };
    let key = saga.request_id.to_be_bytes();
    ctx.intent(CHARGE_REQUEST_SCHEMA)
        .key_bytes(&key)
        .payload(&payload)
        .send();
}

fn emit_reserve_intent(ctx: &mut ReducerCtx<ChainState, ()>, body_preview: &str) {
    let Some(saga) = ctx.state.current_saga.as_ref() else {
        return;
    };
    let payload = ReserveRequest {
        request_id: saga.request_id,
        order_id: saga.order_id.clone(),
        sku: saga.reserve_sku.clone(),
        target: saga.reserve_target.clone(),
    };
    let key = saga.request_id.to_be_bytes();
    ctx.intent(RESERVE_REQUEST_SCHEMA)
        .key_bytes(&key)
        .payload(&payload)
        .send();
}

fn emit_notify_intent(ctx: &mut ReducerCtx<ChainState, ()>, body_preview: String) {
    let Some(saga) = ctx.state.current_saga.as_ref() else {
        return;
    };
    let payload = NotifyRequest {
        request_id: saga.request_id,
        order_id: saga.order_id.clone(),
        status_text: body_preview,
        target: saga.notify_target.clone(),
    };
    let key = saga.request_id.to_be_bytes();
    ctx.intent(NOTIFY_REQUEST_SCHEMA)
        .key_bytes(&key)
        .payload(&payload)
        .send();
}

fn emit_refund_intent(ctx: &mut ReducerCtx<ChainState, ()>) {
    let Some(saga) = ctx.state.current_saga.as_ref() else {
        return;
    };
    let payload = RefundRequest {
        request_id: saga.request_id,
        order_id: saga.order_id.clone(),
        amount_cents: saga.amount_cents,
        target: saga.refund_target.clone(),
    };
    let key = saga.request_id.to_be_bytes();
    ctx.intent(REFUND_REQUEST_SCHEMA)
        .key_bytes(&key)
        .payload(&payload)
        .send();
}
