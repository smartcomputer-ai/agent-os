//! `aos world state` command.

use anyhow::Result;
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

    /// Require exact journal height
    #[arg(long)]
    pub exact_height: Option<u64>,

    /// Require at least this journal height
    #[arg(long, conflicts_with = "exact_height")]
    pub at_least_height: Option<u64>,

    /// Output raw JSON without formatting
    #[arg(long)]
    pub raw: bool,
}

pub async fn cmd_state(opts: &WorldOpts, args: &StateArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    // Try daemon first
    let key_bytes_opt = args.key.as_ref().map(|k| k.as_bytes());
    let consistency = if let Some(h) = args.exact_height {
        Some(format!("exact:{h}"))
    } else if let Some(h) = args.at_least_height {
        Some(format!("at_least:{h}"))
    } else {
        None
    };

    if let Some(mut client) = try_control_client(&dirs).await {
        let (meta, state_opt) = client
            .query_state_decoded(
                "cli-state",
                &args.reducer_name,
                key_bytes_opt,
                consistency.as_deref(),
            )
            .await?;
        println!(
            "meta: {}",
            serde_json::to_string_pretty(&serde_json::json!({
                "journal_height": meta.journal_height,
                "snapshot_hash": meta.snapshot_hash.as_ref().map(|h| h.to_hex()),
                "manifest_hash": meta.manifest_hash.to_hex(),
            }))?
        );
        if let Some(state_bytes) = state_opt {
            print_state(&state_bytes, args.raw)?;
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
    let read = host.query_state(
        &args.reducer_name,
        key_bytes_opt,
        consistency
            .as_deref()
            .map(parse_consistency)
            .unwrap_or(aos_kernel::Consistency::Head),
    );
    if let Some(read) = read {
        println!(
            "meta: {}",
            serde_json::to_string_pretty(&serde_json::json!({
                "journal_height": read.meta.journal_height,
                "snapshot_hash": read.meta.snapshot_hash.as_ref().map(|h| h.to_hex()),
                "manifest_hash": read.meta.manifest_hash.to_hex(),
            }))?
        );
        if let Some(state) = read.value {
            print_state(&state, args.raw)?;
        } else {
            println!("(no state for reducer '{}')", args.reducer_name);
        }
    } else {
        println!("(read failed)");
    }

    Ok(())
}

fn parse_consistency(s: &str) -> aos_kernel::Consistency {
    if let Some(rest) = s.strip_prefix("exact:") {
        rest.parse().ok().map(aos_kernel::Consistency::Exact)
    } else if let Some(rest) = s.strip_prefix("at_least:") {
        rest.parse().ok().map(aos_kernel::Consistency::AtLeast)
    } else {
        None
    }
    .unwrap_or(aos_kernel::Consistency::Head)
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
