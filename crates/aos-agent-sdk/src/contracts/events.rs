use super::{
    HostCommand, RunId, RunLease, SessionConfig, SessionId, SessionLifecycle, StepId, ToolBatchId,
    ToolCallStatus, TurnId,
};
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "$tag", content = "$value")]
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
    StepBoundary,
    ToolBatchStarted {
        tool_batch_id: ToolBatchId,
        expected_call_ids: Vec<String>,
    },
    ToolCallSettled {
        tool_batch_id: ToolBatchId,
        call_id: String,
        status: ToolCallStatus,
        receipt_session_epoch: u64,
        receipt_step_epoch: u64,
    },
    ToolBatchSettled {
        tool_batch_id: ToolBatchId,
        results_ref: Option<String>,
    },
    LeaseIssued {
        lease: RunLease,
    },
    LeaseExpiryCheck {
        observed_time_ns: u64,
    },
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
