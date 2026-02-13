//! `aos trace` command.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use aos_host::control::RequestEnvelope;
use clap::Args;
use serde_json::{Value, json};

use crate::opts::{Mode, WorldOpts, resolve_dirs};
use crate::output::print_success;

use super::try_control_client;

#[derive(Args, Debug)]
pub struct TraceArgs {
    /// Root domain event hash to trace
    #[arg(long)]
    pub event_hash: Option<String>,

    /// Root event schema for correlation mode
    #[arg(long)]
    pub schema: Option<String>,

    /// Correlation field path (example: $value.chat_id)
    #[arg(long)]
    pub correlate_by: Option<String>,

    /// Correlation value as JSON (fallback: plain string)
    #[arg(long)]
    pub value: Option<String>,

    /// Maximum journal records included around the trace
    #[arg(long)]
    pub window_limit: Option<u64>,

    /// Poll until terminal status
    #[arg(long)]
    pub follow: bool,

    /// Poll interval in milliseconds when using --follow
    #[arg(long, default_value_t = 700)]
    pub follow_interval_ms: u64,

    /// Max polls when using --follow
    #[arg(long, default_value_t = 120)]
    pub follow_max_polls: u32,

    /// Write full JSON trace to file
    #[arg(long)]
    pub out: Option<PathBuf>,
}

pub async fn cmd_trace(opts: &WorldOpts, args: &TraceArgs) -> Result<()> {
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
                "trace requires either --event-hash or --schema + --correlate-by + --value"
            );
        }
        _ => {
            anyhow::bail!(
                "trace requires exactly one mode: --event-hash OR --schema + --correlate-by + --value"
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
                "trace requires a running daemon; no control socket at {}",
                dirs.control_socket.display()
            )
        }
    })?;

    let mut polls = 0u32;
    let mut last_state: Option<String> = None;

    let final_result = loop {
        let req = RequestEnvelope {
            v: 1,
            id: format!("cli-trace-{polls}"),
            cmd: "trace-get".into(),
            payload: json!({
                "event_hash": args.event_hash,
                "schema": args.schema,
                "correlate_by": args.correlate_by,
                "value": if args.value.is_some() { Some(correlation_value.clone()) } else { None },
                "window_limit": args.window_limit,
            }),
        };
        let resp = client.request(&req).await?;
        if !resp.ok {
            anyhow::bail!("trace-get failed: {:?}", resp.error);
        }

        let result = resp.result.unwrap_or_else(|| json!({}));
        let state = result
            .get("terminal_state")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        if !args.follow {
            break result;
        }

        if !opts.json && !opts.pretty && last_state.as_deref() != Some(state.as_str()) {
            render_trace_human(&result);
        }

        let should_break = is_terminal_state(&state) || polls + 1 >= args.follow_max_polls;
        polls += 1;

        if should_break {
            break result;
        }

        last_state = Some(state);
        tokio::time::sleep(Duration::from_millis(args.follow_interval_ms)).await;
    };

    if let Some(path) = &args.out {
        fs::write(path, serde_json::to_vec_pretty(&final_result)?)?;
    }

    if opts.json || opts.pretty {
        return print_success(opts, final_result, None, vec![]);
    }

    if !args.follow {
        render_trace_human(&final_result);
    }
    Ok(())
}

fn is_terminal_state(state: &str) -> bool {
    matches!(state, "completed" | "failed")
}

fn render_trace_human(result: &Value) {
    let event_hash = result
        .get("root")
        .and_then(|v| v.get("event_hash"))
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let schema = result
        .get("root")
        .and_then(|v| v.get("schema"))
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let seq = result
        .get("root")
        .and_then(|v| v.get("seq"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let terminal_state = result
        .get("terminal_state")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    println!("trace: {event_hash} schema={schema} seq={seq} terminal={terminal_state}");

    if let Some(wait) = result.get("live_wait") {
        let pending_plan_receipts = wait
            .get("pending_plan_receipts")
            .and_then(|v| v.as_array())
            .map(std::vec::Vec::len)
            .unwrap_or(0);
        let waiting_events = wait
            .get("waiting_events")
            .and_then(|v| v.as_array())
            .map(std::vec::Vec::len)
            .unwrap_or(0);
        let pending_reducer_receipts = wait
            .get("pending_reducer_receipts")
            .and_then(|v| v.as_array())
            .map(std::vec::Vec::len)
            .unwrap_or(0);
        let queued_effects = wait
            .get("queued_effects")
            .and_then(|v| v.as_array())
            .map(std::vec::Vec::len)
            .unwrap_or(0);
        println!(
            "wait: plan_receipts={pending_plan_receipts} waiting_events={waiting_events} reducer_receipts={pending_reducer_receipts} queued_effects={queued_effects}"
        );
    }

    if let Some(entries) = result
        .get("journal_window")
        .and_then(|v| v.get("entries"))
        .and_then(|v| v.as_array())
    {
        for entry in entries {
            let seq = entry.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
            let kind = entry
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            println!("#{seq} {kind}");
        }
    }
}
