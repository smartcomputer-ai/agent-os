//! Session workflow scaffold (`aos.agent/SessionWorkflow@1`).

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use aos_agent_sdk::{
    SessionEffectCommand, SessionReduceError, SessionState, SessionWorkflowEvent,
    apply_session_workflow_event,
};
use aos_wasm_sdk::{ReduceError, Value, Workflow, WorkflowCtx, aos_workflow};

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_workflow!(SessionWorkflow);

#[derive(Default)]
struct SessionWorkflow;

impl Workflow for SessionWorkflow {
    type State = SessionState;
    type Event = SessionWorkflowEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        let out = apply_session_workflow_event(&mut ctx.state, &event).map_err(map_reduce_error)?;
        for effect in out.effects {
            match effect {
                SessionEffectCommand::LlmGenerate {
                    params, cap_slot, ..
                } => ctx
                    .effects()
                    .emit_raw("llm.generate", &params, cap_slot.as_deref()),
            }
        }
        Ok(())
    }
}

fn map_reduce_error(err: SessionReduceError) -> ReduceError {
    match err {
        SessionReduceError::InvalidLifecycleTransition => {
            ReduceError::new("invalid lifecycle transition")
        }
        SessionReduceError::HostCommandRejected => ReduceError::new("host command rejected"),
        SessionReduceError::ToolBatchAlreadyActive => ReduceError::new("tool batch already active"),
        SessionReduceError::ToolBatchNotActive => ReduceError::new("tool batch not active"),
        SessionReduceError::ToolBatchIdMismatch => ReduceError::new("tool batch id mismatch"),
        SessionReduceError::ToolCallUnknown => ReduceError::new("tool call id not expected"),
        SessionReduceError::ToolBatchNotSettled => ReduceError::new("tool batch not settled"),
        SessionReduceError::MissingProvider => ReduceError::new("run config provider missing"),
        SessionReduceError::MissingModel => ReduceError::new("run config model missing"),
        SessionReduceError::UnknownProvider => ReduceError::new("run config provider unknown"),
        SessionReduceError::UnknownModel => ReduceError::new("run config model unknown"),
        SessionReduceError::RunAlreadyActive => ReduceError::new("run already active"),
        SessionReduceError::RunNotActive => ReduceError::new("run not active"),
        SessionReduceError::InvalidWorkspacePromptPackJson => {
            ReduceError::new("workspace prompt pack JSON invalid")
        }
        SessionReduceError::MissingWorkspacePromptPackBytes => {
            ReduceError::new("workspace prompt pack bytes missing for validation")
        }
        SessionReduceError::TooManyPendingIntents => ReduceError::new("too many pending intents"),
        SessionReduceError::ToolProfileUnknown => ReduceError::new("tool profile unknown"),
        SessionReduceError::UnknownToolOverride => ReduceError::new("unknown tool override"),
    }
}
