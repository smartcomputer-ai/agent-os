#![allow(improper_ctypes_definitions)]
#![no_std]

use aos_wasm_sdk::{ReduceError, Reducer, ReducerCtx, aos_reducer};
use serde::{Deserialize, Serialize};

aos_reducer!(FlowTracker);

#[derive(Default)]
struct FlowTracker;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FlowState {
    completed_count: u64,
    last_request_id: Option<u64>,
    last_worker_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FlowCompleted {
    request_id: u64,
    worker_count: u64,
}

impl Reducer for FlowTracker {
    type State = FlowState;
    type Event = FlowCompleted;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        ctx.state.completed_count = ctx.state.completed_count.saturating_add(1);
        ctx.state.last_request_id = Some(event.request_id);
        ctx.state.last_worker_count = Some(event.worker_count);
        Ok(())
    }
}
