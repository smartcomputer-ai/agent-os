use aos_node::{JournalHeight, UniverseId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadProjectionRecord {
    pub journal_head: JournalHeight,
    pub manifest_hash: String,
    #[serde(default)]
    pub universe_id: UniverseId,
    #[serde(default)]
    pub updated_at_ns: u64,
}

pub type CellStateProjectionRecord = crate::kafka::CellStateProjectionRecord;
pub type WorkspaceVersionProjectionRecord = crate::kafka::WorkspaceVersionProjectionRecord;
pub type WorkspaceRegistryProjectionRecord = crate::kafka::WorkspaceRegistryProjectionRecord;
