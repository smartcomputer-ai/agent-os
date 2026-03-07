use alloc::string::String;
use serde::{Deserialize, Serialize};

use crate::HashRef;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultPutParams {
    pub alias: String,
    pub binding_id: String,
    pub value_ref: HashRef,
    pub expected_digest: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultPutReceipt {
    pub alias: String,
    pub version: u64,
    pub binding_id: String,
    pub digest: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultRotateParams {
    pub alias: String,
    pub version: u64,
    pub binding_id: String,
    pub expected_digest: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultRotateReceipt {
    pub alias: String,
    pub version: u64,
    pub binding_id: String,
    pub digest: HashRef,
}
