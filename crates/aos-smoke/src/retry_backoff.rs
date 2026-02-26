//! Workflow-driven retry with exponential backoff using receipt envelopes and timer.set.

use std::path::Path;

use anyhow::{Result, anyhow, ensure};
use aos_effects::builtins::{
    HttpRequestParams, HttpRequestReceipt, RequestTimings, TimerSetParams, TimerSetReceipt,
};
use aos_effects::{EffectKind as EffectsEffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::Kernel;
use aos_store::FsStore;
use aos_wasm_sdk::aos_variant;
use serde::{Deserialize, Serialize};

use crate::example_host::{ExampleHost, HarnessConfig};

const REDUCER_NAME: &str = "demo/RetrySM@1";
const EVENT_SCHEMA: &str = "demo/RetryEvent@1";
const MODULE_CRATE: &str = "crates/aos-smoke/fixtures/08-retry-backoff/reducer";
const ADAPTER_ID_HTTP: &str = "adapter.http.fake";
const ADAPTER_ID_TIMER: &str = "adapter.timer.fake";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartWork {
    req_id: String,
    payload: String,
    max_attempts: u32,
    base_delay_ms: u64,
    now_ns: u64,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum RetryEvent {
        Start(StartWork),
    }
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    enum Pc {
        Idle,
        Requesting,
        Backoff,
        Done,
        Failed,
    }
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
    pending_request: bool,
    last_status: Option<i64>,
    timers_scheduled: u32,
}

pub fn run(example_root: &Path) -> Result<()> {
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_CRATE,
    })?;

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
    host.send_event(&RetryEvent::Start(start))?;

    drive_retry_flow(host.kernel_mut())?;

    let final_state: RetryState = host.read_state()?;
    println!(
        "   final state: pc={:?}, attempt={}, timers={}, last_status={:?}",
        final_state.pc, final_state.attempt, final_state.timers_scheduled, final_state.last_status
    );
    ensure!(
        final_state.pc == Pc::Done,
        "expected retry workflow to finish Done, got {:?}",
        final_state.pc
    );
    ensure!(
        final_state.attempt == 3,
        "expected 3 attempts, got {}",
        final_state.attempt
    );
    ensure!(
        final_state.timers_scheduled == 2,
        "expected 2 scheduled timers, got {}",
        final_state.timers_scheduled
    );
    ensure!(
        final_state.last_status == Some(200),
        "expected final status 200, got {:?}",
        final_state.last_status
    );

    host.finish()?.verify_replay()?;
    Ok(())
}

fn drive_retry_flow(kernel: &mut Kernel<FsStore>) -> Result<()> {
    let mut safety = 0;
    let mut http_attempts = 0;
    let mut scheduled_timers = Vec::new();

    loop {
        let intents = kernel.drain_effects()?;
        if intents.is_empty() {
            break;
        }
        for intent in intents {
            match intent.kind.as_str() {
                EffectsEffectKind::HTTP_REQUEST => {
                    let params: HttpRequestParams = serde_cbor::from_slice(&intent.params_cbor)?;
                    http_attempts += 1;
                    let status = if http_attempts < 3 { 503 } else { 200 };
                    println!(
                        "     http.request attempt={} {} {} -> {}",
                        http_attempts, params.method, params.url, status
                    );
                    let receipt_payload = HttpRequestReceipt {
                        status,
                        headers: Default::default(),
                        body_ref: None,
                        timings: RequestTimings {
                            start_ns: 0,
                            end_ns: 0,
                        },
                        adapter_id: ADAPTER_ID_HTTP.into(),
                    };
                    kernel.handle_receipt(EffectReceipt {
                        intent_hash: intent.intent_hash,
                        adapter_id: ADAPTER_ID_HTTP.into(),
                        status: ReceiptStatus::Ok,
                        payload_cbor: serde_cbor::to_vec(&receipt_payload)?,
                        cost_cents: Some(0),
                        signature: vec![0; 64],
                    })?;
                    kernel.tick_until_idle()?;
                }
                EffectsEffectKind::TIMER_SET => {
                    let params: TimerSetParams = serde_cbor::from_slice(&intent.params_cbor)?;
                    println!(
                        "     timer.set -> key={:?} deliver_ns={}",
                        params.key, params.deliver_at_ns
                    );
                    scheduled_timers.push(params.deliver_at_ns);
                    let receipt_payload = TimerSetReceipt {
                        delivered_at_ns: params.deliver_at_ns,
                        key: params.key.clone(),
                    };
                    kernel.handle_receipt(EffectReceipt {
                        intent_hash: intent.intent_hash,
                        adapter_id: ADAPTER_ID_TIMER.into(),
                        status: ReceiptStatus::Ok,
                        payload_cbor: serde_cbor::to_vec(&receipt_payload)?,
                        cost_cents: Some(0),
                        signature: vec![0; 64],
                    })?;
                    kernel.tick_until_idle()?;
                }
                other => return Err(anyhow!("unexpected effect kind {other}")),
            }
        }
        safety += 1;
        ensure!(safety < 32, "safety trip: too many retry cycles");
    }

    ensure!(
        http_attempts == 3,
        "expected 3 http attempts, got {http_attempts}"
    );
    ensure!(
        scheduled_timers == vec![11_000_000, 21_000_000],
        "unexpected timer schedule {:?}",
        scheduled_timers
    );
    Ok(())
}
