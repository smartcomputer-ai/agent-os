use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShadowSummary {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub predicted_effects: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_receipts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raised_events: Vec<String>,
}
