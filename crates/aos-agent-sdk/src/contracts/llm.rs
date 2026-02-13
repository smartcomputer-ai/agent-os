use super::{ProviderProfileId, ReasoningEffort};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LlmToolChoice {
    Auto,
    NoneChoice,
    Required,
    Tool { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LlmRuntimeArgs {
    pub temperature_dec128: Option<String>,
    pub max_tokens: Option<u64>,
    pub tool_refs: Option<Vec<String>>,
    pub tool_choice: Option<LlmToolChoice>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub stop_sequences: Option<Vec<String>>,
    pub metadata: Option<BTreeMap<String, String>>,
    pub provider_options_ref: Option<String>,
    pub response_format_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LlmGenerateParams {
    pub provider_profile_id: ProviderProfileId,
    pub provider: String,
    pub model: String,
    pub message_refs: Vec<String>,
    pub runtime: LlmRuntimeArgs,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LlmFinishReason {
    pub reason: String,
    pub raw: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LlmTokenUsage {
    pub prompt: u64,
    pub completion: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LlmUsageDetails {
    pub reasoning_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LlmGenerateReceipt {
    pub output_ref: String,
    pub raw_output_ref: Option<String>,
    pub provider_response_id: Option<String>,
    pub finish_reason: LlmFinishReason,
    pub token_usage: LlmTokenUsage,
    pub usage_details: Option<LlmUsageDetails>,
    pub warnings_ref: Option<String>,
    pub cost_cents: Option<u64>,
    pub provider_id: String,
}

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
