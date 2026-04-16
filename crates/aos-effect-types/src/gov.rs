use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::HashRef;
use crate::serde_helpers;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum GovPatchInput {
    Hash(HashRef),
    PatchCbor(#[serde(with = "serde_helpers::bytes")] Vec<u8>),
    PatchDocJson(#[serde(with = "serde_helpers::bytes")] Vec<u8>),
    PatchBlobRef { blob_ref: HashRef, format: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum GovChangeAction {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovDefChange {
    pub kind: String,
    pub name: String,
    pub action: GovChangeAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovPatchSummary {
    #[serde(default)]
    pub base_manifest_hash: Option<HashRef>,
    pub patch_hash: HashRef,
    pub ops: Vec<String>,
    pub def_changes: Vec<GovDefChange>,
    pub manifest_sections: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovProposeParams {
    pub patch: GovPatchInput,
    #[serde(default)]
    pub summary: Option<GovPatchSummary>,
    #[serde(default)]
    pub manifest_base: Option<HashRef>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovProposeReceipt {
    pub proposal_id: u64,
    pub patch_hash: HashRef,
    #[serde(default)]
    pub manifest_base: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovShadowParams {
    pub proposal_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovPredictedEffect {
    pub kind: String,
    pub cap: String,
    pub intent_hash: HashRef,
    #[serde(default)]
    pub params_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovPendingWorkflowReceipt {
    pub instance_id: String,
    pub origin_module_id: String,
    #[serde(default)]
    pub origin_instance_key_b64: Option<String>,
    pub intent_hash: HashRef,
    pub effect_kind: String,
    pub emitted_at_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovWorkflowInstancePreview {
    pub instance_id: String,
    pub status: String,
    pub last_processed_event_seq: u64,
    #[serde(default)]
    pub module_version: Option<String>,
    pub inflight_intents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovModuleEffectAllowlist {
    pub module: String,
    pub effects_emitted: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum GovLedgerKind {
    Capability,
    Policy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum GovLedgerChange {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovLedgerDelta {
    pub ledger: GovLedgerKind,
    pub name: String,
    pub change: GovLedgerChange,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovShadowReceipt {
    pub proposal_id: u64,
    pub manifest_hash: HashRef,
    pub predicted_effects: Vec<GovPredictedEffect>,
    pub pending_workflow_receipts: Vec<GovPendingWorkflowReceipt>,
    pub workflow_instances: Vec<GovWorkflowInstancePreview>,
    pub module_effect_allowlists: Vec<GovModuleEffectAllowlist>,
    pub ledger_deltas: Vec<GovLedgerDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum GovDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovApproveParams {
    pub proposal_id: u64,
    pub decision: GovDecision,
    pub approver: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovApproveReceipt {
    pub proposal_id: u64,
    pub decision: GovDecision,
    pub patch_hash: HashRef,
    pub approver: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovApplyParams {
    pub proposal_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovApplyReceipt {
    pub proposal_id: u64,
    pub manifest_hash_new: HashRef,
    pub patch_hash: HashRef,
}
