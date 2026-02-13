use super::ProviderProfileId;
use alloc::string::String;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionConfig {
    pub provider_profile_id: ProviderProfileId,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RunConfig {
    pub provider_profile_id: ProviderProfileId,
    pub provider: String,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub max_tokens: Option<u64>,
}
