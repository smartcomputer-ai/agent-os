use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::AirSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, AirSchema)]
#[aos(schema = "aos.agent/ToolExecutor@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum ToolExecutor {
    Effect { effect: String },
    DomainEvent { schema: String },
    HostLoop { bridge: String },
}

impl Default for ToolExecutor {
    fn default() -> Self {
        Self::HostLoop {
            bridge: "host.tool".into(),
        }
    }
}

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default, AirSchema,
)]
#[aos(schema = "aos.agent/ToolMapper@1")]
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
    InspectWorld,
    InspectWorkflow,
    WorkspaceInspect,
    WorkspaceList,
    WorkspaceRead,
    WorkspaceApply,
    WorkspaceDiff,
    WorkspaceCommit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ToolAvailabilityRule@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum ToolAvailabilityRule {
    #[default]
    Always,
    HostSessionReady,
    HostSessionNotReady,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ToolParallelismHint@1")]
pub struct ToolParallelismHint {
    pub parallel_safe: bool,
    pub resource_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ToolSpec@1")]
pub struct ToolSpec {
    pub tool_id: String,
    pub tool_name: String,
    #[aos(air_type = "hash")]
    pub tool_ref: String,
    pub description: String,
    pub args_schema_json: String,
    pub mapper: ToolMapper,
    pub executor: ToolExecutor,
    pub availability_rules: Vec<ToolAvailabilityRule>,
    pub parallelism_hint: ToolParallelismHint,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/HostSessionStatus@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum HostSessionStatus {
    #[default]
    Ready,
    Closed,
    Expired,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ToolRuntimeContext@1")]
pub struct ToolRuntimeContext {
    pub host_session_id: Option<String>,
    pub host_session_status: Option<HostSessionStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/EffectiveTool@1")]
pub struct EffectiveTool {
    pub tool_id: String,
    pub tool_name: String,
    #[aos(air_type = "hash")]
    pub tool_ref: String,
    pub description: String,
    pub args_schema_json: String,
    pub mapper: ToolMapper,
    pub executor: ToolExecutor,
    pub parallel_safe: bool,
    pub resource_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/EffectiveToolSet@1")]
pub struct EffectiveToolSet {
    pub profile_id: String,
    pub profile_requires_host_session: bool,
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

    pub fn tool_by_id(&self, tool_id: &str) -> Option<&EffectiveTool> {
        self.ordered_tools
            .iter()
            .find(|tool| tool.tool_id == tool_id)
    }

    pub fn tool_by_llm_name(&self, tool_name: &str) -> Option<&EffectiveTool> {
        self.ordered_tools
            .iter()
            .find(|tool| tool.tool_name == tool_name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ToolCallObserved@1")]
pub struct ToolCallObserved {
    pub call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub arguments_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[aos(air_type = "hash")]
    pub arguments_ref: Option<String>,
    pub provider_call_id: Option<String>,
}

pub type ToolCallObservedList = Vec<ToolCallObserved>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ToolCallLlmResult@1")]
pub struct ToolCallLlmResult {
    pub call_id: String,
    pub tool_id: String,
    pub tool_name: String,
    pub is_error: bool,
    pub output_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/PlannedToolCall@1")]
pub struct PlannedToolCall {
    pub call_id: String,
    pub tool_id: String,
    pub tool_name: String,
    pub arguments_json: String,
    #[aos(air_type = "hash")]
    pub arguments_ref: Option<String>,
    pub provider_call_id: Option<String>,
    pub mapper: ToolMapper,
    pub executor: ToolExecutor,
    pub parallel_safe: bool,
    pub resource_key: Option<String>,
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ToolExecutionPlan@1")]
pub struct ToolExecutionPlan {
    pub groups: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ToolBatchPlan@1")]
pub struct ToolBatchPlan {
    pub observed_calls: Vec<ToolCallObserved>,
    pub planned_calls: Vec<PlannedToolCall>,
    pub execution_plan: ToolExecutionPlan,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ToolOverrideScope@1")]
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

pub fn local_coding_agent_tool_registry() -> BTreeMap<String, ToolSpec> {
    crate::tools::registry::local_coding_agent_tool_registry()
}

pub fn local_coding_agent_tool_profiles() -> BTreeMap<String, Vec<String>> {
    crate::tools::registry::local_coding_agent_tool_profiles()
}

pub fn local_coding_agent_tool_profile_for_provider(provider: &str) -> String {
    crate::tools::registry::local_coding_agent_tool_profile_for_provider(provider)
}

pub fn tool_bundle_inspect() -> Vec<ToolSpec> {
    crate::tools::registry::tool_bundle_inspect()
}

pub fn tool_bundle_host_session() -> Vec<ToolSpec> {
    crate::tools::registry::tool_bundle_host_session()
}

pub fn tool_bundle_host_fs() -> Vec<ToolSpec> {
    crate::tools::registry::tool_bundle_host_fs()
}

pub fn tool_bundle_host_local() -> Vec<ToolSpec> {
    crate::tools::registry::tool_bundle_host_local()
}

pub fn tool_bundle_host_sandbox() -> Vec<ToolSpec> {
    crate::tools::registry::tool_bundle_host_sandbox()
}

pub fn tool_bundle_workspace() -> Vec<ToolSpec> {
    crate::tools::registry::tool_bundle_workspace()
}

#[derive(Debug, Clone, Default)]
pub struct ToolRegistryBuilder {
    registry: BTreeMap<String, ToolSpec>,
}

impl ToolRegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_bundle(mut self, bundle: Vec<ToolSpec>) -> Self {
        for tool in bundle {
            self.registry.insert(tool.tool_id.clone(), tool);
        }
        self
    }

    pub fn with_tool(mut self, tool: ToolSpec) -> Self {
        self.registry.insert(tool.tool_id.clone(), tool);
        self
    }

    pub fn without_tool(mut self, tool_id: &str) -> Self {
        self.registry.remove(tool_id);
        self
    }

    pub fn build(self) -> Result<BTreeMap<String, ToolSpec>, String> {
        crate::tools::registry::validate_tool_registry(&self.registry)?;
        Ok(self.registry)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolProfileBuilder {
    tool_ids: Vec<String>,
}

impl ToolProfileBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_bundle(mut self, bundle: Vec<ToolSpec>) -> Self {
        for tool in bundle {
            self = self.with_tool_id(tool.tool_id.as_str());
        }
        self
    }

    pub fn with_tool_id(mut self, tool_id: &str) -> Self {
        if !self.tool_ids.iter().any(|existing| existing == tool_id) {
            self.tool_ids.push(tool_id.into());
        }
        self
    }

    pub fn without_tool(mut self, tool_id: &str) -> Self {
        self.tool_ids.retain(|existing| existing != tool_id);
        self
    }

    pub fn build(self) -> Vec<String> {
        self.tool_ids
    }

    pub fn build_for_registry(
        self,
        registry: &BTreeMap<String, ToolSpec>,
    ) -> Result<Vec<String>, String> {
        let mut seen = BTreeSet::new();
        for tool_id in &self.tool_ids {
            if !seen.insert(tool_id.clone()) {
                return Err(format!("duplicate tool id '{tool_id}' in profile"));
            }
            if !registry.contains_key(tool_id) {
                return Err(format!("profile references unknown tool id '{tool_id}'"));
            }
        }
        Ok(self.tool_ids)
    }
}
