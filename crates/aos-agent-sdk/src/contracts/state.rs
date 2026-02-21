use super::{
    ActiveToolBatch, RunConfig, RunId, RunLease, SessionConfig, SessionId, SessionLifecycle,
    StepId, TurnId, WorkspaceApplyMode, WorkspaceSnapshot,
};
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionState {
    pub session_id: SessionId,
    pub lifecycle: SessionLifecycle,
    pub session_epoch: u64,
    pub step_epoch: u64,
    pub next_run_seq: u64,
    pub next_turn_seq: u64,
    pub next_step_seq: u64,
    pub session_config: SessionConfig,
    pub active_run_id: Option<RunId>,
    pub active_run_config: Option<RunConfig>,
    pub active_run_step_count: u64,
    pub active_turn_id: Option<TurnId>,
    pub active_step_id: Option<StepId>,
    pub active_tool_batch: Option<ActiveToolBatch>,
    pub in_flight_effects: u64,
    pub max_in_flight_effects: u64,
    pub active_run_lease: Option<RunLease>,
    pub last_heartbeat_at: Option<u64>,
    pub active_workspace_snapshot: Option<WorkspaceSnapshot>,
    pub pending_workspace_snapshot: Option<WorkspaceSnapshot>,
    pub pending_workspace_apply_mode: Option<WorkspaceApplyMode>,
    pub pending_steer: Vec<String>,
    pub pending_follow_up: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
}
