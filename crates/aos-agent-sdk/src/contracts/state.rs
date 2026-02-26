use super::{
    ActiveToolBatch, RunConfig, RunId, SessionConfig, SessionId, SessionLifecycle,
    WorkspaceApplyMode, WorkspaceSnapshot,
};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PendingIntent {
    pub effect_kind: String,
    pub params_hash: String,
    pub intent_id: Option<String>,
    pub cap_slot: Option<String>,
    pub emitted_at_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionState {
    pub session_id: SessionId,
    pub lifecycle: SessionLifecycle,
    pub next_run_seq: u64,
    pub next_tool_batch_seq: u64,
    pub session_config: SessionConfig,
    pub active_run_id: Option<RunId>,
    pub active_run_config: Option<RunConfig>,
    pub active_tool_batch: Option<ActiveToolBatch>,
    pub pending_intents: BTreeMap<String, PendingIntent>,
    pub in_flight_effects: u64,
    pub active_workspace_snapshot: Option<WorkspaceSnapshot>,
    pub pending_workspace_snapshot: Option<WorkspaceSnapshot>,
    pub pending_workspace_apply_mode: Option<WorkspaceApplyMode>,
    pub pending_steer: Vec<String>,
    pub pending_follow_up: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
}
