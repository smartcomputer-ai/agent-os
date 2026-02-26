#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::{collections::BTreeMap, string::String, vec, vec::Vec};
use aos_wasm_sdk::{
    EffectReceiptEnvelope, ReduceError, Reducer, ReducerCtx, aos_reducer, aos_variant,
};
use serde::{Deserialize, Serialize};

const HTTP_REQUEST_EFFECT: &str = "http.request";
const LLM_GENERATE_EFFECT: &str = "llm.generate";

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

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum SummarizerPc {
        Idle,
        Fetching,
        Summarizing,
        Done,
    }
}

impl Default for SummarizerPc {
    fn default() -> Self {
        SummarizerPc::Idle
    }
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum SummarizerEvent {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmRuntimeArgs {
    temperature: Option<String>,
    top_p: Option<String>,
    max_tokens: Option<u64>,
    tool_refs: Option<Vec<String>>,
    tool_choice: Option<String>,
    reasoning_effort: Option<String>,
    stop_sequences: Option<Vec<String>>,
    metadata: Option<BTreeMap<String, String>>,
    provider_options_ref: Option<String>,
    response_format_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmGenerateParams {
    correlation_id: Option<String>,
    provider: String,
    model: String,
    message_refs: Vec<String>,
    runtime: LlmRuntimeArgs,
    api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenUsage {
    prompt: u64,
    completion: u64,
    total: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmGenerateReceipt {
    output_ref: String,
    raw_output_ref: Option<String>,
    provider_response_id: Option<String>,
    finish_reason: LlmFinishReason,
    token_usage: TokenUsage,
    usage_details: Option<LlmUsageDetails>,
    warnings_ref: Option<String>,
    rate_limit_ref: Option<String>,
    cost_cents: Option<u64>,
    provider_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmFinishReason {
    reason: String,
    raw: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmUsageDetails {
    reasoning_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
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
            SummarizerEvent::Receipt(envelope) => handle_receipt(ctx, envelope)?,
        }
        Ok(())
    }
}

fn handle_start(ctx: &mut ReducerCtx<SummarizerState, ()>, url: String) {
    if matches!(ctx.state.pc, SummarizerPc::Fetching | SummarizerPc::Summarizing) {
        return;
    }
    let request_id = ctx.state.next_request_id;
    ctx.state.next_request_id = ctx.state.next_request_id.saturating_add(1);
    ctx.state.pending_request = Some(request_id);
    ctx.state.pc = SummarizerPc::Fetching;
    ctx.state.last_summary = None;
    ctx.state.last_tokens_prompt = None;
    ctx.state.last_tokens_completion = None;
    ctx.state.last_cost_millis = None;

    let params = HttpRequestParams {
        method: "GET".into(),
        url,
        headers: BTreeMap::new(),
        body_ref: None,
    };
    ctx.effects().emit_raw(HTTP_REQUEST_EFFECT, &params, Some("default"));
}

fn handle_receipt(
    ctx: &mut ReducerCtx<SummarizerState, ()>,
    envelope: EffectReceiptEnvelope,
) -> Result<(), ReduceError> {
    if ctx.state.pending_request.is_none() {
        return Ok(());
    }
    match envelope.effect_kind.as_str() {
        HTTP_REQUEST_EFFECT if matches!(ctx.state.pc, SummarizerPc::Fetching) => {
            let receipt: HttpRequestReceipt = envelope
                .decode_receipt_payload()
                .map_err(|_| ReduceError::new("invalid http.request receipt payload"))?;
            let Some(body_ref) = receipt.body_ref else {
                return Err(ReduceError::new("http.request receipt missing body_ref"));
            };

            let llm_params = LlmGenerateParams {
                correlation_id: None,
                provider: "mock".into(),
                model: "mock-summarizer".into(),
                message_refs: vec![body_ref],
                runtime: LlmRuntimeArgs {
                    temperature: Some("0".into()),
                    top_p: None,
                    max_tokens: Some(512),
                    tool_refs: None,
                    tool_choice: None,
                    reasoning_effort: None,
                    stop_sequences: None,
                    metadata: None,
                    provider_options_ref: None,
                    response_format_ref: None,
                },
                api_key: None,
            };
            ctx.effects()
                .emit_raw(LLM_GENERATE_EFFECT, &llm_params, Some("default"));
            ctx.state.pc = SummarizerPc::Summarizing;
        }
        LLM_GENERATE_EFFECT if matches!(ctx.state.pc, SummarizerPc::Summarizing) => {
            let receipt: LlmGenerateReceipt = envelope
                .decode_receipt_payload()
                .map_err(|_| ReduceError::new("invalid llm.generate receipt payload"))?;
            ctx.state.pending_request = None;
            ctx.state.pc = SummarizerPc::Done;
            ctx.state.last_summary = Some(receipt.output_ref);
            ctx.state.last_tokens_prompt = Some(receipt.token_usage.prompt);
            ctx.state.last_tokens_completion = Some(receipt.token_usage.completion);
            ctx.state.last_cost_millis = Some(receipt.cost_cents.unwrap_or(0));
        }
        _ => {}
    }
    Ok(())
}
