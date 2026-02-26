#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::{aos_workflow, aos_variant, ReduceError, Workflow, WorkflowCtx};
use serde::{Deserialize, Serialize};

const REQUEST_SCHEMA: &str = "demo/LiveChatRequest@1";

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
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LiveChatRequest {
    request_id: u64,
    provider: String,
    model: String,
    api_key_alias: String,
    message_refs: Vec<String>,
    tool_refs: Option<Vec<String>>,
    tool_choice: Option<LlmToolChoice>,
    max_tokens: Option<u64>,
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
                if request_id >= ctx.state.next_request_id {
                    ctx.state.next_request_id = request_id.saturating_add(1);
                }
                ctx.state.pending_request_id = Some(request_id);

                let req = LiveChatRequest {
                    request_id,
                    provider,
                    model,
                    api_key_alias,
                    message_refs,
                    tool_refs,
                    tool_choice,
                    max_tokens,
                };
                let key = request_id.to_be_bytes();
                ctx.intent(REQUEST_SCHEMA)
                    .key_bytes(&key)
                    .payload(&req)
                    .send();
            }
            LiveEvent::RunResult {
                request_id,
                output_ref,
            } => {
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
        }
        Ok(())
    }
}
