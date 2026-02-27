#![allow(improper_ctypes_definitions)]
#![no_std]

use aos_agent_sdk::{
    SessionReduceError, SessionRuntimeLimits, SessionState, SessionWorkflowEvent,
    apply_session_event_with_catalog_and_limits,
};
use aos_wasm_sdk::{ReduceError, Workflow, WorkflowCtx, Value, aos_workflow};

aos_workflow!(AgentSessionWorkflow);

const KNOWN_PROVIDERS: &[&str] = &["openai", "anthropic", "openai-compatible"];
const KNOWN_MODELS: &[&str] = &["gpt-5.2", "claude-sonnet-4-5", "gpt-4o-mini"];
const RUNTIME_LIMITS: SessionRuntimeLimits = SessionRuntimeLimits {
    max_pending_intents: Some(6),
};

#[derive(Default)]
struct AgentSessionWorkflow;

impl Workflow for AgentSessionWorkflow {
    type State = SessionState;
    type Event = SessionWorkflowEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        let _ = apply_session_event_with_catalog_and_limits(
            &mut ctx.state,
            &event,
            KNOWN_PROVIDERS,
            KNOWN_MODELS,
            RUNTIME_LIMITS,
        )
        .map_err(map_reduce_error)?;
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
        SessionReduceError::InvalidWorkspacePromptPackJson => {
            ReduceError::new("workspace prompt pack JSON invalid")
        }
        SessionReduceError::InvalidWorkspaceToolCatalogJson => {
            ReduceError::new("workspace tool catalog JSON invalid")
        }
        SessionReduceError::MissingWorkspacePromptPackBytes => {
            ReduceError::new("workspace prompt pack bytes missing for validation")
        }
        SessionReduceError::MissingWorkspaceToolCatalogBytes => {
            ReduceError::new("workspace tool catalog bytes missing for validation")
        }
        SessionReduceError::TooManyPendingIntents => ReduceError::new("too many pending intents"),
    }
}
