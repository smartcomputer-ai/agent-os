use super::ActiveWindowItem;
use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::AirSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TurnInputLane@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum TurnInputLane {
    System,
    Developer,
    #[default]
    Conversation,
    ToolResult,
    Steer,
    Summary,
    Memory,
    Skill,
    Domain,
    RuntimeHint,
    Custom {
        kind: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TurnInputKind@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum TurnInputKind {
    #[default]
    MessageRef,
    ResponseFormatRef,
    ProviderOptionsRef,
    ArtifactRef,
    Custom {
        kind: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TurnPriority@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum TurnPriority {
    Required,
    High,
    #[default]
    Normal,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TurnBudget@1")]
pub struct TurnBudget {
    pub max_input_tokens: Option<u64>,
    pub reserve_output_tokens: Option<u64>,
    pub max_message_refs: Option<u64>,
    pub max_tool_refs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TurnInput@1")]
pub struct TurnInput {
    pub input_id: String,
    pub lane: TurnInputLane,
    pub kind: TurnInputKind,
    pub priority: TurnPriority,
    #[aos(air_type = "hash")]
    pub content_ref: String,
    pub estimated_tokens: Option<u64>,
    pub source_kind: Option<String>,
    pub source_id: Option<String>,
    pub correlation_id: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TurnToolInput@1")]
pub struct TurnToolInput {
    pub tool_id: String,
    pub priority: TurnPriority,
    pub estimated_tokens: Option<u64>,
    pub source_kind: Option<String>,
    pub source_id: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TurnToolChoice@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum TurnToolChoice {
    #[default]
    Auto,
    NoneChoice,
    Required,
    Tool {
        name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TurnTokenEstimate@1")]
pub struct TurnTokenEstimate {
    pub message_tokens: u64,
    pub tool_tokens: u64,
    pub total_input_tokens: u64,
    pub unknown_message_count: u64,
    pub unknown_tool_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/PlannerStateRef@1")]
pub struct PlannerStateRef {
    pub planner_id: String,
    pub key: String,
    #[aos(air_type = "hash")]
    pub state_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TurnReport@1")]
pub struct TurnReport {
    pub planner: String,
    pub selected_message_count: u64,
    pub dropped_message_count: u64,
    pub selected_tool_count: u64,
    pub dropped_tool_count: u64,
    pub token_estimate: TurnTokenEstimate,
    pub budget: TurnBudget,
    pub decision_codes: Vec<String>,
    pub unresolved: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TurnPrerequisiteKind@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum TurnPrerequisiteKind {
    #[default]
    MaterializeToolDefinitions,
    OpenHostSession,
    CompactContext,
    CountTokens,
    Custom {
        kind: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TurnPrerequisite@1")]
pub struct TurnPrerequisite {
    pub prerequisite_id: String,
    pub kind: TurnPrerequisiteKind,
    pub reason: String,
    pub input_ids: Vec<String>,
    pub tool_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TurnStateUpdate@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum TurnStateUpdate {
    UpsertPinnedInput(TurnInput),
    RemovePinnedInput {
        input_id: String,
    },
    UpsertDurableInput(TurnInput),
    RemoveDurableInput {
        input_id: String,
    },
    UpsertCustomStateRef(PlannerStateRef),
    RemoveCustomStateRef {
        planner_id: String,
        key: String,
    },
    #[default]
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TurnObservation@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum TurnObservation {
    InputObserved(TurnInput),
    InputRemoved {
        input_id: String,
    },
    CustomStateRefUpdated(PlannerStateRef),
    CustomStateRefRemoved {
        planner_id: String,
        key: String,
    },
    #[default]
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SessionTurnState@1")]
pub struct SessionTurnState {
    pub pinned_inputs: Vec<TurnInput>,
    pub durable_inputs: Vec<TurnInput>,
    pub last_report: Option<TurnReport>,
    pub custom_state_refs: Vec<PlannerStateRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/TurnPlan@1")]
pub struct TurnPlan {
    pub active_window_items: Vec<ActiveWindowItem>,
    pub selected_tool_ids: Vec<String>,
    pub tool_choice: Option<TurnToolChoice>,
    #[aos(air_type = "hash")]
    pub response_format_ref: Option<String>,
    #[aos(air_type = "hash")]
    pub provider_options_ref: Option<String>,
    pub prerequisites: Vec<TurnPrerequisite>,
    pub state_updates: Vec<TurnStateUpdate>,
    pub report: TurnReport,
}
