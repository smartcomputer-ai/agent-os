use alloc::string::String;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WorkspaceBinding {
    pub workspace: String,
    pub version: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "$tag", content = "$value")]
pub enum WorkspaceApplyMode {
    #[default]
    NextRun,
    NextStepBoundary,
    ImmediateIfIdle,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WorkspaceSnapshot {
    pub workspace: String,
    pub version: Option<u64>,
    pub root_hash: Option<String>,
    pub index_ref: Option<String>,
    pub prompt_pack: Option<String>,
    pub tool_catalog: Option<String>,
    pub prompt_pack_ref: Option<String>,
    pub tool_catalog_ref: Option<String>,
}
