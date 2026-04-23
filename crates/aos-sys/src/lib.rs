//! Shared types for system workflows (`sys/*`).
//!
//! This crate provides common data structures used by built-in system workflows
//! like `sys/Workspace@1`. The types mirror the schemas in
//! `spec/defs/builtin-schemas.air.json` and `spec/defs/builtin-schemas-sdk.air.json`.

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

/// Workflow state: append-only history of workspace commits (`sys/WorkspaceHistory@1`).
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
