//! Session reducer scaffold (`aos.agent/SessionReducer@1`).

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use aos_agent_sdk::{SessionEvent, SessionReduceError, SessionState, apply_session_event};
use aos_wasm_sdk::{ReduceError, Reducer, ReducerCtx, Value, aos_reducer};

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_reducer!(SessionReducer);

#[derive(Default)]
struct SessionReducer;

impl Reducer for SessionReducer {
    type State = SessionState;
    type Event = SessionEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        apply_session_event(&mut ctx.state, &event).map_err(map_reduce_error)
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
