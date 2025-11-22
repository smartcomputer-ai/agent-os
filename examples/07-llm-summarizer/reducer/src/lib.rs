#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::String;
use aos_air_exec::Value as AirValue;
use aos_wasm_sdk::{aos_reducer, ReduceError, Reducer, ReducerCtx};
use serde::{Deserialize, Serialize};

const REQUEST_SCHEMA: &str = "demo/SummarizeRequest@1";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SummarizerState {
    pc: SummarizerPc,
    next_request_id: u64,
    pending_request: Option<u64>,
    last_summary: Option<String>,
    last_tokens_prompt: Option<u64>,
    last_tokens_completion: Option<u64>,
    last_cost_millis: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SummarizerPc {
    Idle,
    Summarizing,
    Done,
}

impl Default for SummarizerPc {
    fn default() -> Self {
        SummarizerPc::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SummarizerEvent {
    Start { url: String },
    SummaryReady {
        request_id: u64,
        summary: String,
        tokens_prompt: u64,
        tokens_completion: u64,
        cost_millis: u64,
    },
}

aos_reducer!(LlmSummarizerSm);

#[derive(Default)]
struct LlmSummarizerSm;

impl Reducer for LlmSummarizerSm {
    type State = SummarizerState;
    type Event = SummarizerEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        match event {
            SummarizerEvent::Start { url } => handle_start(ctx, url),
            SummarizerEvent::SummaryReady {
                request_id,
                summary,
                tokens_prompt,
                tokens_completion,
                cost_millis,
            } => handle_summary_ready(
                ctx,
                request_id,
                summary,
                tokens_prompt,
                tokens_completion,
                cost_millis,
            ),
        }
        Ok(())
    }
}

fn handle_start(ctx: &mut ReducerCtx<SummarizerState, ()>, url: String) {
    if matches!(ctx.state.pc, SummarizerPc::Summarizing) {
        return;
    }
    let request_id = ctx.state.next_request_id;
    ctx.state.next_request_id = ctx.state.next_request_id.saturating_add(1);
    ctx.state.pending_request = Some(request_id);
    ctx.state.pc = SummarizerPc::Summarizing;
    ctx.state.last_summary = None;
    ctx.state.last_tokens_prompt = None;
    ctx.state.last_tokens_completion = None;
    ctx.state.last_cost_millis = None;

    let intent_value = AirValue::record([
        ("request_id", AirValue::Nat(request_id)),
        ("url", AirValue::Text(url)),
    ]);
    let key = request_id.to_be_bytes();
    ctx.intent(REQUEST_SCHEMA)
        .key_bytes(&key)
        .payload(&intent_value)
        .send();
}

fn handle_summary_ready(
    ctx: &mut ReducerCtx<SummarizerState, ()>,
    request_id: u64,
    summary: String,
    tokens_prompt: u64,
    tokens_completion: u64,
    cost_millis: u64,
) {
    if ctx.state.pending_request != Some(request_id) {
        return;
    }
    ctx.state.pending_request = None;
    ctx.state.pc = SummarizerPc::Done;
    ctx.state.last_summary = Some(summary);
    ctx.state.last_tokens_prompt = Some(tokens_prompt);
    ctx.state.last_tokens_completion = Some(tokens_completion);
    ctx.state.last_cost_millis = Some(cost_millis);
}
