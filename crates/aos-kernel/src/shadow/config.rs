use serde::{Deserialize, Serialize};

use crate::governance::ManifestPatch;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowConfig {
    pub proposal_id: u64,
    pub patch: ManifestPatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<ShadowHarness>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShadowHarness {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seed_events: Vec<(String, Vec<u8>)>,
}
