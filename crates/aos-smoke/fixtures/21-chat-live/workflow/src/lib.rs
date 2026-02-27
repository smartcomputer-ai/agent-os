#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::{
    EffectReceiptEnvelope, ReduceError, Workflow, WorkflowCtx, aos_variant, aos_workflow,
};
use serde::{Deserialize, Serialize};

const LLM_GENERATE_EFFECT: &str = "llm.generate";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LiveState {
    next_request_id: u64,
    pending_request_id: Option<u64>,
    last_output_ref: Option<String>,
    outputs: Vec<RunOutput>,
}

impl Default for LiveState {
    fn default() -> Self {
        Self {
            next_request_id: 1,
            pending_request_id: None,
            last_output_ref: None,
            outputs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunOutput {
    request_id: u64,
    output_ref: String,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum LlmToolChoice {
        Auto,
        #[serde(rename = "None")]
        NoneChoice,
        Required,
        Tool { name: String },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SecretRef {
    alias: String,
    version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "$tag", content = "$value")]
enum TextOrSecretRef {
    #[serde(rename = "literal")]
    Literal(String),
    #[serde(rename = "secret")]
    Secret(SecretRef),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmRuntimeArgs {
    temperature: Option<String>,
    top_p: Option<String>,
    max_tokens: Option<u64>,
    tool_refs: Option<Vec<String>>,
    tool_choice: Option<LlmToolChoice>,
    reasoning_effort: Option<String>,
    stop_sequences: Option<Vec<String>>,
    metadata: Option<alloc::collections::BTreeMap<String, String>>,
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
    api_key: Option<TextOrSecretRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmGenerateReceipt {
    output_ref: String,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum LiveEvent {
        RunRequested {
            request_id: u64,
            provider: String,
            model: String,
            api_key_alias: String,
            message_refs: Vec<String>,
            tool_refs: Option<Vec<String>>,
            tool_choice: Option<LlmToolChoice>,
            max_tokens: Option<u64>,
        },
        RunResult {
            request_id: u64,
            output_ref: String,
        },
        Receipt(EffectReceiptEnvelope),
    }
}

aos_workflow!(LiveAgentWorkflow);

#[derive(Default)]
struct LiveAgentWorkflow;

impl Workflow for LiveAgentWorkflow {
    type State = LiveState;
    type Event = LiveEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        match event {
            LiveEvent::RunRequested {
                request_id,
                provider,
                model,
                api_key_alias,
                message_refs,
                tool_refs,
                tool_choice,
                max_tokens,
            } => {
                on_run_requested(
                    ctx,
                    request_id,
                    provider,
                    model,
                    api_key_alias,
                    message_refs,
                    tool_refs,
                    tool_choice,
                    max_tokens,
                );
            }
            LiveEvent::RunResult {
                request_id,
                output_ref,
            } => apply_run_result(ctx, request_id, output_ref),
            LiveEvent::Receipt(envelope) => on_receipt(ctx, envelope)?,
        }
        Ok(())
    }
}

fn on_run_requested(
    ctx: &mut WorkflowCtx<LiveState, ()>,
    request_id: u64,
    provider: String,
    model: String,
    api_key_alias: String,
    message_refs: Vec<String>,
    tool_refs: Option<Vec<String>>,
    tool_choice: Option<LlmToolChoice>,
    max_tokens: Option<u64>,
) {
    if request_id >= ctx.state.next_request_id {
        ctx.state.next_request_id = request_id.saturating_add(1);
    }
    ctx.state.pending_request_id = Some(request_id);

    let params = LlmGenerateParams {
        correlation_id: Some(alloc::format!("chat-live-{request_id}")),
        provider,
        model,
        message_refs,
        runtime: LlmRuntimeArgs {
            temperature: None,
            top_p: None,
            max_tokens,
            tool_refs,
            tool_choice,
            reasoning_effort: None,
            stop_sequences: None,
            metadata: None,
            provider_options_ref: None,
            response_format_ref: None,
        },
        api_key: Some(TextOrSecretRef::Secret(SecretRef {
            alias: api_key_alias,
            version: 1,
        })),
    };

    ctx.effects().emit_raw(LLM_GENERATE_EFFECT, &params, Some("default"));
}

fn on_receipt(
    ctx: &mut WorkflowCtx<LiveState, ()>,
    envelope: EffectReceiptEnvelope,
) -> Result<(), ReduceError> {
    if envelope.effect_kind != LLM_GENERATE_EFFECT {
        return Ok(());
    }

    let Some(request_id) = ctx.state.pending_request_id else {
        return Ok(());
    };

    if envelope.status != "ok" {
        return Err(ReduceError::new("llm.generate receipt status not ok"));
    }

    let receipt: LlmGenerateReceipt = envelope
        .decode_receipt_payload()
        .map_err(|_| ReduceError::new("invalid llm.generate receipt payload"))?;

    apply_run_result(ctx, request_id, receipt.output_ref);
    Ok(())
}

fn apply_run_result(ctx: &mut WorkflowCtx<LiveState, ()>, request_id: u64, output_ref: String) {
    if ctx.state.pending_request_id == Some(request_id) {
        ctx.state.pending_request_id = None;
    }
    ctx.state.last_output_ref = Some(output_ref.clone());

    if let Some(existing) = ctx
        .state
        .outputs
        .iter_mut()
        .find(|entry| entry.request_id == request_id)
    {
        existing.output_ref = output_ref;
    } else {
        ctx.state.outputs.push(RunOutput {
            request_id,
            output_ref,
        });
    }
}
