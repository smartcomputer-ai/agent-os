#![allow(improper_ctypes_definitions)]
#![no_std]

use aos_wasm_sdk::{aos_workflow, aos_variant, ReduceError, Workflow, WorkflowCtx, Value};
use serde::{Deserialize, Serialize};

aos_workflow!(CounterSm);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CounterState {
    pc: CounterPc,
    remaining: u64,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum CounterPc {
        Idle,
        Counting,
        Done,
    }
}

impl Default for CounterPc {
    fn default() -> Self {
        CounterPc::Idle
    }
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum CounterEvent {
        Start { target: u64 },
        Tick,
    }
}

#[derive(Default)]
struct CounterSm;

impl Workflow for CounterSm {
    type State = CounterState;
    type Event = CounterEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State>,
    ) -> Result<(), ReduceError> {
        match event {
            CounterEvent::Start { target } => {
                if target == 0 {
                    ctx.state.pc = CounterPc::Done;
                    ctx.state.remaining = 0;
                } else {
                    ctx.state.pc = CounterPc::Counting;
                    ctx.state.remaining = target;
                }
            }
            CounterEvent::Tick => {
                if matches!(ctx.state.pc, CounterPc::Counting) && ctx.state.remaining > 0 {
                    ctx.state.remaining -= 1;
                    if ctx.state.remaining == 0 {
                        ctx.state.pc = CounterPc::Done;
                    }
                }
            }
        }
        Ok(())
    }
}
