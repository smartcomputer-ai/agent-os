use alloc::collections::BTreeMap;
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
#[aos(schema = "aos.agent/HostMountConfig@1")]
pub struct HostMountConfig {
    pub host_path: String,
    pub guest_path: String,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, AirSchema)]
#[aos(schema = "aos.agent/HostTargetConfig@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum HostTargetConfig {
    Local {
        mounts: Option<Vec<HostMountConfig>>,
        workdir: Option<String>,
        env: Option<BTreeMap<String, String>>,
        network_mode: Option<String>,
    },
    Sandbox {
        image: String,
        runtime_class: Option<String>,
        workdir: Option<String>,
        env: Option<BTreeMap<String, String>>,
        network_mode: Option<String>,
        mounts: Option<Vec<HostMountConfig>>,
        cpu_limit_millis: Option<u64>,
        memory_limit_bytes: Option<u64>,
    },
}

impl Default for HostTargetConfig {
    fn default() -> Self {
        Self::Local {
            mounts: None,
            workdir: None,
            env: None,
            network_mode: Some("none".into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/HostSessionOpenConfig@1")]
pub struct HostSessionOpenConfig {
    pub target: HostTargetConfig,
    pub session_ttl_ns: Option<u64>,
    pub labels: Option<BTreeMap<String, String>>,
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
    pub default_host_session_open: Option<HostSessionOpenConfig>,
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
    pub host_session_open: Option<HostSessionOpenConfig>,
}
