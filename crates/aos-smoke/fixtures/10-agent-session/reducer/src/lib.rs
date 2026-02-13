#![allow(improper_ctypes_definitions)]
#![no_std]

use aos_agent_sdk::{SessionEvent, SessionState, SessionReduceError, apply_session_event};
use aos_wasm_sdk::{ReduceError, Reducer, ReducerCtx, Value, aos_reducer};

aos_reducer!(AgentSessionReducer);

#[derive(Default)]
struct AgentSessionReducer;

impl Reducer for AgentSessionReducer {
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
        SessionReduceError::RunAlreadyActive => ReduceError::new("run already active"),
    }
}
