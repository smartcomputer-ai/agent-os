use std::path::Path;

use anyhow::{Result, anyhow, ensure};
use aos_effects::builtins::{HeaderMap, HttpRequestReceipt, RequestTimings};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_host::trace::{TraceQuery, diagnose_trace, trace_get};
use aos_kernel::journal::{JournalKind, JournalRecord};
use aos_wasm_sdk::aos_variant;
use serde::{Deserialize, Serialize};

use crate::example_host::{ExampleHost, HarnessConfig};

const REDUCER_NAME: &str = "demo/FetchNotify@1";
const EVENT_SCHEMA: &str = "demo/FetchNotifyEvent@1";
const MODULE_CRATE: &str = "crates/aos-smoke/fixtures/10-trace-failure-classification/reducer";

const AIR_ALLOW: &str = "crates/aos-smoke/fixtures/10-trace-failure-classification/air.allow";
const AIR_CAP_DENY: &str = "crates/aos-smoke/fixtures/10-trace-failure-classification/air.cap_deny";
const AIR_POLICY_DENY: &str =
    "crates/aos-smoke/fixtures/10-trace-failure-classification/air.policy_deny";

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum FetchEventEnvelope {
        Start { url: String, method: String },
    }
}

pub fn run(example_root: &Path) -> Result<()> {
    println!("→ Trace Failure Classification demo");

    run_policy_denied(example_root)?;
    run_capability_denied(example_root)?;
    run_adapter_timeout(example_root)?;
    run_adapter_error(example_root)?;

    Ok(())
}

fn run_policy_denied(example_root: &Path) -> Result<()> {
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: Some(Path::new(AIR_POLICY_DENY)),
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_CRATE,
    })?;

    let _ = host
        .send_event(&start_event("https://example.com/policy-denied.json"))
        .expect_err("policy-denied path should fail event processing");
    assert_trace_cause(&mut host, "policy_denied")?;
    println!("   policy/cap: policy_denied");
    Ok(())
}

fn run_capability_denied(example_root: &Path) -> Result<()> {
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: Some(Path::new(AIR_CAP_DENY)),
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_CRATE,
    })?;

    let _ = host
        .send_event(&start_event("https://example.com/cap-denied.json"))
        .expect_err("capability-denied path should fail event processing");
    assert_trace_cause(&mut host, "capability_denied")?;
    println!("   policy/cap: capability_denied");
    Ok(())
}

fn run_adapter_timeout(example_root: &Path) -> Result<()> {
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: Some(Path::new(AIR_ALLOW)),
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_CRATE,
    })?;

    host.send_event(&start_event("https://example.com/timeout.json"))?;
    let intents = host.drain_effects()?;
    ensure!(
        intents.len() == 1,
        "expected one effect intent for timeout path, got {}",
        intents.len()
    );
    let intent = intents.into_iter().next().expect("one intent");
    host.apply_receipt(EffectReceipt {
        intent_hash: intent.intent_hash,
        adapter_id: "http.mock".into(),
        status: ReceiptStatus::Timeout,
        payload_cbor: serde_cbor::to_vec(&http_receipt_payload(504)?)?,
        cost_cents: Some(0),
        signature: vec![0; 64],
    })?;

    assert_trace_cause(&mut host, "adapter_timeout")?;
    println!("   adapter: adapter_timeout");
    Ok(())
}

fn run_adapter_error(example_root: &Path) -> Result<()> {
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: Some(Path::new(AIR_ALLOW)),
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_CRATE,
    })?;

    host.send_event(&start_event("https://example.com/error.json"))?;
    let intents = host.drain_effects()?;
    ensure!(
        intents.len() == 1,
        "expected one effect intent for error path, got {}",
        intents.len()
    );
    let intent = intents.into_iter().next().expect("one intent");
    host.apply_receipt(EffectReceipt {
        intent_hash: intent.intent_hash,
        adapter_id: "http.mock".into(),
        status: ReceiptStatus::Error,
        payload_cbor: serde_cbor::to_vec(&http_receipt_payload(500)?)?,
        cost_cents: Some(0),
        signature: vec![0; 64],
    })?;

    assert_trace_cause(&mut host, "adapter_error")?;
    println!("   adapter: adapter_error");
    Ok(())
}

fn assert_trace_cause(host: &mut ExampleHost, expected_cause: &str) -> Result<()> {
    let root_event_hash = latest_start_event_hash(host)?;
    let trace = trace_get(
        host.kernel_mut(),
        TraceQuery {
            event_hash: Some(root_event_hash),
            window_limit: Some(256),
            ..TraceQuery::default()
        },
    )?;
    let diagnosis = diagnose_trace(&trace);
    let cause = diagnosis
        .get("cause")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    ensure!(
        cause == expected_cause,
        "expected trace cause={expected_cause}, got {cause}: {}",
        serde_json::to_string_pretty(&diagnosis)?
    );
    Ok(())
}

fn latest_start_event_hash(host: &mut ExampleHost) -> Result<String> {
    let entries = host.kernel_mut().dump_journal()?;
    for entry in entries.into_iter().rev() {
        if entry.kind != JournalKind::DomainEvent {
            continue;
        }
        let record: JournalRecord = serde_cbor::from_slice(&entry.payload)?;
        let JournalRecord::DomainEvent(domain) = record else {
            continue;
        };
        if domain.schema != EVENT_SCHEMA {
            continue;
        }
        let value: serde_json::Value = serde_cbor::from_slice(&domain.value)?;
        if value.get("$tag").and_then(|v| v.as_str()) == Some("Start") {
            return Ok(domain.event_hash);
        }
    }
    Err(anyhow!("failed to locate root Start event hash"))
}

fn start_event(url: &str) -> FetchEventEnvelope {
    FetchEventEnvelope::Start {
        url: url.into(),
        method: "GET".into(),
    }
}

fn http_receipt_payload(status: i32) -> Result<HttpRequestReceipt> {
    Ok(HttpRequestReceipt {
        status,
        headers: HeaderMap::new(),
        body_ref: None,
        timings: RequestTimings {
            start_ns: 10,
            end_ns: 20,
        },
        adapter_id: "http.mock".into(),
    })
}
