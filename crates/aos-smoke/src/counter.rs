//! Counter demo wired up through AIR JSON assets and the reducer harness.
//!
//! This is a minimal example with no micro-effects, showing how to load
//! a manifest from AIR JSON assets and drive a reducer through events.

use std::path::Path;

use anyhow::Result;
use aos_wasm_sdk::aos_variant;
use serde::{Deserialize, Serialize};

use crate::example_host::{ExampleHost, HarnessConfig};

const REDUCER_NAME: &str = "demo/CounterSM@1";
const EVENT_SCHEMA: &str = "demo/CounterEvent@1";
const TARGET_COUNT: u64 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct CounterState {
    pc: CounterPc,
    remaining: u64,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

pub fn run(example_root: &Path) -> Result<()> {
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: "examples/00-counter/reducer",
    })?;

    println!("→ Counter demo (target {TARGET_COUNT})");
    println!("     start (target {TARGET_COUNT})");
    host.send_event(&CounterEvent::Start {
        target: TARGET_COUNT,
    })?;

    for tick in 1..=TARGET_COUNT {
        host.send_event(&CounterEvent::Tick)?;
        println!("     tick #{tick}");
    }

    host.run_cycle_batch()?;

    let final_state: CounterState = host.read_state()?;
    println!(
        "   final state: pc={:?}, remaining={}",
        final_state.pc, final_state.remaining
    );

    host.finish()?.verify_replay()?;
    Ok(())
}
