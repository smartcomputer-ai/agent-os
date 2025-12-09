#![crate_type = "cdylib"]

use std::string::String;
use aos_sys::{ObjectMeta, ObjectVersions, Version};
use aos_wasm_sdk::{aos_reducer, ReduceError, Reducer, ReducerCtx, Value};
use serde::{Deserialize, Serialize};

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_reducer!(ObjectCatalog);

/// ObjectCatalog reducer — P1 draft per p3-query-catalog.md.
#[derive(Default)]
struct ObjectCatalog;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ObjectRegistered {
    meta: ObjectMeta,
}

impl Reducer for ObjectCatalog {
    type State = ObjectVersions;
    type Event = ObjectRegistered;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        // Basic key invariant: if keyed, the key must equal meta.name bytes.
        ctx.ensure_key_eq(event.meta.name.as_bytes())?;

        // Append-only version bump.
        let next: Version = ctx.state.latest.saturating_add(1);
        ctx.state.latest = next;
        ctx.state.versions.insert(next, event.meta);
        Ok(())
    }
}

// Placeholder to quiet unused imports when this bin is built standalone.
#[allow(dead_code)]
fn _keep_strings(_: &String) {}
