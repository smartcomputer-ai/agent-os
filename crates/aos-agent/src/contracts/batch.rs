use super::{ToolBatchId, ToolBatchPlan};
use alloc::collections::BTreeMap;
use alloc::string::String;
use aos_wasm_sdk::{AirSchema, PendingBatch, PendingEffectSet};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, AirSchema)]
#[aos(schema = "aos.agent/ToolCallStatus@1")]
#[serde(tag = "$tag", content = "$value")]
pub enum ToolCallStatus {
    Queued,
    Pending,
    Succeeded,
    Failed { code: String, detail: String },
    Ignored,
    Cancelled,
}

impl ToolCallStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed { .. } | Self::Ignored | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, AirSchema)]
#[aos(schema = "aos.agent/ActiveToolBatch@1")]
pub struct ActiveToolBatch {
    pub tool_batch_id: ToolBatchId,
    #[aos(air_type = "hash")]
    pub intent_id: String,
    #[aos(air_type = "hash")]
    pub params_hash: Option<String>,
    pub plan: ToolBatchPlan,
    pub call_status: BTreeMap<String, ToolCallStatus>,
    #[aos(schema_ref = PendingEffectSet<String>)]
    pub pending_effects: PendingEffectSet<String>,
    #[aos(schema_ref = PendingBatch<String>)]
    pub execution: PendingBatch<String>,
    pub llm_results: BTreeMap<String, super::ToolCallLlmResult>,
    #[aos(air_type = "hash")]
    pub results_ref: Option<String>,
}

impl ActiveToolBatch {
    pub fn is_settled(&self) -> bool {
        self.plan.observed_calls.iter().all(|call| {
            self.call_status
                .get(&call.call_id)
                .is_some_and(ToolCallStatus::is_terminal)
        })
    }

    pub fn contains_call(&self, call_id: &str) -> bool {
        self.plan
            .observed_calls
            .iter()
            .any(|call| call.call_id == call_id)
    }
}
