//! Shared types for system reducers (`sys/*`).
//!
//! This crate provides common data structures used by built-in system reducers
//! like `sys/ObjectCatalog@1`. The types mirror the schemas in
//! `spec/defs/builtin-schemas.air.json`.

#![no_std]

extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ObjectCatalog types (sys/ObjectCatalog@1)
// ---------------------------------------------------------------------------

/// Version counter for catalog entries.
pub type Version = u64;

/// Metadata describing a single object version (`sys/ObjectMeta@1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectMeta {
    pub name: String,
    pub kind: String,
    pub hash: String,
    pub tags: BTreeSet<String>,
    pub created_at: u64,
    pub owner: String,
}

/// Reducer state: append-only versions per object name (`sys/ObjectVersions@1`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ObjectVersions {
    pub latest: Version,
    pub versions: BTreeMap<Version, ObjectMeta>,
}

/// Event to register an object in the catalog (`sys/ObjectRegistered@1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectRegistered {
    pub meta: ObjectMeta,
}
