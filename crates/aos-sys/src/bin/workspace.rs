//! Workspace workflow (`sys/Workspace@1`).
//!
//! A keyed workflow that maintains append-only workspace commit history.

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::String;
use aos_sys::{WorkspaceCommit, WorkspaceHistory, WorkspaceVersion};
use aos_wasm_sdk::{ReduceError, Value, Workflow, WorkflowCtx, aos_workflow};
use serde_cbor;

// Required for WASM binary entry point
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_workflow!(Workspace);

/// Workspace workflow — keyed by workspace name.
///
/// Invariants:
/// - Key must equal `event.workspace` (enforced via `ensure_key_eq`)
/// - Versions are append-only; `latest` increments monotonically
/// - expected_head (if set) must match current latest
#[derive(Default)]
struct Workspace;

impl Workflow for Workspace {
    type State = WorkspaceHistory;
    type Event = WorkspaceCommit;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        let workspace = event.workspace;
        if !is_valid_workspace_name(&workspace) {
            return Err(ReduceError::new("invalid workspace name"));
        }

        // Key must match event.workspace for keyed routing (safeguard).
        if let Some(key) = ctx.key() {
            let decoded_key: String = if let Ok(decoded) = serde_cbor::from_slice(key) {
                decoded
            } else {
                String::from_utf8(key.to_vec())
                    .map_err(|_| ReduceError::new("key decode failed"))?
            };
            if decoded_key != workspace {
                return Err(ReduceError::new("key mismatch"));
            }
        }

        if let Some(expected) = event.expected_head {
            if expected != ctx.state.latest {
                return Err(ReduceError::new("workspace head mismatch"));
            }
        }

        let next: WorkspaceVersion = ctx.state.latest.saturating_add(1);
        ctx.state.latest = next;
        ctx.state.versions.insert(next, event.meta);
        Ok(())
    }
}

fn is_valid_workspace_name(name: &str) -> bool {
    if name.is_empty() || name.contains('/') {
        return false;
    }
    name.chars().all(is_url_safe_char)
}

fn is_url_safe_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '~' | '-')
}
