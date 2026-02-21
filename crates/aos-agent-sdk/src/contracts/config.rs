use alloc::string::String;
use serde::{Deserialize, Serialize};

use super::WorkspaceBinding;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionConfig {
    pub provider: String,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub max_tokens: Option<u64>,
    pub workspace_binding: Option<WorkspaceBinding>,
    pub default_prompt_pack: Option<String>,
    pub default_tool_catalog: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RunConfig {
    pub provider: String,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub max_tokens: Option<u64>,
    pub workspace_binding: Option<WorkspaceBinding>,
    pub prompt_pack: Option<String>,
    pub tool_catalog: Option<String>,
}
