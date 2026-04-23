//! Session workflow scaffold (`aos.agent/SessionWorkflow@1`).

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use aos_agent::SessionWorkflow;
use aos_wasm_sdk::aos_workflow;

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_workflow!(SessionWorkflow);
