//! Session reducer scaffold (`aos.agent/SessionReducer@1`).

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use aos_agent_sdk::{
    HostCommandKind, SessionEvent, SessionEventKind, SessionState, enqueue_host_text,
};
use aos_wasm_sdk::{ReduceError, Reducer, ReducerCtx, Value, aos_reducer};

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_reducer!(SessionReducer);

#[derive(Default)]
struct SessionReducer;

impl Reducer for SessionReducer {
    type State = SessionState;
    type Event = SessionEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        // P2.1 scaffold behavior: record host text queues and keep metadata fresh.
        ctx.state.updated_at = event.step_epoch;

        match event.event {
            SessionEventKind::HostCommandReceived(cmd) => {
                enqueue_host_text(&mut ctx.state, &cmd.command);
                if let HostCommandKind::LeaseHeartbeat { heartbeat_at, .. } = cmd.command {
                    ctx.state.last_heartbeat_at = Some(heartbeat_at);
                }
            }
            SessionEventKind::LifecycleChanged(next) => {
                ctx.state.lifecycle = next;
            }
            _ => {}
        }

        Ok(())
    }
}
