#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// Version counter for catalog entries.
pub type Version = u64;

/// Metadata describing a single object version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectMeta {
    pub name: String,
    pub kind: String,
    pub hash: String,
    pub tags: Vec<String>,
    pub created_at: u64,
    pub owner: String,
}

/// Reducer state: append-only versions per object name.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ObjectVersions {
    pub latest: Version,
    pub versions: BTreeMap<Version, ObjectMeta>,
}
