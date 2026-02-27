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
