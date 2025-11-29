#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::String;
use aos_wasm_sdk::{aos_reducer, ReduceError, Reducer, ReducerCtx, TimerSetParams, Value};
use serde::{Deserialize, Serialize};

const WORK_REQUESTED: &str = "demo/WorkRequested@1";

aos_reducer!(RetrySm);

#[derive(Default)]
struct RetrySm;

impl Reducer for RetrySm {
    type State = RetryState;
    type Event = RetryEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State>,
    ) -> Result<(), ReduceError> {
        match event {
            RetryEvent::Start(ev) => handle_start(ctx, ev),
            RetryEvent::Ok(ev) => handle_ok(ctx, ev),
            RetryEvent::Err(ev) => handle_err(ctx, ev),
            RetryEvent::Timer(timer) => handle_timer(ctx, timer),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RetryState {
    pc: Pc,
    attempt: u32,
    max_attempts: u32,
    base_delay_ms: u64,
    anchor_ns: u64,
    payload: String,
    req_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Pc {
    Idle,
    Waiting,
    Done,
    Failed,
}

impl Default for Pc {
    fn default() -> Self {
        Pc::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartWork {
    req_id: String,
    payload: String,
    max_attempts: u32,
    base_delay_ms: u64,
    now_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkOk {
    req_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkErr {
    req_id: String,
    transient: bool,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerFired {
    requested: TimerSetParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum RetryEvent {
    Start(StartWork),
    Ok(WorkOk),
    Err(WorkErr),
    Timer(TimerFired),
}

fn handle_start(ctx: &mut ReducerCtx<RetryState>, ev: StartWork) {
    ctx.state.pc = Pc::Waiting;
    ctx.state.attempt = 1;
    ctx.state.max_attempts = ev.max_attempts.max(1); // guard against zero
    ctx.state.base_delay_ms = ev.base_delay_ms.max(1);
    ctx.state.anchor_ns = ev.now_ns;
    ctx.state.payload = ev.payload.clone();
    ctx.state.req_id = ev.req_id.clone();
    emit_work_requested(ctx);
}

fn handle_ok(ctx: &mut ReducerCtx<RetryState>, _ev: WorkOk) {
    if ctx.state.req_id.is_empty() {
        return;
    }
    ctx.state.pc = Pc::Done;
}

fn handle_err(ctx: &mut ReducerCtx<RetryState>, ev: WorkErr) {
    if !matches!(ctx.state.pc, Pc::Waiting) || ev.req_id != ctx.state.req_id {
        return;
    }

    if ev.transient && ctx.state.attempt < ctx.state.max_attempts {
        schedule_retry(ctx);
        ctx.state.attempt += 1;
    } else {
        ctx.state.pc = Pc::Failed;
    }
}

fn handle_timer(ctx: &mut ReducerCtx<RetryState>, _timer: TimerFired) {
    if !matches!(ctx.state.pc, Pc::Waiting) {
        return;
    }
    emit_work_requested(ctx);
}

fn emit_work_requested(ctx: &mut ReducerCtx<RetryState>) {
    let req_id = ctx.state.req_id.clone();
    let payload = ctx.state.payload.clone();
    // Emit in canonical lens form so plan input decoding (ExprValue) succeeds.
    let value = serde_json::json!({
        "Record": {
            "req_id": { "Text": req_id },
            "payload": { "Text": payload }
        }
    });
    ctx.intent(WORK_REQUESTED).payload(&value).send();
}

fn schedule_retry(ctx: &mut ReducerCtx<RetryState>) {
    let shift = ctx.state.attempt.saturating_sub(1);
    let factor = 1u64
        .checked_shl(shift.min(63))
        .unwrap_or(u64::MAX);
    let delay_ms = ctx.state.base_delay_ms.saturating_mul(factor);
    let delay_ns = delay_ms.saturating_mul(1_000_000);
    let deliver_at_ns = ctx.state.anchor_ns.saturating_add(delay_ns);
    let params = TimerSetParams {
        deliver_at_ns,
        key: Some(ctx.state.req_id.clone()),
    };
    ctx.effects().timer_set(&params, "timer");
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkRequested {
    req_id: String,
    payload: String,
}
