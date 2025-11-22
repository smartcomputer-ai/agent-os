use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ShadowSummary {
    pub manifest_hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub predicted_effects: Vec<PredictedEffect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_receipts: Vec<PendingPlanReceipt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan_results: Vec<PlanResultPreview>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ledger_deltas: Vec<LedgerDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PredictedEffect {
    pub kind: String,
    pub cap: String,
    pub intent_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_json: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingPlanReceipt {
    pub plan_id: u64,
    pub plan: Option<String>,
    pub intent_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanResultPreview {
    pub plan: String,
    pub plan_id: u64,
    pub output_schema: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LedgerDelta {
    pub ledger: LedgerKind,
    pub name: String,
    pub change: DeltaKind,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LedgerKind {
    Capability,
    Policy,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeltaKind {
    Added,
    Removed,
    Changed,
}
