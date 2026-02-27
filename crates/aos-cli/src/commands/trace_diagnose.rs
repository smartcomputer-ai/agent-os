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
    aos_host::trace::diagnose_trace(trace)
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
    fn diagnose_invariant_violation_from_legacy_record() {
        let trace = trace_with_entries(vec![json!({
            "kind": "legacy_plan_ended",
            "record": { "status": "error", "error_code": "invariant_violation" }
        })]);
        let diagnosis = diagnose(&trace);
        assert_eq!(diagnosis["cause"], "invariant_violation");
    }

    #[test]
    fn diagnose_unknown_failure_for_generic_legacy_error() {
        let trace = trace_with_entries(vec![json!({
            "kind": "legacy_plan_ended",
            "record": { "status": "error", "error_code": "other_error" }
        })]);
        let diagnosis = diagnose(&trace);
        assert_eq!(diagnosis["cause"], "unknown_failure");
    }
}
