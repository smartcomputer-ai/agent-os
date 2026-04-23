use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::AirSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, AirSchema)]
#[aos(schema = "aos.agent/ReasoningEffort@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/SessionConfig@1")]
pub struct SessionConfig {
    pub provider: String,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub max_tokens: Option<u64>,
    #[aos(air_type = "hash")]
    pub default_prompt_refs: Option<Vec<String>>,
    pub default_tool_profile: Option<String>,
    pub default_tool_enable: Option<Vec<String>>,
    pub default_tool_disable: Option<Vec<String>>,
    pub default_tool_force: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/RunConfig@1")]
pub struct RunConfig {
    pub provider: String,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub max_tokens: Option<u64>,
    #[aos(air_type = "hash")]
    pub prompt_refs: Option<Vec<String>>,
    pub tool_profile: Option<String>,
    pub tool_enable: Option<Vec<String>>,
    pub tool_disable: Option<Vec<String>>,
    pub tool_force: Option<Vec<String>>,
}
