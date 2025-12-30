#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::String;
use aos_wasm_sdk::{aos_reducer, aos_variant, aos_event_union, ReduceError, Reducer, ReducerCtx, TimerSetParams, Value};
use serde::{Deserialize, Serialize};

aos_reducer!(TimerSm);

#[derive(Default)]
struct TimerSm;

impl Reducer for TimerSm {
    type State = TimerState;
    type Event = TimerEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State>,
    ) -> Result<(), ReduceError> {
        match event {
            TimerEvent::Start(start) => handle_start(ctx, start),
            TimerEvent::Fired(_fired) => handle_timer_fired(ctx),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TimerState {
    pc: TimerPc,
    key: Option<String>,
    deadline_ns: Option<u64>,
    fired_key: Option<String>,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum TimerPc {
        Idle,
        Awaiting,
        Done,
        TimedOut,
    }
}

impl Default for TimerPc {
    fn default() -> Self {
        TimerPc::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartEvent {
    deliver_at_ns: u64,
    key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerFiredEvent {
    requested: TimerSetParams,
}

aos_event_union! {
    #[derive(Debug, Clone, Serialize)]
    enum TimerEvent {
        Start(StartEvent),
        Fired(TimerFiredEvent)
    }
}

fn handle_start(ctx: &mut ReducerCtx<TimerState>, event: StartEvent) {
    if matches!(ctx.state.pc, TimerPc::Idle | TimerPc::Done | TimerPc::TimedOut) {
        ctx.state.pc = if event.deliver_at_ns == 0 {
            TimerPc::Done
        } else {
            TimerPc::Awaiting
        };
        ctx.state.key = event.key.clone();
        ctx.state.deadline_ns = Some(event.deliver_at_ns);
        ctx.state.fired_key = None;

        if let (TimerPc::Awaiting, Some(deadline)) = (&ctx.state.pc, ctx.state.deadline_ns) {
            let params = TimerSetParams {
                deliver_at_ns: deadline,
                key: ctx.state.key.clone(),
            };
            ctx.effects().timer_set(&params, "default");
        }
    }
}

fn handle_timer_fired(ctx: &mut ReducerCtx<TimerState>) {
    if !matches!(ctx.state.pc, TimerPc::Awaiting) {
        return;
    }
    if ctx.state.deadline_ns.is_some() {
        ctx.state.pc = TimerPc::Done;
        ctx.state.fired_key = ctx.state.key.clone();
    }
}
