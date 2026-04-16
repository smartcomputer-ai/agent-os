use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::HashRef;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobPutParams {
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    #[serde(default)]
    pub blob_ref: Option<HashRef>,
    #[serde(default)]
    pub refs: Option<Vec<HashRef>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobEdge {
    pub blob_ref: HashRef,
    pub refs: Vec<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobPutReceipt {
    pub blob_ref: HashRef,
    pub edge_ref: HashRef,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobGetParams {
    pub blob_ref: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobGetReceipt {
    pub blob_ref: HashRef,
    pub size: u64,
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
}
