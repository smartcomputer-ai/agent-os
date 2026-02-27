#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::{collections::BTreeMap, string::{String, ToString}};
use aos_wasm_sdk::{
    EffectReceiptEnvelope, ReduceError, Workflow, WorkflowCtx, TimerSetParams, aos_workflow,
    aos_variant,
};
use serde::{Deserialize, Serialize};

const HTTP_REQUEST_EFFECT: &str = "http.request";
const TIMER_SET_EFFECT: &str = "timer.set";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RetryState {
    pc: Pc,
    attempt: u32,
    max_attempts: u32,
    base_delay_ms: u64,
    anchor_ns: u64,
    payload: String,
    req_id: String,
    pending_request: bool,
    last_status: Option<i64>,
    timers_scheduled: u32,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum Pc {
        Idle,
        Requesting,
        Backoff,
        Done,
        Failed,
    }
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

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum RetryEvent {
        Start(StartWork),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerSetReceipt {
    delivered_at_ns: u64,
    key: Option<String>,
}

aos_workflow!(RetrySm);

#[derive(Default)]
struct RetrySm;

impl Workflow for RetrySm {
    type State = RetryState;
    type Event = RetryEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        match event {
            RetryEvent::Start(ev) => handle_start(ctx, ev),
            RetryEvent::Receipt(envelope) => handle_receipt(ctx, envelope)?,
        }
        Ok(())
    }
}

fn handle_start(ctx: &mut WorkflowCtx<RetryState, ()>, ev: StartWork) {
    if matches!(ctx.state.pc, Pc::Requesting | Pc::Backoff) {
        return;
    }

    ctx.state.pc = Pc::Requesting;
    ctx.state.attempt = 1;
    ctx.state.max_attempts = ev.max_attempts.max(1);
    ctx.state.base_delay_ms = ev.base_delay_ms.max(1);
    ctx.state.anchor_ns = ev.now_ns;
    ctx.state.payload = ev.payload;
    ctx.state.req_id = ev.req_id;
    ctx.state.pending_request = true;
    ctx.state.last_status = None;
    ctx.state.timers_scheduled = 0;
    emit_http_request(ctx);
}

fn handle_receipt(
    ctx: &mut WorkflowCtx<RetryState, ()>,
    envelope: EffectReceiptEnvelope,
) -> Result<(), ReduceError> {
    if envelope.effect_kind != HTTP_REQUEST_EFFECT {
        if envelope.effect_kind == TIMER_SET_EFFECT && matches!(ctx.state.pc, Pc::Backoff) {
            let receipt: TimerSetReceipt = envelope
                .decode_receipt_payload()
                .map_err(|_| ReduceError::new("invalid timer.set receipt payload"))?;
            if receipt.key.as_deref() != Some(ctx.state.req_id.as_str()) {
                return Ok(());
            }
            ctx.state.pc = Pc::Requesting;
            ctx.state.pending_request = true;
            emit_http_request(ctx);
            return Ok(());
        }
        return Ok(());
    }
    if !ctx.state.pending_request || !matches!(ctx.state.pc, Pc::Requesting) {
        return Ok(());
    }

    let receipt: HttpRequestReceipt = envelope
        .decode_receipt_payload()
        .map_err(|_| ReduceError::new("invalid http.request receipt payload"))?;
    let status = receipt.status as i64;
    ctx.state.last_status = Some(status);
    ctx.state.pending_request = false;

    if status < 400 {
        ctx.state.pc = Pc::Done;
        return Ok(());
    }

    let transient = status >= 500;
    if transient && ctx.state.attempt < ctx.state.max_attempts {
        schedule_retry(ctx);
        ctx.state.attempt = ctx.state.attempt.saturating_add(1);
        ctx.state.pc = Pc::Backoff;
    } else {
        ctx.state.pc = Pc::Failed;
    }
    Ok(())
}

fn emit_http_request(ctx: &mut WorkflowCtx<RetryState, ()>) {
    let mut headers = BTreeMap::new();
    headers.insert("x-request-id".into(), ctx.state.req_id.clone());
    headers.insert("x-attempt".into(), ctx.state.attempt.to_string());

    let params = HttpRequestParams {
        method: "POST".into(),
        url: "https://example.com/work".into(),
        headers,
        body_ref: None,
    };
    ctx.effects().emit_raw(HTTP_REQUEST_EFFECT, &params, Some("default"));
}

fn schedule_retry(ctx: &mut WorkflowCtx<RetryState, ()>) {
    let shift = ctx.state.attempt.saturating_sub(1).min(63);
    let factor = 1u64 << shift;
    let delay_ms = ctx.state.base_delay_ms.saturating_mul(factor);
    let delay_ns = delay_ms.saturating_mul(1_000_000);
    let deliver_at_ns = ctx.state.anchor_ns.saturating_add(delay_ns);
    let params = TimerSetParams {
        deliver_at_ns,
        key: Some(ctx.state.req_id.clone()),
    };
    ctx.state.timers_scheduled = ctx.state.timers_scheduled.saturating_add(1);
    ctx.effects().timer_set(&params, "default");
}
