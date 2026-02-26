#![allow(improper_ctypes_definitions)]
#![no_std]

use aos_agent_sdk::{
    SessionEvent, SessionReduceError, SessionRuntimeLimits, SessionState,
    apply_session_event_with_catalog_and_limits,
};
use aos_wasm_sdk::{ReduceError, Workflow, WorkflowCtx, Value, aos_workflow};

aos_workflow!(Demiurge);

const KNOWN_PROVIDERS: &[&str] = &["openai-responses", "anthropic", "openai-compatible", "mock"];
const KNOWN_MODELS: &[&str] = &[
    "gpt-5.2",
    "gpt-5-mini",
    "gpt-5.2-codex",
    "claude-sonnet-4-5",
    "gpt-mock",
];
const RUNTIME_LIMITS: SessionRuntimeLimits = SessionRuntimeLimits {
    max_steps_per_run: Some(64),
};

#[derive(Default)]
struct Demiurge;

impl Workflow for Demiurge {
    type State = SessionState;
    type Event = SessionEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        apply_session_event_with_catalog_and_limits(
            &mut ctx.state,
            &event,
            KNOWN_PROVIDERS,
            KNOWN_MODELS,
            RUNTIME_LIMITS,
        )
        .map_err(map_reduce_error)
    }
}

fn map_reduce_error(err: SessionReduceError) -> ReduceError {
    match err {
        SessionReduceError::InvalidLifecycleTransition => {
            ReduceError::new("invalid lifecycle transition")
        }
        SessionReduceError::HostCommandRejected => ReduceError::new("host command rejected"),
        SessionReduceError::StepBoundaryRejected => ReduceError::new("step boundary rejected"),
        SessionReduceError::ToolBatchAlreadyActive => ReduceError::new("tool batch already active"),
        SessionReduceError::ToolBatchNotActive => ReduceError::new("tool batch not active"),
        SessionReduceError::ToolBatchIdMismatch => ReduceError::new("tool batch id mismatch"),
        SessionReduceError::ToolCallUnknown => ReduceError::new("tool call id not expected"),
        SessionReduceError::ToolBatchNotSettled => ReduceError::new("tool batch not settled"),
        SessionReduceError::MissingRunConfig => ReduceError::new("run config missing"),
        SessionReduceError::MissingActiveRun => ReduceError::new("active run missing"),
        SessionReduceError::MissingActiveTurn => ReduceError::new("active turn missing"),
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
    }
}
