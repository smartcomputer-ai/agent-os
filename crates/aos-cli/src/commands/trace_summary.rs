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
        cmd: "trace-summary".into(),
        payload: json!({}),
    };
    let resp = client.request(&req).await?;
    if !resp.ok {
        anyhow::bail!("trace-summary failed: {:?}", resp.error);
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
    let intents = summary
        .get("totals")
        .and_then(|v| v.get("effects"))
        .and_then(|v| v.get("intents"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let ok = summary
        .get("totals")
        .and_then(|v| v.get("effects"))
        .and_then(|v| v.get("receipts"))
        .and_then(|v| v.get("ok"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let err = summary
        .get("totals")
        .and_then(|v| v.get("effects"))
        .and_then(|v| v.get("receipts"))
        .and_then(|v| v.get("error"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let timeout = summary
        .get("totals")
        .and_then(|v| v.get("effects"))
        .and_then(|v| v.get("receipts"))
        .and_then(|v| v.get("timeout"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let waiting = summary
        .get("totals")
        .and_then(|v| v.get("workflows"))
        .and_then(|v| v.get("waiting"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    println!(
        "trace-summary: intents={intents} receipts(ok={ok} error={err} timeout={timeout}) waiting_workflows={waiting}"
    );
}
