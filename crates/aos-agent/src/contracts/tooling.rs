use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum ToolExecutor {
    Effect {
        effect_kind: String,
        cap_slot: Option<String>,
    },
    HostLoop {
        bridge: String,
    },
}

impl Default for ToolExecutor {
    fn default() -> Self {
        Self::HostLoop {
            bridge: "host.tool".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(tag = "$tag", content = "$value")]
pub enum ToolMapper {
    #[default]
    HostSessionOpen,
    HostExec,
    HostSessionSignal,
    HostFsReadFile,
    HostFsWriteFile,
    HostFsEditFile,
    HostFsApplyPatch,
    HostFsGrep,
    HostFsGlob,
    HostFsStat,
    HostFsExists,
    HostFsListDir,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "$tag", content = "$value")]
pub enum ToolAvailabilityRule {
    #[default]
    Always,
    HostSessionReady,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolParallelismHint {
    pub parallel_safe: bool,
    pub resource_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolSpec {
    pub tool_name: String,
    pub tool_ref: String,
    pub description: String,
    pub args_schema_json: String,
    pub mapper: ToolMapper,
    pub executor: ToolExecutor,
    pub availability_rules: Vec<ToolAvailabilityRule>,
    pub parallelism_hint: ToolParallelismHint,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "$tag", content = "$value")]
pub enum HostSessionStatus {
    #[default]
    Ready,
    Closed,
    Expired,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolRuntimeContext {
    pub host_session_id: Option<String>,
    pub host_session_status: Option<HostSessionStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EffectiveTool {
    pub tool_name: String,
    pub tool_ref: String,
    pub description: String,
    pub args_schema_json: String,
    pub mapper: ToolMapper,
    pub executor: ToolExecutor,
    pub parallel_safe: bool,
    pub resource_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EffectiveToolSet {
    pub profile_id: String,
    pub ordered_tools: Vec<EffectiveTool>,
}

impl EffectiveToolSet {
    pub fn tool_refs(&self) -> Option<Vec<String>> {
        if self.ordered_tools.is_empty() {
            None
        } else {
            Some(
                self.ordered_tools
                    .iter()
                    .map(|tool| tool.tool_ref.clone())
                    .collect(),
            )
        }
    }

    pub fn tool_by_name(&self, tool_name: &str) -> Option<&EffectiveTool> {
        self.ordered_tools
            .iter()
            .find(|tool| tool.tool_name == tool_name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolCallObserved {
    pub call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub arguments_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments_ref: Option<String>,
    pub provider_call_id: Option<String>,
}

pub type ToolCallObservedList = Vec<ToolCallObserved>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolCallLlmResult {
    pub call_id: String,
    pub tool_name: String,
    pub is_error: bool,
    pub output_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PlannedToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments_json: String,
    pub arguments_ref: Option<String>,
    pub provider_call_id: Option<String>,
    pub mapper: ToolMapper,
    pub executor: ToolExecutor,
    pub parallel_safe: bool,
    pub resource_key: Option<String>,
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolExecutionPlan {
    pub groups: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolBatchPlan {
    pub observed_calls: Vec<ToolCallObserved>,
    pub planned_calls: Vec<PlannedToolCall>,
    pub execution_plan: ToolExecutionPlan,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "$tag", content = "$value")]
pub enum ToolOverrideScope {
    #[default]
    Session,
    Run,
}

pub fn default_tool_registry() -> BTreeMap<String, ToolSpec> {
    crate::tools::registry::default_tool_registry()
}

pub fn default_tool_profiles() -> BTreeMap<String, Vec<String>> {
    crate::tools::registry::default_tool_profiles()
}

pub fn default_tool_profile_for_provider(provider: &str) -> String {
    crate::tools::registry::default_tool_profile_for_provider(provider)
}
