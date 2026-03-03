#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::String;
use aos_wasm_sdk::{ReduceError, Value, Workflow, WorkflowCtx, aos_workflow};
use serde::{Deserialize, Serialize};

aos_workflow!(PerfCounter);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PerfState {
    count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PerfEvent {
    cell: String,
    inc: u64,
}

#[derive(Default)]
struct PerfCounter;

impl Workflow for PerfCounter {
    type State = PerfState;
    type Event = PerfEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State>,
    ) -> Result<(), ReduceError> {
        let _ = event.cell;
        ctx.state.count = ctx.state.count.saturating_add(event.inc);
        Ok(())
    }
}
