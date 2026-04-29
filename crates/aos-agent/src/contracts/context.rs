use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::AirSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextInputScope@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum ContextInputScope {
    World,
    #[default]
    Session,
    Run,
    Cause,
    Transcript,
    Summary,
    Memory,
    Skill,
    Domain,
    Workspace,
    Custom {
        kind: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextInputKind@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum ContextInputKind {
    #[default]
    MessageRef,
    PromptRef,
    SummaryRef,
    ArtifactRef,
    DomainRef,
    WorkspaceRef,
    MemoryRef,
    SkillRef,
    Custom {
        kind: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextPriority@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum ContextPriority {
    Required,
    High,
    #[default]
    Normal,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextBudget@1")]
pub struct ContextBudget {
    pub max_refs: Option<u64>,
    pub reserve_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextInput@1")]
pub struct ContextInput {
    pub input_id: String,
    pub kind: ContextInputKind,
    pub scope: ContextInputScope,
    pub priority: ContextPriority,
    #[aos(air_type = "hash")]
    pub content_ref: String,
    pub label: Option<String>,
    pub source_kind: Option<String>,
    pub source_id: Option<String>,
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextSelection@1")]
pub struct ContextSelection {
    pub input_id: String,
    pub selected: bool,
    pub reason: String,
    #[aos(air_type = "hash")]
    pub content_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextActionKind@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum ContextActionKind {
    #[default]
    Summarize,
    Compact,
    Materialize,
    Custom {
        kind: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextAction@1")]
pub struct ContextAction {
    pub action_id: String,
    pub kind: ContextActionKind,
    pub reason: String,
    pub required: bool,
    pub input_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextReport@1")]
pub struct ContextReport {
    pub engine: String,
    pub selected_count: u64,
    pub dropped_count: u64,
    pub budget: ContextBudget,
    pub decisions: Vec<String>,
    pub unresolved: Vec<String>,
    pub compaction_recommended: bool,
    pub compaction_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextPlan@1")]
pub struct ContextPlan {
    #[aos(air_type = "hash")]
    pub selected_refs: Vec<String>,
    pub selections: Vec<ContextSelection>,
    pub actions: Vec<ContextAction>,
    pub report: ContextReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ContextObservation@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum ContextObservation {
    SummaryCompleted {
        #[aos(air_type = "hash")]
        summary_ref: String,
        input_refs: Vec<String>,
    },
    InputPinned(ContextInput),
    InputRemoved {
        input_id: String,
    },
    #[default]
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SessionContextState@1")]
pub struct SessionContextState {
    pub pinned_inputs: Vec<ContextInput>,
    #[aos(air_type = "hash")]
    pub summary_refs: Vec<String>,
    pub last_report: Option<ContextReport>,
}
