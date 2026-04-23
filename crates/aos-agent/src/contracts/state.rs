use super::{
    ActiveToolBatch, EffectiveToolSet, RunConfig, RunId, SessionConfig, SessionId,
    SessionLifecycle, ToolBatchId, ToolRuntimeContext, ToolSpec, default_tool_profiles,
    default_tool_registry,
};
use alloc::collections::BTreeMap;
use alloc::string::String;
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
    FollowUpMessage { index: u64 },
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
#[aos(schema = "aos.agent/PendingFollowUpTurn@1")]
pub struct PendingFollowUpTurn {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, AirSchema)]
#[aos(schema = "aos.agent/SessionState@1")]
pub struct SessionState {
    pub session_id: SessionId,
    pub lifecycle: SessionLifecycle,
    pub next_run_seq: u64,
    pub next_tool_batch_seq: u64,
    pub session_config: SessionConfig,
    pub active_run_id: Option<RunId>,
    pub active_run_config: Option<RunConfig>,
    pub active_tool_batch: Option<ActiveToolBatch>,
    #[aos(map_key_air_type = "hash", schema_ref = PendingEffect)]
    pub pending_effects: PendingEffects,
    #[aos(map_key_air_type = "hash", schema_ref = SharedPendingBlobGet)]
    pub pending_blob_gets: SharedBlobGets<PendingBlobGet>,
    #[aos(map_key_air_type = "hash", schema_ref = SharedPendingBlobPut)]
    pub pending_blob_puts: SharedBlobPuts<PendingBlobPut>,
    pub pending_follow_up_turn: Option<PendingFollowUpTurn>,
    #[aos(air_type = "hash")]
    pub queued_llm_message_refs: Option<Vec<String>>,
    #[aos(air_type = "hash")]
    pub conversation_message_refs: Vec<String>,
    #[aos(air_type = "hash")]
    pub last_output_ref: Option<String>,
    pub tool_refs_materialized: bool,
    pub in_flight_effects: u64,
    pub tool_registry: BTreeMap<String, ToolSpec>,
    pub tool_profiles: BTreeMap<String, Vec<String>>,
    pub tool_profile: String,
    pub tool_runtime_context: ToolRuntimeContext,
    pub effective_tools: EffectiveToolSet,
    #[aos(air_type = "hash")]
    pub last_tool_plan_hash: Option<String>,
    pub pending_steer: Vec<String>,
    pub pending_follow_up: Vec<String>,
    #[aos(air_type = "time")]
    pub created_at: u64,
    #[aos(air_type = "time")]
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
            pending_effects: PendingEffects::new(),
            pending_blob_gets: SharedBlobGets::new(),
            pending_blob_puts: SharedBlobPuts::new(),
            pending_follow_up_turn: None,
            queued_llm_message_refs: None,
            conversation_message_refs: Vec::new(),
            last_output_ref: None,
            tool_refs_materialized: false,
            in_flight_effects: 0,
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
