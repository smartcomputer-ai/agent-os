#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::{collections::BTreeMap, string::String};
use aos_wasm_sdk::{
    EffectReceiptEnvelope, ReduceError, Workflow, WorkflowCtx, aos_workflow, aos_variant,
};
use serde::{Deserialize, Serialize};

const HTTP_REQUEST_EFFECT: &str = "http.request";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SafeUpgradeState {
    pc: SafeUpgradePc,
    next_request_id: u64,
    pending_request: Option<u64>,
    primary_status: Option<i64>,
    follow_status: Option<i64>,
    requests_observed: u64,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum SafeUpgradePc {
        Idle,
        Fetching,
        Completed,
    }
}

impl Default for SafeUpgradePc {
    fn default() -> Self {
        SafeUpgradePc::Idle
    }
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum SafeUpgradeEvent {
        Start { url: String },
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

aos_workflow!(SafeUpgradeSm);

#[derive(Default)]
struct SafeUpgradeSm;

impl Workflow for SafeUpgradeSm {
    type State = SafeUpgradeState;
    type Event = SafeUpgradeEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        match event {
            SafeUpgradeEvent::Start { url } => handle_start(ctx, url),
            SafeUpgradeEvent::Receipt(envelope) => handle_receipt(ctx, envelope)?,
        }
        Ok(())
    }
}

fn handle_start(ctx: &mut WorkflowCtx<SafeUpgradeState, ()>, url: String) {
    if matches!(ctx.state.pc, SafeUpgradePc::Fetching) {
        return;
    }

    let request_id = ctx.state.next_request_id;
    ctx.state.next_request_id = ctx.state.next_request_id.saturating_add(1);
    ctx.state.pending_request = Some(request_id);
    ctx.state.pc = SafeUpgradePc::Fetching;
    ctx.state.primary_status = None;
    ctx.state.follow_status = None;
    ctx.state.requests_observed = 0;

    emit_fetch(ctx, url);
}

fn handle_receipt(
    ctx: &mut WorkflowCtx<SafeUpgradeState, ()>,
    envelope: EffectReceiptEnvelope,
) -> Result<(), ReduceError> {
    if ctx.state.pending_request.is_none() {
        return Ok(());
    }
    if envelope.effect_kind != HTTP_REQUEST_EFFECT {
        return Ok(());
    }

    let receipt: HttpRequestReceipt = envelope
        .decode_receipt_payload()
        .map_err(|_| ReduceError::new("invalid http.request receipt payload"))?;

    ctx.state.pending_request = None;
    ctx.state.pc = SafeUpgradePc::Completed;
    ctx.state.primary_status = Some(receipt.status as i64);
    ctx.state.follow_status = None;
    ctx.state.requests_observed = 1;
    Ok(())
}

fn emit_fetch(ctx: &mut WorkflowCtx<SafeUpgradeState, ()>, url: String) {
    let params = HttpRequestParams {
        method: "GET".into(),
        url,
        headers: BTreeMap::new(),
        body_ref: None,
    };
    ctx.effects()
        .emit_raw(HTTP_REQUEST_EFFECT, &params, Some("default"));
}
