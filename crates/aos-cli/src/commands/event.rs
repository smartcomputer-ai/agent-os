//! `aos event send` command.

use anyhow::{Context, Result};
use aos_host::host::ExternalEvent;
use aos_host::modes::batch::BatchRunner;
use clap::Args;
use serde_json::Value as JsonValue;

use crate::input::parse_input_value;
use crate::opts::{Mode, WorldOpts, resolve_dirs};
use crate::output::print_success;
use crate::util::load_world_env;

use super::{create_host, prepare_world, should_use_control, try_control_client};

#[derive(Args, Debug)]
pub struct EventArgs {
    /// Event schema (e.g., demo/Increment@1)
    pub schema: String,

    /// Event value: JSON literal, @file, or @- for stdin
    pub value: String,
}

pub async fn cmd_event(opts: &WorldOpts, args: &EventArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    // Parse the event value
    let json_str = parse_input_value(&args.value)?;
    let parsed: JsonValue = serde_json::from_str(&json_str).context("parse event value as JSON")?;
    let cbor = serde_cbor::to_vec(&parsed).context("encode event value as CBOR")?;

    // If daemon is running, send via control channel (enqueue only, daemon processes)
    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let resp = client.send_event("cli-event", &args.schema, &cbor).await?;
            if !resp.ok {
                anyhow::bail!("event-send failed: {:?}", resp.error);
            }
            return print_success(
                opts,
                serde_json::json!({ "enqueued": args.schema }),
                None,
                vec![],
            );
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        } else if !opts.quiet {
            // fall through to batch
        }
    }

    // No daemon: run in batch mode (enqueue + process until quiescent)
    load_world_env(&dirs.world)?;

    let (store, loaded) = prepare_world(&dirs, opts)?;
    let host = create_host(store, loaded, &dirs, opts)?;
    let mut runner = BatchRunner::new(host);

    let events = vec![ExternalEvent::DomainEvent {
        schema: args.schema.clone(),
        value: cbor,
    }];
    let res = runner.step(events).await?;
    print_success(
        opts,
        serde_json::json!({
            "status": "ok",
            "events": res.events_injected,
            "effects": res.cycle.effects_dispatched,
            "receipts": res.cycle.receipts_applied
        }),
        None,
        if opts.quiet {
            vec![]
        } else {
            vec!["daemon unavailable; using batch mode".into()]
        },
    )
}
