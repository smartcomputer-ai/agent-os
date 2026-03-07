//! Session workflow scaffold (`aos.agent/SessionWorkflow@1`).

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use aos_agent::{
    SessionState, SessionWorkflowEvent,
    helpers::{apply_session_workflow_event, emit_session_lifecycle_changed, map_reduce_error},
};
use aos_wasm_sdk::{ReduceError, Value, Workflow, WorkflowCtx, aos_workflow};

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_workflow!(SessionWorkflow);

#[derive(Default)]
struct SessionWorkflow;

impl Workflow for SessionWorkflow {
    type State = SessionState;
    type Event = SessionWorkflowEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        let prev_lifecycle = ctx.state.lifecycle;
        let prev_run_id = ctx.state.active_run_id.clone();
        let out = apply_session_workflow_event(&mut ctx.state, &event).map_err(map_reduce_error)?;
        for domain_event in out.domain_events {
            domain_event.emit(ctx);
        }
        for effect in out.effects {
            effect.emit(ctx);
        }
        emit_session_lifecycle_changed(ctx, prev_lifecycle, prev_run_id);
        Ok(())
    }
}
