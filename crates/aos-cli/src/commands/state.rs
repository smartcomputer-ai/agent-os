//! `aos world state` command.

use anyhow::{Context, Result};
use base64::Engine;
use clap::Args;
use serde_json::Value as JsonValue;

use crate::opts::{WorldOpts, resolve_dirs};
use crate::util::load_world_env;

use super::{create_host, prepare_world, try_control_client};

#[derive(Args, Debug)]
pub struct StateArgs {
    /// Reducer name (e.g., demo/Counter@1)
    pub reducer_name: String,

    /// Key for keyed reducers (future)
    #[arg(long)]
    pub key: Option<String>,

    /// Output raw JSON without formatting
    #[arg(long)]
    pub raw: bool,
}

pub async fn cmd_state(opts: &WorldOpts, args: &StateArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    // Try daemon first
    if let Some(mut client) = try_control_client(&dirs).await {
        let resp = client.query_state("cli-state", &args.reducer_name).await?;
        if !resp.ok {
            anyhow::bail!("query-state failed: {:?}", resp.error);
        }

        // Extract state from response
        if let Some(result) = resp.result {
            if let Some(state_b64) = result.get("state_b64").and_then(|v| v.as_str()) {
                let state_bytes = base64::engine::general_purpose::STANDARD
                    .decode(state_b64)
                    .context("decode state base64")?;
                print_state(&state_bytes, args.raw)?;
            } else {
                println!("(no state)");
            }
        } else {
            println!("(no state)");
        }
        return Ok(());
    }

    // Fall back to batch mode
    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let host = create_host(store, loaded, &dirs, opts)?;

    // Query state directly from host
    let key_bytes = args.key.as_ref().map(|k| k.as_bytes());
    if let Some(state) = host.state(&args.reducer_name, key_bytes) {
        print_state(state, args.raw)?;
    } else {
        println!("(no state for reducer '{}')", args.reducer_name);
    }

    Ok(())
}

fn print_state(state: &[u8], raw: bool) -> Result<()> {
    // Try to decode as CBOR -> JSON
    match serde_cbor::from_slice::<JsonValue>(state) {
        Ok(json) => {
            if raw {
                println!("{}", serde_json::to_string(&json)?);
            } else {
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
        }
        Err(_) => {
            // Fall back to hex dump
            println!("(binary data, {} bytes)", state.len());
            println!("{}", hex::encode(state));
        }
    }
    Ok(())
}
