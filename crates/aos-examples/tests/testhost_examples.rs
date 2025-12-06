use std::sync::Arc;

use aos_host::fixtures::{self, TestStore};
use aos_host::testhost::TestHost;
use aos_wasm_abi::{ReducerEffect, ReducerOutput};
use serde::{Deserialize, Serialize};
use serde_cbor;
use serde_json;

// Counter-like flow using TestHost and in-crate fixtures.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct CounterState {
    pc: CounterPc,
    remaining: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[tokio::test]
async fn counter_example_via_testhost() {
    let store = fixtures::new_mem_store();

    // Stub reducer that yields the done state we expect.
    let final_state = CounterState {
        pc: CounterPc::Done,
        remaining: 0,
    };
    let output = ReducerOutput {
        state: Some(serde_cbor::to_vec(&final_state).unwrap()),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };
    let module =
        fixtures::stub_reducer_module(&store, "demo/CounterSM@1", &output);

    let loaded = fixtures::build_loaded_manifest(
        vec![],
        vec![],
        vec![module],
        vec![fixtures::routing_event(
            "demo/CounterEvent@1",
            "demo/CounterSM@1",
        )],
    );

    let mut host = TestHost::from_loaded_manifest(store, loaded).unwrap();
    host.send_event(
        "demo/CounterEvent@1",
        serde_json::json!({ "Start": { "target": 3 } }),
    )
    .unwrap();
    host.run_cycle_batch().await.unwrap();

    let state: CounterState = host.state("demo/CounterSM@1").unwrap();
    assert_eq!(state.pc, CounterPc::Done);
}

// Hello-timer style flow that emits a timer.set; run via daemon-style helper.
#[tokio::test]
async fn hello_timer_example_via_testhost() {
    let store: Arc<TestStore> = fixtures::new_mem_store();

    let timer_params = serde_cbor::to_vec(&serde_json::json!({
        "deliver_at_ns": 42u64,
        "key": "demo"
    }))
    .unwrap();
    let effect = ReducerEffect::with_cap_slot(
        aos_effects::EffectKind::TIMER_SET,
        timer_params,
        "default",
    );
    let output = ReducerOutput {
        state: Some(vec![0x01]),
        domain_events: vec![],
        effects: vec![effect],
        ann: None,
    };
    let module =
        fixtures::stub_reducer_module(&store, "demo/HelloTimer@1", &output);
    let loaded = fixtures::build_loaded_manifest(
        vec![],
        vec![],
        vec![module],
        vec![fixtures::routing_event(
            "demo/HelloTimerEvent@1",
            "demo/HelloTimer@1",
        )],
    );

    let mut host = TestHost::from_loaded_manifest(store, loaded).unwrap();
    host.send_event(
        "demo/HelloTimerEvent@1",
        serde_json::json!({ "Start": {} }),
    )
    .unwrap();

    let cycle = host.run_cycle_with_timers().await.unwrap();
    assert_eq!(cycle.effects_dispatched, 1);
    assert_eq!(cycle.receipts_applied, 1);
}
