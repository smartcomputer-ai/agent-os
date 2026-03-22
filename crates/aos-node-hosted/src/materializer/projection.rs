use std::collections::BTreeMap;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellStateProjectionRecord {
    pub journal_head: JournalHeight,
    pub workflow: String,
    #[serde(with = "serde_bytes")]
    pub key_hash: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub key_bytes: Vec<u8>,
    pub state_hash: String,
    pub size: u64,
    #[serde(default)]
    pub last_active_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceVersionProjectionRecord {
    pub root_hash: String,
    pub owner: String,
    #[serde(default)]
    pub created_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRegistryProjectionRecord {
    pub journal_head: JournalHeight,
    pub workspace: String,
    pub latest_version: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub versions: BTreeMap<u64, WorkspaceVersionProjectionRecord>,
    #[serde(default)]
    pub updated_at_ns: u64,
}
