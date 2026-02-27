use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ShadowSummary {
    pub manifest_hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub predicted_effects: Vec<PredictedEffect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_workflow_receipts: Vec<PendingWorkflowReceipt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_instances: Vec<WorkflowInstancePreview>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub module_effect_allowlists: Vec<ModuleEffectAllowlist>,
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
pub struct PendingWorkflowReceipt {
    pub instance_id: String,
    pub origin_module_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_instance_key_b64: Option<String>,
    pub intent_hash: String,
    pub effect_kind: String,
    pub emitted_at_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowInstancePreview {
    pub instance_id: String,
    pub status: String,
    pub last_processed_event_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_version: Option<String>,
    pub inflight_intents: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModuleEffectAllowlist {
    pub module: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects_emitted: Vec<String>,
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
