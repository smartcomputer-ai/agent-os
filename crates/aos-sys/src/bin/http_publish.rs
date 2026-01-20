//! HTTP publish registry reducer (`sys/HttpPublish@1`).
//!
//! Stores publish rules by ID for deterministic routing in the host.

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use aos_sys::{HttpPublishRegistry, HttpPublishSet};
use aos_wasm_sdk::{ReduceError, Reducer, ReducerCtx, Value, aos_reducer};

// Required for WASM binary entry point
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_reducer!(HttpPublish);

#[derive(Default)]
struct HttpPublish;

impl Reducer for HttpPublish {
    type State = HttpPublishRegistry;
    type Event = HttpPublishSet;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        if let Some(rule) = event.rule {
            ctx.state.rules.insert(event.id, rule);
        } else {
            ctx.state.rules.remove(&event.id);
        }
        Ok(())
    }
}
