use super::{ProviderProfileId, ReasoningEffort, RunId};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProviderProfile {
    pub profile_id: ProviderProfileId,
    pub provider: String,
    pub default_model: String,
    pub catalog_model_id: Option<String>,
    pub allowed_models: Option<Vec<String>>,
    pub max_tokens_cap: Option<u64>,
    pub default_reasoning_effort: Option<ReasoningEffort>,
    pub provider_options_default_ref: Option<String>,
    pub response_format_default_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProfileLookupRequested {
    pub provider_profile_id: ProviderProfileId,
    pub run_id: RunId,
    pub session_epoch: u64,
    pub step_epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProfileLookupResolved {
    pub provider_profile_id: ProviderProfileId,
    pub run_id: RunId,
    pub session_epoch: u64,
    pub step_epoch: u64,
    pub provider: String,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub max_tokens: Option<u64>,
    pub diagnostics_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProfileLookupFailureKind {
    UnknownProfile,
    ModelNotAllowed { model: String },
    MissingRequiredResolvedValues,
    ValidationError { code: String },
}

impl Default for ProfileLookupFailureKind {
    fn default() -> Self {
        Self::UnknownProfile
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProfileLookupFailed {
    pub provider_profile_id: ProviderProfileId,
    pub run_id: RunId,
    pub session_epoch: u64,
    pub step_epoch: u64,
    pub failure: ProfileLookupFailureKind,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProfileCatalogState {
    pub profiles: BTreeMap<String, ProviderProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ProfileCatalogEvent {
    UpsertProfile {
        profile: ProviderProfile,
    },
    DeleteProfile {
        profile_id: ProviderProfileId,
    },
    LookupRequested(ProfileLookupRequested),
    #[default]
    Noop,
}
