use alloc::string::String;
use aos_wasm_sdk::AirSchema;
use serde::{Deserialize, Serialize};

use super::{ToolCallObserved, ToolCallObservedList};

pub type LlmToolCall = ToolCallObserved;
pub type LlmToolCallList = ToolCallObservedList;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/LlmUsageRecord@1")]
pub struct LlmUsageRecord {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/LlmTokenCountQuality@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum LlmTokenCountQuality {
    Exact,
    ProviderEstimate,
    LocalEstimate,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/LlmTokenCountRecord@1")]
pub struct LlmTokenCountRecord {
    pub input_tokens: Option<u64>,
    pub original_input_tokens: Option<u64>,
    pub tool_tokens: Option<u64>,
    pub response_format_tokens: Option<u64>,
    pub quality: LlmTokenCountQuality,
    pub provider: String,
    pub model: String,
    pub candidate_plan_id: Option<String>,
    #[aos(air_type = "hash")]
    pub provider_metadata_ref: Option<String>,
    #[aos(air_type = "hash")]
    pub warnings_ref: Option<String>,
    #[aos(air_type = "time")]
    pub counted_at_ns: u64,
}
