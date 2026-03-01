use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    pub arguments_ref: String,
    pub provider_call_id: Option<String>,
}

pub type ToolCallObservedList = Vec<ToolCallObserved>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PlannedToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments_ref: String,
    pub provider_call_id: Option<String>,
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

fn pseudo_hash(seed: &str) -> String {
    let mut out = String::from("sha256:");
    let digest = Sha256::digest(seed.as_bytes());
    for byte in digest {
        let hi = byte >> 4;
        let lo = byte & 0x0f;
        out.push(nibble_to_hex(hi));
        out.push(nibble_to_hex(lo));
    }
    out
}

const fn nibble_to_hex(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        _ => (b'a' + (n - 10)) as char,
    }
}

fn host_tool(tool_name: &str, requires_host_session: bool, hint: ToolParallelismHint) -> ToolSpec {
    ToolSpec {
        tool_name: tool_name.to_string(),
        tool_ref: pseudo_hash(tool_name),
        executor: ToolExecutor::HostLoop {
            bridge: "host.tool".into(),
        },
        availability_rules: if requires_host_session {
            vec![ToolAvailabilityRule::HostSessionReady]
        } else {
            vec![ToolAvailabilityRule::Always]
        },
        parallelism_hint: hint,
    }
}

pub fn default_tool_registry() -> BTreeMap<String, ToolSpec> {
    let mut registry = BTreeMap::new();
    let tools = [
        host_tool(
            "host.session.open",
            false,
            ToolParallelismHint {
                parallel_safe: false,
                resource_key: Some("host.session".into()),
            },
        ),
        host_tool(
            "host.exec",
            true,
            ToolParallelismHint {
                parallel_safe: false,
                resource_key: Some("host.exec".into()),
            },
        ),
        host_tool(
            "host.fs.read_file",
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.write_file",
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: Some("host.fs.write".into()),
            },
        ),
        host_tool(
            "host.fs.edit_file",
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: Some("host.fs.write".into()),
            },
        ),
        host_tool(
            "host.fs.apply_patch",
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: Some("host.fs.write".into()),
            },
        ),
        host_tool(
            "host.fs.grep",
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.glob",
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.stat",
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.exists",
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
        host_tool(
            "host.fs.list_dir",
            true,
            ToolParallelismHint {
                parallel_safe: true,
                resource_key: None,
            },
        ),
    ];

    for tool in tools {
        registry.insert(tool.tool_name.clone(), tool);
    }
    registry
}

pub fn default_tool_profiles() -> BTreeMap<String, Vec<String>> {
    let common = vec![
        "host.session.open".into(),
        "host.exec".into(),
        "host.fs.read_file".into(),
        "host.fs.write_file".into(),
        "host.fs.grep".into(),
        "host.fs.glob".into(),
        "host.fs.stat".into(),
        "host.fs.exists".into(),
        "host.fs.list_dir".into(),
    ];

    let mut profiles = BTreeMap::new();

    let mut openai = common.clone();
    openai.push("host.fs.apply_patch".into());
    profiles.insert("openai".into(), openai.clone());
    profiles.insert("default".into(), openai);

    let mut anthropic = common.clone();
    anthropic.push("host.fs.edit_file".into());
    profiles.insert("anthropic".into(), anthropic.clone());
    profiles.insert("gemini".into(), anthropic);

    profiles
}

pub fn default_tool_profile_for_provider(provider: &str) -> String {
    let normalized = provider.trim().to_ascii_lowercase();
    if normalized.contains("anthropic") {
        "anthropic".into()
    } else if normalized.contains("gemini") {
        "gemini".into()
    } else {
        "openai".into()
    }
}
