//! Hello Timer demo wired up through AIR JSON assets and the workflow harness.
//!
//! Workflow emits a `timer.set` micro-effect, the harness drains the synthetic
//! receipt, and replay verification proves determinism end-to-end.

use std::path::Path;

use anyhow::{Result, ensure};
use aos_wasm_sdk::{aos_event_union, aos_variant};
use serde::{Deserialize, Serialize};

use crate::example_host::{ExampleHost, HarnessConfig};

const WORKFLOW_NAME: &str = "demo/TimerSM@1";
const EVENT_SCHEMA: &str = "demo/TimerEvent@1";
const DELIVER_AT_NS: u64 = 1_000_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct TimerState {
    pc: TimerPc,
    key: Option<String>,
    deadline_ns: Option<u64>,
    fired_key: Option<String>,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

// Tagged vs record union: Start is a variant, Fired is a record receipt.
aos_event_union! {
    #[derive(Debug, Clone, Serialize)]
    enum TimerEvent {
        Start(StartEvent),
        Fired(TimerFiredEvent)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerSetParams {
    deliver_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

pub fn run(example_root: &Path) -> Result<()> {
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        workflow_name: WORKFLOW_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: "crates/aos-smoke/fixtures/01-hello-timer/workflow",
    })?;

    println!("→ Hello Timer demo");
    println!("     start key=? deliver_ns={DELIVER_AT_NS}");
    host.time_set(0);
    let start = TimerEvent::Start(StartEvent {
        deliver_at_ns: DELIVER_AT_NS,
        key: None,
    });
    host.send_event(&start)?;
    host.time_set(0);
    let cycle = host.run_cycle_with_timers()?;
    ensure!(
        cycle.effects_dispatched == 1 && cycle.receipts_applied == 0,
        "expected queued timer to schedule without firing immediately, got {:?}",
        cycle
    );
    let status = host.quiescence_status();
    ensure!(
        status.timers_pending == 1 && status.next_timer_deadline_ns == Some(DELIVER_AT_NS),
        "unexpected timer status {:?}",
        status
    );
    println!("     timer.set scheduled deliver_ns={DELIVER_AT_NS}");
    let jumped = host.time_jump_next_due()?;
    ensure!(
        jumped == Some(DELIVER_AT_NS),
        "expected next timer due at {DELIVER_AT_NS}, got {:?}",
        jumped
    );
    println!("     timer fired via logical-time jump");

    let final_state: TimerState = host.read_state()?;
    println!(
        "   final state: pc={:?}, key={:?}, fired_key={:?}",
        final_state.pc, final_state.key, final_state.fired_key
    );
    ensure!(
        final_state.pc == TimerPc::Done
            && final_state.deadline_ns == Some(DELIVER_AT_NS)
            && final_state.fired_key.is_none(),
        "unexpected final timer state {:?}",
        final_state
    );

    host.finish()?.verify_replay()?;
    Ok(())
}
