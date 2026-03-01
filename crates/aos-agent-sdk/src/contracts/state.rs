use super::{
    ActiveToolBatch, EffectiveToolSet, RunConfig, RunId, SessionConfig, SessionId,
    SessionLifecycle, ToolRuntimeContext, ToolSpec, WorkspaceApplyMode, WorkspaceSnapshot,
    default_tool_profiles, default_tool_registry,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    pub tool_registry: BTreeMap<String, ToolSpec>,
    pub tool_profiles: BTreeMap<String, Vec<String>>,
    pub tool_profile: String,
    pub tool_runtime_context: ToolRuntimeContext,
    pub effective_tools: EffectiveToolSet,
    pub last_tool_plan_hash: Option<String>,
    pub pending_steer: Vec<String>,
    pub pending_follow_up: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            session_id: SessionId::default(),
            lifecycle: SessionLifecycle::default(),
            next_run_seq: 0,
            next_tool_batch_seq: 0,
            session_config: SessionConfig::default(),
            active_run_id: None,
            active_run_config: None,
            active_tool_batch: None,
            pending_intents: BTreeMap::new(),
            in_flight_effects: 0,
            active_workspace_snapshot: None,
            pending_workspace_snapshot: None,
            pending_workspace_apply_mode: None,
            tool_registry: default_tool_registry(),
            tool_profiles: default_tool_profiles(),
            tool_profile: "openai".into(),
            tool_runtime_context: ToolRuntimeContext::default(),
            effective_tools: EffectiveToolSet::default(),
            last_tool_plan_hash: None,
            pending_steer: Vec::new(),
            pending_follow_up: Vec::new(),
            created_at: 0,
            updated_at: 0,
        }
    }
}
