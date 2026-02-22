//! `aos trace-summary` command.

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
pub struct TraceSummaryArgs {
    /// Write full JSON summary to file
    #[arg(long)]
    pub out: Option<PathBuf>,
}

pub async fn cmd_trace_summary(opts: &WorldOpts, args: &TraceSummaryArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let mut client = try_control_client(&dirs).await.ok_or_else(|| {
        if matches!(opts.mode, Mode::Daemon) {
            anyhow::anyhow!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            )
        } else {
            anyhow::anyhow!(
                "trace-summary requires a running daemon; no control socket at {}",
                dirs.control_socket.display()
            )
        }
    })?;

    let req = RequestEnvelope {
        v: 1,
        id: "cli-trace-summary".into(),
        cmd: "plan-summary".into(),
        payload: json!({}),
    };
    let resp = client.request(&req).await?;
    if !resp.ok {
        anyhow::bail!("plan-summary failed: {:?}", resp.error);
    }

    let summary = resp.result.unwrap_or_else(|| json!({}));
    if let Some(path) = &args.out {
        fs::write(path, serde_json::to_vec_pretty(&summary)?)?;
    }

    if opts.json || opts.pretty {
        return print_success(opts, summary, None, vec![]);
    }

    render_human(&summary);
    Ok(())
}

fn render_human(summary: &Value) {
    let started = summary
        .get("totals")
        .and_then(|v| v.get("runs"))
        .and_then(|v| v.get("started"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let ok = summary
        .get("totals")
        .and_then(|v| v.get("runs"))
        .and_then(|v| v.get("ok"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let err = summary
        .get("totals")
        .and_then(|v| v.get("runs"))
        .and_then(|v| v.get("error"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let timeout_path = summary
        .get("totals")
        .and_then(|v| v.get("failure_signals"))
        .and_then(|v| v.get("timeout_path"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let adapter_error = summary
        .get("totals")
        .and_then(|v| v.get("failure_signals"))
        .and_then(|v| v.get("adapter_error"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    println!(
        "trace-summary: runs started={started} ok={ok} error={err} timeout_path={timeout_path} adapter_error={adapter_error}"
    );

    if let Some(plans) = summary.get("plans").and_then(|v| v.as_array()) {
        for plan in plans {
            let name = plan
                .get("plan_name")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let started = plan
                .get("runs")
                .and_then(|v| v.get("started"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let ok = plan
                .get("runs")
                .and_then(|v| v.get("ok"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let err = plan
                .get("runs")
                .and_then(|v| v.get("error"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            println!("  {name}: started={started} ok={ok} error={err}");
        }
    }
}
