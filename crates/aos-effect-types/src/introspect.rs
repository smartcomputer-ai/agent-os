use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::HashRef;
use crate::serde_helpers;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadMeta {
    pub journal_height: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_hash: Option<HashRef>,
    pub manifest_hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectManifestParams {
    pub consistency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectManifestReceipt {
    #[serde(with = "serde_helpers::bytes")]
    pub manifest: Vec<u8>,
    pub meta: ReadMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectWorkflowStateParams {
    pub workflow: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_helpers::bytes_opt"
    )]
    pub key: Option<Vec<u8>>,
    pub consistency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectWorkflowStateReceipt {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_helpers::bytes_opt"
    )]
    pub state: Option<Vec<u8>>,
    pub meta: ReadMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct IntrospectJournalHeadParams {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectJournalHeadReceipt {
    pub meta: ReadMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectListCellsParams {
    pub workflow: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectCellInfo {
    #[serde(with = "serde_helpers::bytes")]
    pub key: Vec<u8>,
    pub state_hash: HashRef,
    pub size: u64,
    pub last_active_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrospectListCellsReceipt {
    pub cells: Vec<IntrospectCellInfo>,
    pub meta: ReadMeta,
}
