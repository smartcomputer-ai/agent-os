//! ObjectCatalog reducer (`sys/ObjectCatalog@1`).
//!
//! A keyed reducer that maintains a versioned catalog of named objects.
//! Each object name maps to an append-only history of versions.

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::String;
use aos_sys::{ObjectRegistered, ObjectVersions, Version};
use aos_wasm_sdk::{ReduceError, Reducer, ReducerCtx, Value, aos_reducer};
use serde_cbor;

// Required for WASM binary entry point
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_reducer!(ObjectCatalog);

/// ObjectCatalog reducer — keyed by object name.
///
/// Invariants:
/// - Key must equal `meta.name` (enforced via `ensure_key_eq`)
/// - Versions are append-only; `latest` increments monotonically
/// - No micro-effects; pure state machine
#[derive(Default)]
struct ObjectCatalog;

impl Reducer for ObjectCatalog {
    type State = ObjectVersions;
    type Event = ObjectRegistered;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        let meta = event.meta;

        // Key must match meta.name for keyed routing (safeguard).
        if let Some(key) = ctx.key() {
            let decoded_key: String =
                serde_cbor::from_slice(key).map_err(|_| ReduceError::new("key decode failed"))?;
            if decoded_key != meta.name {
                return Err(ReduceError::new("key mismatch"));
            }
        }

        // Append-only version bump (0 → 1 on first registration)
        let next: Version = ctx.state.latest.saturating_add(1);
        ctx.state.latest = next;
        ctx.state.versions.insert(next, meta);
        Ok(())
    }
}
