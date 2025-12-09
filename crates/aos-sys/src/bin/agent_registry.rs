#![crate_type = "cdylib"]

// Minimal main to satisfy bin target when built for wasm.
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

use aos_wasm_sdk::{aos_reducer, ReduceError, Reducer, ReducerCtx, Value};
use serde::{Deserialize, Serialize};

aos_reducer!(AgentRegistry);

#[derive(Default)]
struct AgentRegistry;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentEvent;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AgentState;

impl Reducer for AgentRegistry {
    type State = AgentState;
    type Event = AgentEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        _event: Self::Event,
        _ctx: &mut ReducerCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        Ok(())
    }
}
