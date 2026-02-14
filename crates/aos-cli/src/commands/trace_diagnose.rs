//! `aos trace-diagnose` command.

use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use aos_host::control::RequestEnvelope;
use clap::Args;
use serde_json::{Value, json};

use crate::opts::{Mode, WorldOpts, resolve_dirs};
use crate::output::print_success;

use super::try_control_client;

#[derive(Args, Debug)]
pub struct TraceDiagnoseArgs {
    /// Root domain event hash to diagnose
    #[arg(long)]
    pub event_hash: Option<String>,

    /// Correlation mode: event schema
    #[arg(long)]
    pub schema: Option<String>,

    /// Correlation mode: field path (example: $value.request_id)
    #[arg(long)]
    pub correlate_by: Option<String>,

    /// Correlation mode: field value (JSON or plain text)
    #[arg(long)]
    pub value: Option<String>,

    /// Journal window size for trace retrieval
    #[arg(long)]
    pub window_limit: Option<u64>,

    /// Write JSON output to file
    #[arg(long)]
    pub out: Option<PathBuf>,
}

pub async fn cmd_trace_diagnose(opts: &WorldOpts, args: &TraceDiagnoseArgs) -> Result<()> {
    let correlation_value = if let Some(raw) = &args.value {
        serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.clone()))
    } else {
        Value::Null
    };

    match (
        args.event_hash.is_some(),
        args.schema.is_some(),
        args.correlate_by.is_some(),
        args.value.is_some(),
    ) {
        (true, false, false, false) => {}
        (false, true, true, true) => {}
        (false, false, false, false) => {
            anyhow::bail!(
                "trace-diagnose requires either --event-hash or --schema + --correlate-by + --value"
            );
        }
        _ => {
            anyhow::bail!(
                "trace-diagnose requires exactly one mode: --event-hash OR --schema + --correlate-by + --value"
            );
        }
    }

    let dirs = resolve_dirs(opts)?;
    let mut client = try_control_client(&dirs).await.ok_or_else(|| {
        if matches!(opts.mode, Mode::Daemon) {
            anyhow::anyhow!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            )
        } else {
            anyhow::anyhow!(
                "trace-diagnose requires a running daemon; no control socket at {}",
                dirs.control_socket.display()
            )
        }
    })?;

    let req = RequestEnvelope {
        v: 1,
        id: "cli-trace-diagnose".into(),
        cmd: "trace-get".into(),
        payload: json!({
            "event_hash": args.event_hash,
            "schema": args.schema,
            "correlate_by": args.correlate_by,
            "value": if args.value.is_some() { Some(correlation_value) } else { None::<Value> },
            "window_limit": args.window_limit,
        }),
    };

    let resp = client.request(&req).await?;
    if !resp.ok {
        anyhow::bail!("trace-get failed: {:?}", resp.error);
    }
    let trace = resp.result.unwrap_or_else(|| json!({}));

    let diagnosis = diagnose(&trace);
    let result = json!({
        "diagnosis": diagnosis,
        "trace": trace,
    });

    if let Some(path) = &args.out {
        fs::write(path, serde_json::to_vec_pretty(&result)?)?;
    }

    if opts.json || opts.pretty {
        return print_success(opts, result, None, vec![]);
    }

    render_human(&result);
    Ok(())
}

fn diagnose(trace: &Value) -> Value {
    let terminal = trace
        .get("terminal_state")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let entries = trace
        .get("journal_window")
        .and_then(|v| v.get("entries"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let live_wait = trace
        .get("live_wait")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let pending_plan_receipts = live_wait
        .get("pending_plan_receipts")
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0);
    let plan_waiting_receipts = live_wait
        .get("plan_waiting_receipts")
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0);
    let plan_waiting_events = live_wait
        .get("plan_waiting_events")
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0);
    let pending_reducer_receipts = live_wait
        .get("pending_reducer_receipts")
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0);
    let queued_effects = live_wait
        .get("queued_effects")
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0);

    let mut policy_denied = false;
    let mut capability_denied = false;
    let mut adapter_error = false;
    let mut adapter_timeout = false;
    let mut plan_error = false;
    let mut invariant_violation = false;

    for entry in entries {
        let kind = entry
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let record = entry.get("record").cloned().unwrap_or(Value::Null);

        match kind {
            "policy_decision" => {
                let decision = find_str(&record, "decision").unwrap_or_default();
                if decision.eq_ignore_ascii_case("deny") {
                    policy_denied = true;
                }
            }
            "cap_decision" => {
                let decision = find_str(&record, "decision").unwrap_or_default();
                if decision.eq_ignore_ascii_case("deny") {
                    capability_denied = true;
                }
            }
            "effect_receipt" => {
                let status = find_str(&record, "status").unwrap_or_default();
                if status.eq_ignore_ascii_case("error") {
                    adapter_error = true;
                }
                if status.eq_ignore_ascii_case("timeout") {
                    adapter_timeout = true;
                }
            }
            "plan_ended" => {
                let status = find_str(&record, "status").unwrap_or_default();
                if status.eq_ignore_ascii_case("error") {
                    let error_code = find_str(&record, "error_code").unwrap_or_default();
                    if error_code.eq_ignore_ascii_case("invariant_violation") {
                        invariant_violation = true;
                    } else {
                        plan_error = true;
                    }
                }
            }
            _ => {}
        }
    }

    let cause = if policy_denied {
        "policy_denied"
    } else if capability_denied {
        "capability_denied"
    } else if adapter_timeout {
        "adapter_timeout"
    } else if adapter_error {
        "adapter_error"
    } else if invariant_violation {
        "invariant_violation"
    } else if plan_error {
        "unknown_failure"
    } else if terminal == "waiting_receipt" {
        "waiting_receipt"
    } else if terminal == "waiting_event" {
        "waiting_event"
    } else if terminal == "completed" {
        "completed"
    } else {
        "unknown"
    };

    let hint = match cause {
        "policy_denied" => "Adjust policy rules or plan origin/cap mapping.",
        "capability_denied" => "Inspect capability grant constraints and enforcer output.",
        "adapter_timeout" => "Check adapter timeout and upstream endpoint latency.",
        "adapter_error" => "Inspect adapter receipt payload for failure details.",
        "invariant_violation" => "Inspect plan invariants and local/step value assumptions.",
        "unknown_failure" => "Inspect plan/runtime records to identify the failure source.",
        "waiting_receipt" => "Flow is waiting for effect receipts or queued effect execution.",
        "waiting_event" => "Flow is waiting for a follow-up domain event.",
        "completed" => "Flow completed successfully.",
        _ => "Insufficient signal; inspect full trace timeline.",
    };

    let event_hash = trace
        .get("root")
        .and_then(|v| v.get("event_hash"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            trace
                .get("query")
                .and_then(|v| v.get("event_hash"))
                .and_then(|v| v.as_str())
        });

    let intent_hash = first_intent_hash(trace.get("live_wait").unwrap_or(&Value::Null));

    json!({
        "terminal_state": terminal,
        "cause": cause,
        "hint": hint,
        "hashes": {
            "event_hash": event_hash,
            "intent_hash": intent_hash,
        },
        "waits": {
            "pending_plan_receipts": pending_plan_receipts,
            "plan_waiting_receipts": plan_waiting_receipts,
            "plan_waiting_events": plan_waiting_events,
            "pending_reducer_receipts": pending_reducer_receipts,
            "queued_effects": queued_effects,
        }
    })
}

fn first_intent_hash(value: &Value) -> Option<String> {
    let read = |arr: Option<&Vec<Value>>| -> Option<String> {
        let arr = arr?;
        for item in arr {
            if let Some(hash) = item.get("intent_hash").and_then(|v| v.as_str()) {
                return Some(hash.to_string());
            }
            if let Some(hashes) = item.get("intent_hashes").and_then(|v| v.as_array()) {
                if let Some(hash) = hashes.first().and_then(|v| v.as_str()) {
                    return Some(hash.to_string());
                }
            }
        }
        None
    };

    read(
        value
            .get("pending_plan_receipts")
            .and_then(|v| v.as_array()),
    )
    .or_else(|| {
        read(
            value
                .get("pending_reducer_receipts")
                .and_then(|v| v.as_array()),
        )
    })
    .or_else(|| {
        read(
            value
                .get("plan_waiting_receipts")
                .and_then(|v| v.as_array()),
        )
    })
}

fn find_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match value {
        Value::Object(map) => {
            if let Some(v) = map.get(key).and_then(Value::as_str) {
                return Some(v);
            }
            for v in map.values() {
                if let Some(found) = find_str(v, key) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => {
            for item in items {
                if let Some(found) = find_str(item, key) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn render_human(result: &Value) {
    let diagnosis = result.get("diagnosis").cloned().unwrap_or(Value::Null);
    let terminal = diagnosis
        .get("terminal_state")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let cause = diagnosis
        .get("cause")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let hint = diagnosis
        .get("hint")
        .and_then(|v| v.as_str())
        .unwrap_or("-");

    println!("trace-diagnose: terminal={terminal} cause={cause}");
    println!("hint: {hint}");

    let event_hash = diagnosis
        .get("hashes")
        .and_then(|v| v.get("event_hash"))
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let intent_hash = diagnosis
        .get("hashes")
        .and_then(|v| v.get("intent_hash"))
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    println!("event_hash={event_hash}");
    println!("intent_hash={intent_hash}");
}

#[cfg(test)]
mod tests {
    use super::diagnose;
    use serde_json::json;

    fn trace_with_entries(entries: Vec<serde_json::Value>) -> serde_json::Value {
        json!({
            "terminal_state": "failed",
            "journal_window": { "entries": entries },
            "live_wait": {}
        })
    }

    #[test]
    fn diagnose_policy_denied() {
        let trace = trace_with_entries(vec![json!({
            "kind": "policy_decision",
            "record": { "decision": "deny" }
        })]);
        let diagnosis = diagnose(&trace);
        assert_eq!(diagnosis["cause"], "policy_denied");
    }

    #[test]
    fn diagnose_capability_denied() {
        let trace = trace_with_entries(vec![json!({
            "kind": "cap_decision",
            "record": { "decision": "deny" }
        })]);
        let diagnosis = diagnose(&trace);
        assert_eq!(diagnosis["cause"], "capability_denied");
    }

    #[test]
    fn diagnose_adapter_timeout() {
        let trace = trace_with_entries(vec![json!({
            "kind": "effect_receipt",
            "record": { "status": "timeout" }
        })]);
        let diagnosis = diagnose(&trace);
        assert_eq!(diagnosis["cause"], "adapter_timeout");
    }

    #[test]
    fn diagnose_adapter_error() {
        let trace = trace_with_entries(vec![json!({
            "kind": "effect_receipt",
            "record": { "status": "error" }
        })]);
        let diagnosis = diagnose(&trace);
        assert_eq!(diagnosis["cause"], "adapter_error");
    }

    #[test]
    fn diagnose_invariant_violation() {
        let trace = trace_with_entries(vec![json!({
            "kind": "plan_ended",
            "record": { "status": "error", "error_code": "invariant_violation" }
        })]);
        let diagnosis = diagnose(&trace);
        assert_eq!(diagnosis["cause"], "invariant_violation");
    }

    #[test]
    fn diagnose_unknown_failure_for_generic_plan_error() {
        let trace = trace_with_entries(vec![json!({
            "kind": "plan_ended",
            "record": { "status": "error", "error_code": "other_error" }
        })]);
        let diagnosis = diagnose(&trace);
        assert_eq!(diagnosis["cause"], "unknown_failure");
    }
}
