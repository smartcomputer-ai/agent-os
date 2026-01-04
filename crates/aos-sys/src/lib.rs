//! Shared types for system reducers (`sys/*`).
//!
//! This crate provides common data structures used by built-in system reducers
//! like `sys/Workspace@1`. The types mirror the schemas in
//! `spec/defs/builtin-schemas.air.json`.

#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Workspace types (sys/Workspace@1)
// ---------------------------------------------------------------------------

/// Version counter for workspace commits.
pub type WorkspaceVersion = u64;

/// Commit metadata for a workspace version (`sys/WorkspaceCommitMeta@1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceCommitMeta {
    pub root_hash: String,
    pub owner: String,
    pub created_at: u64,
}

/// Reducer state: append-only history of workspace commits (`sys/WorkspaceHistory@1`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceHistory {
    pub latest: WorkspaceVersion,
    pub versions: BTreeMap<WorkspaceVersion, WorkspaceCommitMeta>,
}

/// Event to commit a new workspace version (`sys/WorkspaceCommit@1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceCommit {
    pub workspace: String,
    pub expected_head: Option<WorkspaceVersion>,
    pub meta: WorkspaceCommitMeta,
}

/// Tree entry within a workspace (`sys/WorkspaceEntry@1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub name: String,
    pub kind: String,
    pub hash: String,
    pub size: u64,
    pub mode: u64,
}

/// Workspace tree node (`sys/WorkspaceTree@1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceTree {
    pub entries: Vec<WorkspaceEntry>,
}

// ---------------------------------------------------------------------------
// Cap enforcer ABI types (sys/CapCheckInput@1, sys/CapCheckOutput@1)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapEffectOrigin {
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapCheckInput {
    pub cap_def: String,
    pub grant_name: String,
    #[serde(with = "serde_bytes")]
    pub cap_params: Vec<u8>,
    pub effect_kind: String,
    #[serde(with = "serde_bytes")]
    pub effect_params: Vec<u8>,
    pub origin: CapEffectOrigin,
    pub logical_now_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapDenyReason {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapCheckOutput {
    pub constraints_ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny: Option<CapDenyReason>,
}
