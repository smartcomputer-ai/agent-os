use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LlmToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments_ref: String,
    pub provider_call_id: Option<String>,
}

pub type LlmToolCallList = Vec<LlmToolCall>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LlmOutputEnvelope {
    pub assistant_text: Option<String>,
    pub tool_calls_ref: Option<String>,
    pub reasoning_ref: Option<String>,
}
