use super::{
    ActiveToolBatch, LlmUsageRecord, RunConfig, RunId, RunLifecycle, RunTrace, RunTraceSummary,
    SessionConfig, SessionId, SessionLifecycle, SessionStatus, SessionTurnState, ToolBatchId,
    ToolRuntimeContext, ToolSpec, TurnPlan,
};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use aos_wasm_sdk::{AirSchema, PendingEffect, PendingEffects, SharedBlobGets, SharedBlobPuts};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/PendingBlobGetKind@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum PendingBlobGetKind {
    #[default]
    LlmOutputEnvelope,
    LlmToolCalls,
    ToolCallArguments {
        tool_batch_id: ToolBatchId,
        call_id: String,
    },
    ToolResultBlob {
        tool_batch_id: ToolBatchId,
        call_id: String,
        blob_ref: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/PendingBlobGet@1")]
pub struct PendingBlobGet {
    pub kind: PendingBlobGetKind,
    #[aos(air_type = "time")]
    pub emitted_at_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, AirSchema)]
#[aos(schema = "aos.agent/PendingBlobPutKind@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum PendingBlobPutKind {
    ToolDefinition { tool_id: String },
    ToolFollowUpMessage { index: u64 },
}

impl Default for PendingBlobPutKind {
    fn default() -> Self {
        Self::ToolDefinition {
            tool_id: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/PendingBlobPut@1")]
pub struct PendingBlobPut {
    pub kind: PendingBlobPutKind,
    #[aos(air_type = "time")]
    pub emitted_at_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/StagedToolFollowUpTurn@1")]
pub struct StagedToolFollowUpTurn {
    pub tool_batch_id: ToolBatchId,
    #[aos(air_type = "hash")]
    pub base_message_refs: Vec<String>,
    pub expected_messages: u64,
    #[aos(map_key_air_type = "nat", air_type = "hash")]
    pub blob_refs_by_index: BTreeMap<u64, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SharedPendingBlobGet@1")]
pub struct SharedPendingBlobGet {
    pub pending: PendingEffect,
    pub waiters: Vec<PendingBlobGet>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SharedPendingBlobPut@1")]
pub struct SharedPendingBlobPut {
    pub pending: PendingEffect,
    pub waiters: Vec<PendingBlobPut>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/CauseRef@1")]
pub struct CauseRef {
    pub kind: String,
    pub id: String,
    #[aos(air_type = "hash")]
    pub ref_: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, AirSchema)]
#[aos(schema = "aos.agent/RunCauseOrigin@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum RunCauseOrigin {
    DirectIngress {
        source: String,
        #[aos(air_type = "hash")]
        request_ref: Option<String>,
    },
    DomainEvent {
        schema: String,
        #[aos(air_type = "hash")]
        event_ref: Option<String>,
        key: Option<String>,
    },
    Internal {
        reason: String,
        #[aos(air_type = "hash")]
        ref_: Option<String>,
    },
}

impl Default for RunCauseOrigin {
    fn default() -> Self {
        Self::Internal {
            reason: String::new(),
            ref_: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/RunCause@1")]
pub struct RunCause {
    pub kind: String,
    pub origin: RunCauseOrigin,
    #[aos(air_type = "hash")]
    pub input_refs: Vec<String>,
    pub payload_schema: Option<String>,
    #[aos(air_type = "hash")]
    pub payload_ref: Option<String>,
    pub subject_refs: Vec<CauseRef>,
}

impl RunCause {
    pub fn direct_input(input_ref: String) -> Self {
        Self {
            kind: "aos.agent/user_input".into(),
            origin: RunCauseOrigin::DirectIngress {
                source: "aos.agent/RunRequested".into(),
                request_ref: None,
            },
            input_refs: vec![input_ref],
            payload_schema: None,
            payload_ref: None,
            subject_refs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/RunFailure@1")]
pub struct RunFailure {
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/RunOutcome@1")]
pub struct RunOutcome {
    #[aos(air_type = "hash")]
    pub output_ref: Option<String>,
    pub failure: Option<RunFailure>,
    pub cancelled_reason: Option<String>,
    #[aos(air_type = "hash")]
    pub interrupted_reason_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/QueuedRunStart@1")]
pub struct QueuedRunStart {
    pub cause: RunCause,
    pub run_overrides: Option<SessionConfig>,
    #[aos(air_type = "time")]
    pub queued_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/RunInterrupt@1")]
pub struct RunInterrupt {
    #[aos(air_type = "hash")]
    pub reason_ref: Option<String>,
    #[aos(air_type = "time")]
    pub requested_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/RunState@1")]
pub struct RunState {
    pub run_id: RunId,
    pub lifecycle: RunLifecycle,
    pub cause: RunCause,
    pub config: RunConfig,
    #[aos(air_type = "hash")]
    pub input_refs: Vec<String>,
    pub turn_plan: Option<TurnPlan>,
    pub trace: RunTrace,
    #[aos(air_type = "hash")]
    pub queued_steer_refs: Vec<String>,
    pub interrupt: Option<RunInterrupt>,
    pub active_tool_batch: Option<ActiveToolBatch>,
    #[aos(map_key_air_type = "hash", schema_ref = PendingEffect)]
    pub pending_effects: PendingEffects,
    #[aos(map_key_air_type = "hash", schema_ref = SharedPendingBlobGet)]
    pub pending_blob_gets: SharedBlobGets<PendingBlobGet>,
    #[aos(map_key_air_type = "hash", schema_ref = SharedPendingBlobPut)]
    pub pending_blob_puts: SharedBlobPuts<PendingBlobPut>,
    pub staged_tool_follow_up_turn: Option<StagedToolFollowUpTurn>,
    #[aos(air_type = "hash")]
    pub pending_llm_turn_refs: Option<Vec<String>>,
    #[aos(air_type = "hash")]
    pub last_output_ref: Option<String>,
    pub last_llm_usage: Option<LlmUsageRecord>,
    pub tool_refs_materialized: bool,
    pub in_flight_effects: u64,
    pub outcome: Option<RunOutcome>,
    #[aos(air_type = "time")]
    pub started_at: u64,
    #[aos(air_type = "time")]
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/RunRecord@1")]
pub struct RunRecord {
    pub run_id: RunId,
    pub lifecycle: RunLifecycle,
    pub cause: RunCause,
    #[aos(air_type = "hash")]
    pub input_refs: Vec<String>,
    pub outcome: Option<RunOutcome>,
    pub last_llm_usage: Option<LlmUsageRecord>,
    pub trace_summary: RunTraceSummary,
    #[aos(air_type = "time")]
    pub started_at: u64,
    #[aos(air_type = "time")]
    pub ended_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, AirSchema)]
#[aos(schema = "aos.agent/SessionState@1")]
pub struct SessionState {
    pub session_id: SessionId,
    pub status: SessionStatus,
    pub lifecycle: SessionLifecycle,
    pub next_run_seq: u64,
    pub next_tool_batch_seq: u64,
    pub session_config: SessionConfig,
    pub turn_state: SessionTurnState,
    pub current_run: Option<RunState>,
    pub run_history: Vec<RunRecord>,
    pub active_run_id: Option<RunId>,
    pub active_run_config: Option<RunConfig>,
    pub active_tool_batch: Option<ActiveToolBatch>,
    #[aos(map_key_air_type = "hash", schema_ref = PendingEffect)]
    pub pending_effects: PendingEffects,
    #[aos(map_key_air_type = "hash", schema_ref = SharedPendingBlobGet)]
    pub pending_blob_gets: SharedBlobGets<PendingBlobGet>,
    #[aos(map_key_air_type = "hash", schema_ref = SharedPendingBlobPut)]
    pub pending_blob_puts: SharedBlobPuts<PendingBlobPut>,
    pub staged_tool_follow_up_turn: Option<StagedToolFollowUpTurn>,
    #[aos(air_type = "hash")]
    pub pending_llm_turn_refs: Option<Vec<String>>,
    #[aos(air_type = "hash")]
    pub transcript_message_refs: Vec<String>,
    #[aos(air_type = "hash")]
    pub last_output_ref: Option<String>,
    pub tool_refs_materialized: bool,
    pub in_flight_effects: u64,
    pub tool_registry: BTreeMap<String, ToolSpec>,
    pub tool_profiles: BTreeMap<String, Vec<String>>,
    pub tool_profile: String,
    pub tool_runtime_context: ToolRuntimeContext,
    #[aos(air_type = "hash")]
    pub last_tool_plan_hash: Option<String>,
    #[aos(air_type = "hash")]
    pub queued_steer_refs: Vec<String>,
    pub queued_follow_up_runs: Vec<QueuedRunStart>,
    pub run_interrupt: Option<RunInterrupt>,
    #[aos(air_type = "time")]
    pub created_at: u64,
    #[aos(air_type = "time")]
    pub updated_at: u64,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            session_id: SessionId::default(),
            status: SessionStatus::default(),
            lifecycle: SessionLifecycle::default(),
            next_run_seq: 0,
            next_tool_batch_seq: 0,
            session_config: SessionConfig::default(),
            turn_state: SessionTurnState::default(),
            current_run: None,
            run_history: Vec::new(),
            active_run_id: None,
            active_run_config: None,
            active_tool_batch: None,
            pending_effects: PendingEffects::new(),
            pending_blob_gets: SharedBlobGets::new(),
            pending_blob_puts: SharedBlobPuts::new(),
            staged_tool_follow_up_turn: None,
            pending_llm_turn_refs: None,
            transcript_message_refs: Vec::new(),
            last_output_ref: None,
            tool_refs_materialized: false,
            in_flight_effects: 0,
            tool_registry: BTreeMap::new(),
            tool_profiles: BTreeMap::new(),
            tool_profile: String::new(),
            tool_runtime_context: ToolRuntimeContext::default(),
            last_tool_plan_hash: None,
            queued_steer_refs: Vec::new(),
            queued_follow_up_runs: Vec::new(),
            run_interrupt: None,
            created_at: 0,
            updated_at: 0,
        }
    }
}
