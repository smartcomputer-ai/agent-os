use super::{HostCommand, RunId, SessionConfig, SessionId, SessionLifecycle, StepId, TurnId};
use alloc::string::String;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum SessionEventKind {
    RunRequested {
        input_ref: String,
        run_overrides: Option<SessionConfig>,
    },
    RunStarted,
    HostCommandReceived(HostCommand),
    HostCommandApplied {
        command_id: String,
    },
    LifecycleChanged(SessionLifecycle),
    RunCompleted,
    RunFailed {
        code: String,
        detail: String,
    },
    RunCancelled {
        reason: Option<String>,
    },
    #[default]
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionEvent {
    pub session_id: SessionId,
    pub run_id: Option<RunId>,
    pub turn_id: Option<TurnId>,
    pub step_id: Option<StepId>,
    pub session_epoch: u64,
    pub step_epoch: u64,
    pub event: SessionEventKind,
}
