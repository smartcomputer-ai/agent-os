//! `aos world event` command.

use anyhow::{Context, Result};
use aos_host::host::ExternalEvent;
use aos_host::modes::batch::BatchRunner;
use clap::Args;
use serde_json::Value as JsonValue;

use crate::input::parse_input_value;
use crate::opts::{resolve_dirs, WorldOpts};
use crate::util::load_world_env;

use super::{create_host, is_daemon_running, prepare_world, try_control_client};

#[derive(Args, Debug)]
pub struct EventArgs {
    /// Event schema (e.g., demo/Increment@1)
    pub schema: String,

    /// Event value: JSON literal, @file, or @- for stdin
    pub value: String,

    /// Run in batch mode: enqueue event, process until quiescent, then exit
    #[arg(long)]
    pub batch: bool,
}

pub async fn cmd_event(opts: &WorldOpts, args: &EventArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    // Parse the event value
    let json_str = parse_input_value(&args.value)?;
    let parsed: JsonValue = serde_json::from_str(&json_str).context("parse event value as JSON")?;
    let cbor = serde_cbor::to_vec(&parsed).context("encode event value as CBOR")?;

    if args.batch {
        // Batch mode: error if daemon running, then enqueue + run until quiescent
        if is_daemon_running(&dirs).await {
            anyhow::bail!(
                "A daemon is already running. --batch requires no daemon to be running."
            );
        }

        // Load world-specific .env
        load_world_env(&dirs.world)?;

        let (store, loaded) = prepare_world(&dirs, opts)?;
        let host = create_host(store, loaded, &dirs, opts)?;
        let mut runner = BatchRunner::new(host);

        // Enqueue the event and run
        let events = vec![ExternalEvent::DomainEvent {
            schema: args.schema.clone(),
            value: cbor,
        }];
        let res = runner.step(events).await?;
        println!(
            "Batch complete: events={} effects={} receipts={}",
            res.events_injected, res.cycle.effects_dispatched, res.cycle.receipts_applied
        );
    } else {
        // Non-batch mode: enqueue event only (via daemon if running, otherwise direct to journal)
        if let Some(mut client) = try_control_client(&dirs).await {
            // Daemon running: send via control channel
            let resp = client
                .send_event("cli-event", &args.schema, &cbor)
                .await?;
            if !resp.ok {
                anyhow::bail!("send-event failed: {:?}", resp.error);
            }
            println!("Event enqueued: {}", args.schema);
        } else {
            // No daemon: write directly to journal
            load_world_env(&dirs.world)?;
            let (store, loaded) = prepare_world(&dirs, opts)?;
            let mut host = create_host(store, loaded, &dirs, opts)?;
            host.enqueue_external(ExternalEvent::DomainEvent {
                schema: args.schema.clone(),
                value: cbor,
            })?;
            println!("Event written to journal: {}", args.schema);
            println!("Run `aos world run` or `aos world run --batch` to process.");
        }
    }

    Ok(())
}
