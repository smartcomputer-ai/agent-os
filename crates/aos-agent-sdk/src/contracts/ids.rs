use alloc::string::String;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub struct SessionId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub struct RunId {
    pub session_id: SessionId,
    pub run_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub struct ToolBatchId {
    pub run_id: RunId,
    pub batch_seq: u64,
}
