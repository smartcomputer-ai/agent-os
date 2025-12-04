//! `aos world event` command.

use anyhow::{Context, Result};
use aos_host::host::ExternalEvent;
use aos_host::modes::batch::BatchRunner;
use clap::Args;
use serde_json::Value as JsonValue;

use crate::input::parse_input_value;
use crate::opts::{resolve_dirs, WorldOpts};
use crate::util::load_world_env;

use super::{create_host, prepare_world, try_control_client};

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
    if let Some(mut client) = try_control_client(&dirs).await {
        let resp = client
            .send_event("cli-event", &args.schema, &cbor)
            .await?;
        if !resp.ok {
            anyhow::bail!("send-event failed: {:?}", resp.error);
        }
        println!("Event enqueued: {}", args.schema);
        return Ok(());
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
    println!(
        "ok (events={} effects={} receipts={})",
        res.events_injected, res.cycle.effects_dispatched, res.cycle.receipts_applied
    );

    Ok(())
}
