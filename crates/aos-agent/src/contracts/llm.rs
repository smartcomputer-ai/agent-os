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
