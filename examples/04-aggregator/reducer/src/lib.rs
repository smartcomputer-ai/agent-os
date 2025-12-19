#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::{string::String, vec, vec::Vec};
use aos_wasm_sdk::{aos_reducer, aos_variant, ReduceError, Reducer, ReducerCtx};
use serde::{Deserialize, Serialize};

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
    body_preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "$tag", content = "$value")]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AggregateRequest {
    request_id: u64,
    topic: String,
    primary: AggregationTarget,
    secondary: AggregationTarget,
    tertiary: AggregationTarget,
}

aos_reducer!(AggregatorSm);

#[derive(Default)]
struct AggregatorSm;

impl Reducer for AggregatorSm {
    type State = AggregatorState;
    type Event = AggregatorEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
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
        Ok(())
    }
}

fn handle_start(
    ctx: &mut ReducerCtx<AggregatorState, ()>,
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

    let intent_value = AggregateRequest {
        request_id,
        topic,
        primary,
        secondary,
        tertiary,
    };
    let key = request_id.to_be_bytes();
    ctx.intent(AGGREGATE_REQUEST_SCHEMA)
        .key_bytes(&key)
        .payload(&intent_value)
        .send();
}

fn handle_complete(
    ctx: &mut ReducerCtx<AggregatorState, ()>,
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
