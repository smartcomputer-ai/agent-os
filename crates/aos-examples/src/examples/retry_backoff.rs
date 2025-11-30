//! Reducer-driven retry with exponential backoff using timer.set micro-effects.

use std::path::Path;

use anyhow::{Result, ensure};
use aos_effects::{EffectKind as EffectsEffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::Kernel;
use aos_store::FsStore;
use serde::{Deserialize, Serialize};
use serde_cbor;

use crate::support::reducer_harness::{ExampleReducerHarness, HarnessConfig};

const REDUCER_NAME: &str = "demo/RetrySM@1";
const START_EVENT_SCHEMA: &str = "demo/StartWork@1";
const MODULE_CRATE: &str = "examples/08-retry-backoff/reducer";
const ADAPTER_ID: &str = "adapter.timer.fake";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartWork {
    req_id: String,
    payload: String,
    max_attempts: u32,
    base_delay_ms: u64,
    now_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerSetParams {
    deliver_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerSetReceipt {
    delivered_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
enum Pc {
    Idle,
    Waiting,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RetryState {
    pc: Pc,
    attempt: u32,
    max_attempts: u32,
    base_delay_ms: u64,
    anchor_ns: u64,
    payload: String,
    req_id: String,
}

pub fn run(example_root: &Path) -> Result<()> {
    let harness = ExampleReducerHarness::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        reducer_name: REDUCER_NAME,
        event_schema: START_EVENT_SCHEMA,
        module_crate: MODULE_CRATE,
    })?;
    let mut run = harness.start()?;

    println!("→ Retry Backoff demo");
    let start = StartWork {
        req_id: "req-123".into(),
        payload: "do_work".into(),
        max_attempts: 3,
        base_delay_ms: 10,
        now_ns: 1_000_000,
    };
    println!(
        "     start req_id={} max_attempts={} base_delay_ms={}",
        start.req_id, start.max_attempts, start.base_delay_ms
    );
    run.submit_event(&start)?;

    synthesize_timer_receipts(run.kernel_mut())?;

    let final_state: RetryState = run.read_state()?;
    println!(
        "   final state: pc={:?}, attempt={}, req_id={}",
        final_state.pc, final_state.attempt, final_state.req_id
    );

    run.finish()?.verify_replay()?;
    Ok(())
}

fn synthesize_timer_receipts(kernel: &mut Kernel<FsStore>) -> Result<()> {
    let mut safety = 0;
    loop {
        let intents = kernel.drain_effects();
        if intents.is_empty() {
            break;
        }
        for intent in intents {
            ensure!(
                intent.kind.as_str() == EffectsEffectKind::TIMER_SET,
                "unexpected effect {:?}",
                intent.kind
            );
            let params: TimerSetParams = serde_cbor::from_slice(&intent.params_cbor)?;
            println!(
                "     timer.set -> key={:?} deliver_ns={}",
                params.key, params.deliver_at_ns
            );
            let receipt_payload = TimerSetReceipt {
                delivered_at_ns: params.deliver_at_ns,
                key: params.key.clone(),
            };
            let receipt = EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: ADAPTER_ID.into(),
                status: ReceiptStatus::Ok,
                payload_cbor: serde_cbor::to_vec(&receipt_payload)?,
                cost_cents: Some(0),
                signature: vec![0; 64],
            };
            kernel.handle_receipt(receipt)?;
        }
        safety += 1;
        ensure!(safety < 16, "safety trip: too many retry cycles");
    }
    Ok(())
}
