#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use aos_effects::builtins::{
    HashRef, LlmGenerateParams, LlmGenerateReceipt, LlmRuntimeArgs, LlmToolChoice, SecretRef,
    TextOrSecretRef,
};
use aos_wasm_sdk::{
    EffectReceiptEnvelope, ReduceError, Workflow, WorkflowCtx, aos_variant, aos_workflow,
};
use serde::{Deserialize, Serialize};

const LLM_GENERATE_EFFECT: &str = "sys/llm.generate@1";

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
        message_refs: message_refs
            .into_iter()
            .map(|value| {
                HashRef::new(value)
                    .unwrap_or_else(|_| panic!("invalid message ref in chat-live workflow"))
            })
            .collect(),
        runtime: LlmRuntimeArgs {
            temperature: None,
            top_p: None,
            max_tokens,
            tool_refs: tool_refs.map(|refs| {
                refs.into_iter()
                    .map(|value| {
                        HashRef::new(value)
                            .unwrap_or_else(|_| panic!("invalid tool ref in chat-live workflow"))
                    })
                    .collect()
            }),
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
    if envelope.effect_op != LLM_GENERATE_EFFECT {
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

    apply_run_result(ctx, request_id, receipt.output_ref.to_string());
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

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use aos_wasm_abi::{ABI_VERSION, DomainEvent, WorkflowContext, WorkflowInput, WorkflowOutput};
    use aos_wasm_sdk::{StepError, step_bytes};
    use alloc::vec;

    fn context_bytes() -> Vec<u8> {
        let ctx = WorkflowContext {
            now_ns: 1,
            logical_now_ns: 1,
            journal_height: 7,
            entropy: vec![0x11; 64],
            event_hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            manifest_hash:
                "sha256:1111111111111111111111111111111111111111111111111111111111111111".into(),
            workflow: "demo/LiveChat@1".into(),
            key: None,
            cell_mode: false,
        };
        serde_cbor::to_vec(&ctx).expect("context bytes")
    }

    fn step_with(state: Option<Vec<u8>>, event: &LiveEvent) -> Result<WorkflowOutput, StepError> {
        let input = WorkflowInput {
            version: ABI_VERSION,
            state,
            event: DomainEvent::new(
                "demo/LiveEvent@1",
                serde_cbor::to_vec(event).expect("event bytes"),
            ),
            ctx: Some(context_bytes()),
        };
        let input_bytes = input.encode().expect("encode input");
        let output_bytes = step_bytes::<LiveAgentWorkflow>(&input_bytes)?;
        WorkflowOutput::decode(&output_bytes).map_err(StepError::AbiDecode)
    }

    #[test]
    fn run_requested_reduces_without_trap() {
        let event = LiveEvent::RunRequested {
            request_id: 1,
            provider: "openai-responses".into(),
            model: "gpt-5.2".into(),
            api_key_alias: "llm/openai_api".into(),
            message_refs: vec![
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            ],
            tool_refs: Some(vec![
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            ]),
            tool_choice: Some(LlmToolChoice::Required),
            max_tokens: Some(768),
        };
        let output = step_with(None, &event).expect("step");
        assert_eq!(output.effects.len(), 1);
    }
}
