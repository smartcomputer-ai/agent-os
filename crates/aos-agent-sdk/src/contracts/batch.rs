use super::ToolBatchId;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolCallStatus {
    Pending,
    Succeeded,
    Failed { code: String, detail: String },
    IgnoredStale,
    Cancelled,
}

impl ToolCallStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed { .. } | Self::IgnoredStale | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ActiveToolBatch {
    pub tool_batch_id: ToolBatchId,
    pub issued_at_step_epoch: u64,
    pub expected_call_ids: BTreeSet<String>,
    pub call_status: BTreeMap<String, ToolCallStatus>,
    pub results_ref: Option<String>,
}

impl ActiveToolBatch {
    pub fn is_settled(&self) -> bool {
        self.expected_call_ids.iter().all(|call_id| {
            self.call_status
                .get(call_id)
                .is_some_and(ToolCallStatus::is_terminal)
        })
    }
}
