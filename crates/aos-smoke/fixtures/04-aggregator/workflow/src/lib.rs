#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::{collections::BTreeMap, format, string::String, vec::Vec};
use aos_wasm_sdk::{
    EffectReceiptEnvelope, HttpRequestParams, ReduceError, Workflow, WorkflowCtx,
    aos_workflow, aos_variant,
};
use serde::{Deserialize, Serialize};

const HTTP_REQUEST_EFFECT: &str = "sys/http.request@1";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AggregatorState {
    pc: AggregatorPc,
    next_request_id: u64,
    pending_request: Option<u64>,
    current_topic: Option<String>,
    pending_targets: Vec<String>,
    pending_by_hash: BTreeMap<String, String>,
    last_responses: Vec<AggregateResponse>,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum AggregatorPc {
        Idle,
        Running,
        Done,
    }
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
    body_ref: Option<String>,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum AggregatorEvent {
    Start {
        topic: String,
        primary: AggregationTarget,
        secondary: AggregationTarget,
        tertiary: AggregationTarget,
    },
    Receipt(EffectReceiptEnvelope),
    }
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

aos_workflow!(AggregatorSm);

#[derive(Default)]
struct AggregatorSm;

impl Workflow for AggregatorSm {
    type State = AggregatorState;
    type Event = AggregatorEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        match event {
            AggregatorEvent::Start {
                topic,
                primary,
                secondary,
                tertiary,
            } => handle_start(ctx, topic, primary, secondary, tertiary)?,
            AggregatorEvent::Receipt(envelope) => handle_receipt(ctx, envelope)?,
        }
        Ok(())
    }
}

fn handle_start(
    ctx: &mut WorkflowCtx<AggregatorState, ()>,
    topic: String,
    primary: AggregationTarget,
    secondary: AggregationTarget,
    tertiary: AggregationTarget,
) -> Result<(), ReduceError> {
    if matches!(ctx.state.pc, AggregatorPc::Running) {
        return Ok(());
    }
    let request_id = ctx.state.next_request_id;
    ctx.state.next_request_id = ctx.state.next_request_id.saturating_add(1);
    ctx.state.pending_request = Some(request_id);
    ctx.state.current_topic = Some(topic.clone());
    ctx.state.pc = AggregatorPc::Running;
    ctx.state.pending_targets.clear();
    ctx.state.pending_by_hash.clear();
    ctx.state.last_responses.clear();

    queue_http_request(ctx, primary)?;
    queue_http_request(ctx, secondary)?;
    queue_http_request(ctx, tertiary)?;

    Ok(())
}

fn queue_http_request(
    ctx: &mut WorkflowCtx<AggregatorState, ()>,
    target: AggregationTarget,
) -> Result<(), ReduceError> {
    let issuer_ref = target.name.clone();
    let params = HttpRequestParams {
        method: target.method,
        url: target.url,
        headers: BTreeMap::new(),
        body_ref: None,
    };
    ctx.state.pending_targets.push(target.name);
    ctx.state
        .pending_by_hash
        .insert(issuer_ref.clone(), issuer_ref.clone());
    ctx.effects().emit_raw_with_issuer_ref(
        HTTP_REQUEST_EFFECT,
        &params,
        Some("default"),
        Some(issuer_ref.as_str()),
    );
    Ok(())
}

fn handle_receipt(
    ctx: &mut WorkflowCtx<AggregatorState, ()>,
    envelope: EffectReceiptEnvelope,
) -> Result<(), ReduceError> {
    if ctx.state.pending_request.is_none() {
        return Ok(());
    }
    if envelope.effect_op != HTTP_REQUEST_EFFECT {
        return Ok(());
    }
    let Some(issuer_ref) = envelope.issuer_ref.as_ref() else {
        return Err(ReduceError::new("missing issuer_ref on receipt"));
    };
    let Some(source) = ctx.state.pending_by_hash.remove(issuer_ref.as_str()) else {
        // Ignore duplicates or stale receipts for already-settled requests.
        return Ok(());
    };
    let receipt: HttpRequestReceipt = envelope
        .decode_receipt_payload()
        .map_err(|_| ReduceError::new("invalid http.request receipt payload"))?;

    ctx.state
        .pending_targets
        .retain(|name| name != source.as_str());
    ctx.state.last_responses.push(AggregateResponse {
        source,
        status: receipt.status as i64,
        body_ref: receipt.body_ref,
    });
    ctx.state
        .last_responses
        .sort_by(|left, right| left.source.cmp(&right.source));

    if ctx.state.pending_by_hash.is_empty() {
        ctx.state.pending_request = None;
        ctx.state.pc = AggregatorPc::Done;
    }

    Ok(())
}
