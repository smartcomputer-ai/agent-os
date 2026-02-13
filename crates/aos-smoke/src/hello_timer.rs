//! Hello Timer demo wired up through AIR JSON assets and the reducer harness.
//!
//! Reducer emits a `timer.set` micro-effect, the harness drains the synthetic
//! receipt, and replay verification proves determinism end-to-end.

use std::path::Path;

use anyhow::{Result, ensure};
use aos_effects::{EffectKind as EffectsEffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::Kernel;
use aos_store::FsStore;
use aos_wasm_sdk::{aos_event_union, aos_variant};
use serde::{Deserialize, Serialize};
use serde_cbor;

use crate::example_host::{ExampleHost, HarnessConfig};

const REDUCER_NAME: &str = "demo/TimerSM@1";
const EVENT_SCHEMA: &str = "demo/TimerEvent@1";
const ADAPTER_ID: &str = "adapter.timer.fake";
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerSetReceipt {
    delivered_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

pub fn run(example_root: &Path) -> Result<()> {
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: "examples/01-hello-timer/reducer",
    })?;

    println!("→ Hello Timer demo");
    println!("     start key=? deliver_ns={DELIVER_AT_NS}");
    let start = TimerEvent::Start(StartEvent {
        deliver_at_ns: DELIVER_AT_NS,
        key: None,
    });
    host.send_event(&start)?;
    synthesize_timer_receipts(host.kernel_mut())?;

    let final_state: TimerState = host.read_state()?;
    println!(
        "   final state: pc={:?}, key={:?}, fired_key={:?}",
        final_state.pc, final_state.key, final_state.fired_key
    );

    host.finish()?.verify_replay()?;
    Ok(())
}

fn synthesize_timer_receipts(kernel: &mut Kernel<FsStore>) -> Result<()> {
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
            kernel.tick_until_idle()?;
            println!("     timer fired (synthetic receipt)");
        }
    }
    Ok(())
}
