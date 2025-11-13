#![allow(improper_ctypes_definitions)]

use aos_effects::builtins::TimerSetParams;
use aos_wasm_abi::{ReducerEffect, ReducerInput, ReducerOutput};
use serde::{Deserialize, Serialize};
use std::alloc::{Layout, alloc as host_alloc};
use std::slice;

const EVENT_SCHEMA: &str = "demo/TimerEvent@1";
const SYS_TIMER_FIRED: &str = "sys/TimerFired@1";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TimerState {
    pc: TimerPc,
    key: Option<String>,
    deadline_ns: Option<u64>,
    fired_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum TimerPc {
    Idle,
    Awaiting,
    Done,
    TimedOut,
}

impl Default for TimerPc {
    fn default() -> Self {
        TimerPc::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartEvent {
    deliver_at_ns: u64,
    key: String,
}

#[cfg_attr(target_arch = "wasm32", unsafe(export_name = "alloc"))]
pub extern "C" fn wasm_alloc(len: i32) -> i32 {
    if len <= 0 {
        return 0;
    }
    let layout = Layout::from_size_align(len as usize, 8).expect("layout");
    unsafe { host_alloc(layout) as i32 }
}

#[cfg_attr(target_arch = "wasm32", unsafe(export_name = "step"))]
pub extern "C" fn wasm_step(ptr: i32, len: i32) -> (i32, i32) {
    let input_bytes = unsafe { slice::from_raw_parts(ptr as *const u8, len as usize) };
    let input = ReducerInput::decode(input_bytes).expect("valid reducer input");

    let mut state = input
        .state
        .map(|bytes| serde_cbor::from_slice::<TimerState>(&bytes).expect("state"))
        .unwrap_or_default();

    let mut effects = Vec::new();
    match input.event.schema.as_str() {
        EVENT_SCHEMA => {
            if let Ok(event) = serde_cbor::from_slice::<StartEvent>(&input.event.value) {
                handle_start(&mut state, event, &mut effects);
            } else {
                handle_timer_fired(&mut state);
            }
        }
        SYS_TIMER_FIRED => {
            handle_timer_fired(&mut state);
        }
        _ => {}
    }

    let state_bytes = serde_cbor::to_vec(&state).expect("encode state");
    let output = ReducerOutput {
        state: Some(state_bytes),
        domain_events: Vec::new(),
        effects,
        ann: None,
    };
    let output_bytes = output.encode().expect("encode output");
    write_back(&output_bytes)
}

fn handle_start(state: &mut TimerState, event: StartEvent, effects: &mut Vec<ReducerEffect>) {
    if matches!(state.pc, TimerPc::Idle | TimerPc::Done | TimerPc::TimedOut) {
        state.pc = if event.deliver_at_ns == 0 {
            TimerPc::Done
        } else {
            TimerPc::Awaiting
        };
        state.key = Some(event.key.clone());
        state.deadline_ns = Some(event.deliver_at_ns);
        state.fired_key = None;

        if let (TimerPc::Awaiting, Some(key), Some(deadline)) =
            (&state.pc, state.key.clone(), state.deadline_ns)
        {
            let params = TimerSetParams {
                deliver_at_ns: deadline,
                key: Some(key),
            };
            let params_bytes = serde_cbor::to_vec(&params).expect("params");
            effects.push(ReducerEffect::with_cap_slot(
                "timer.set",
                params_bytes,
                "timer",
            ));
        }
    }
}

fn handle_timer_fired(state: &mut TimerState) {
    if !matches!(state.pc, TimerPc::Awaiting) {
        return;
    }
    if state.deadline_ns.is_some() {
        state.pc = TimerPc::Done;
        state.fired_key = state.key.clone();
    }
}

fn write_back(bytes: &[u8]) -> (i32, i32) {
    let len = bytes.len() as i32;
    let ptr = wasm_alloc(len);
    unsafe {
        let out = slice::from_raw_parts_mut(ptr as *mut u8, len as usize);
        out.copy_from_slice(bytes);
    }
    (ptr, len)
}
