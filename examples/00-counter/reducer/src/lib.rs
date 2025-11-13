#![allow(improper_ctypes_definitions)]

use aos_wasm_abi::{ReducerInput, ReducerOutput};
use serde::{Deserialize, Serialize};
use std::alloc::{Layout, alloc as host_alloc};
use std::slice;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CounterState {
    pc: CounterPc,
    remaining: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CounterPc {
    Idle,
    Counting,
    Done,
}

impl Default for CounterPc {
    fn default() -> Self {
        CounterPc::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CounterEvent {
    Start { target: u64 },
    Tick,
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
        .map(|bytes| serde_cbor::from_slice::<CounterState>(&bytes).expect("state"))
        .unwrap_or_default();
    let event: CounterEvent = serde_cbor::from_slice(&input.event.value).expect("event");

    match event {
        CounterEvent::Start { target } => {
            state.pc = if target == 0 {
                CounterPc::Done
            } else {
                CounterPc::Counting
            };
            state.remaining = target;
        }
        CounterEvent::Tick => {
            if matches!(state.pc, CounterPc::Counting) && state.remaining > 0 {
                state.remaining -= 1;
                if state.remaining == 0 {
                    state.pc = CounterPc::Done;
                }
            }
        }
    }

    let state_bytes = serde_cbor::to_vec(&state).expect("encode state");
    let output = ReducerOutput {
        state: Some(state_bytes),
        domain_events: Vec::new(),
        effects: Vec::new(),
        ann: None,
    };
    let output_bytes = output.encode().expect("encode output");
    write_back(&output_bytes)
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
