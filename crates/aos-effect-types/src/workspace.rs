use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::HashRef;
use crate::serde_helpers;

pub type WorkspaceAnnotations = BTreeMap<String, HashRef>;
pub type WorkspaceAnnotationsPatch = BTreeMap<String, Option<HashRef>>;
pub type WorkspaceVersion = u64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceCommitMeta {
    pub root_hash: HashRef,
    pub owner: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WorkspaceHistory {
    pub latest: WorkspaceVersion,
    pub versions: BTreeMap<WorkspaceVersion, WorkspaceCommitMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceCommit {
    pub workspace: String,
    #[serde(default)]
    pub expected_head: Option<WorkspaceVersion>,
    pub meta: WorkspaceCommitMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceResolveParams {
    pub workspace: String,
    #[serde(default)]
    pub version: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceResolveReceipt {
    pub exists: bool,
    #[serde(default)]
    pub resolved_version: Option<u64>,
    #[serde(default)]
    pub head: Option<u64>,
    #[serde(default)]
    pub root_hash: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceListParams {
    pub root_hash: HashRef,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    pub limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceListEntry {
    pub path: String,
    pub kind: String,
    #[serde(default)]
    pub hash: Option<HashRef>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub mode: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceListReceipt {
    pub entries: Vec<WorkspaceListEntry>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceReadRefParams {
    pub root_hash: HashRef,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRefEntry {
    pub kind: String,
    pub hash: HashRef,
    pub size: u64,
    pub mode: u64,
}

pub type WorkspaceReadRefReceipt = Option<WorkspaceRefEntry>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceReadBytesRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceReadBytesParams {
    pub root_hash: HashRef,
    pub path: String,
    #[serde(default)]
    pub range: Option<WorkspaceReadBytesRange>,
}

pub type WorkspaceReadBytesReceipt = Vec<u8>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceWriteBytesParams {
    pub root_hash: HashRef,
    pub path: String,
    #[serde(with = "serde_helpers::bytes")]
    pub bytes: Vec<u8>,
    #[serde(default)]
    pub mode: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceWriteBytesReceipt {
    pub new_root_hash: HashRef,
    pub blob_hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceWriteRefParams {
    pub root_hash: HashRef,
    pub path: String,
    pub blob_hash: HashRef,
    #[serde(default)]
    pub mode: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceWriteRefReceipt {
    pub new_root_hash: HashRef,
    pub blob_hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRemoveParams {
    pub root_hash: HashRef,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRemoveReceipt {
    pub new_root_hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceDiffParams {
    pub root_a: HashRef,
    pub root_b: HashRef,
    #[serde(default)]
    pub prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceDiffChange {
    pub path: String,
    pub kind: String,
    #[serde(default)]
    pub old_hash: Option<HashRef>,
    #[serde(default)]
    pub new_hash: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceDiffReceipt {
    pub changes: Vec<WorkspaceDiffChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceAnnotationsGetParams {
    pub root_hash: HashRef,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceAnnotationsGetReceipt {
    #[serde(default)]
    pub annotations: Option<WorkspaceAnnotations>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceAnnotationsSetParams {
    pub root_hash: HashRef,
    #[serde(default)]
    pub path: Option<String>,
    pub annotations_patch: WorkspaceAnnotationsPatch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceAnnotationsSetReceipt {
    pub new_root_hash: HashRef,
    pub annotations_hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceEmptyRootParams {
    pub workspace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceEmptyRootReceipt {
    pub root_hash: HashRef,
}
