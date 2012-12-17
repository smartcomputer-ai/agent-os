//! `aos state get` command.

use anyhow::Result;
use clap::Args;
use serde_json::Value as JsonValue;

use crate::opts::{Mode, WorldOpts, resolve_dirs};
use crate::output::print_success;
use crate::util::load_world_env;
use crate::key::{KeyOverrides, encode_key_for_reducer};

use super::{create_host, prepare_world, should_use_control, try_control_client};

#[derive(Args, Debug)]
pub struct StateArgs {
    /// Reducer name (e.g., demo/Counter@1)
    pub reducer_name: String,

    /// Key for keyed reducers (UTF-8)
    #[arg(long)]
    pub key: Option<String>,

    /// Key as JSON literal
    #[arg(long)]
    pub key_json: Option<String>,

    /// Key as hex-encoded bytes
    #[arg(long)]
    pub key_hex: Option<String>,

    /// Key as base64-encoded bytes
    #[arg(long)]
    pub key_b64: Option<String>,

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
    let key_bytes_opt = resolve_key(&dirs, args)?;
    let consistency = if let Some(h) = args.exact_height {
        Some(format!("exact:{h}"))
    } else if let Some(h) = args.at_least_height {
        Some(format!("at_least:{h}"))
    } else {
        None
    };

    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let (meta, state_opt) = client
                .query_state_decoded(
                    "cli-state",
                    &args.reducer_name,
                key_bytes_opt.as_deref(),
                consistency.as_deref(),
            )
            .await?;
            let (data, warning) = state_opt
                .map(|bytes| decode_state(&bytes, args.raw))
                .transpose()?
                .unwrap_or_else(|| (serde_json::json!(null), None));
            return print_success(
                opts,
                data,
                Some(meta_to_json(&meta)),
                warning.into_iter().collect(),
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

    // Fall back to batch mode
    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let host = create_host(store, loaded, &dirs, opts)?;

    // Query state directly from host
    let read = host.query_state(
        &args.reducer_name,
        key_bytes_opt.as_deref(),
        consistency
            .as_deref()
            .map(parse_consistency)
            .unwrap_or(aos_kernel::Consistency::Head),
    );
    if let Some(read) = read {
        let (data, warning) = read
            .value
            .map(|bytes| decode_state(&bytes, args.raw))
            .transpose()?
            .unwrap_or_else(|| (serde_json::json!(null), None));
        print_success(
            opts,
            data,
            Some(meta_to_json(&read.meta)),
            warning
                .into_iter()
                .chain(if opts.quiet {
                    None
                } else {
                    Some("daemon unavailable; using batch mode".into())
                })
                .collect(),
        )?;
    } else {
        print_success(
            opts,
            serde_json::json!(null),
            None,
            vec!["read failed".into()],
        )?;
    }

    Ok(())
}

fn resolve_key(dirs: &crate::opts::ResolvedDirs, args: &StateArgs) -> Result<Option<Vec<u8>>> {
    let overrides = KeyOverrides {
        utf8: args.key.clone(),
        json: args.key_json.clone(),
        hex: args.key_hex.clone(),
        b64: args.key_b64.clone(),
    };
    if overrides.utf8.is_none()
        && overrides.json.is_none()
        && overrides.hex.is_none()
        && overrides.b64.is_none()
    {
        return Ok(None);
    }
    encode_key_for_reducer(dirs, &args.reducer_name, &overrides).map(Some)
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

fn meta_to_json(meta: &aos_kernel::ReadMeta) -> JsonValue {
    serde_json::json!({
        "journal_height": meta.journal_height,
        "snapshot_hash": meta.snapshot_hash.as_ref().map(|h| h.to_hex()),
        "manifest_hash": meta.manifest_hash.to_hex(),
    })
}

fn decode_state(state: &[u8], raw: bool) -> Result<(JsonValue, Option<String>)> {
    match serde_cbor::from_slice::<JsonValue>(state) {
        Ok(json) => {
            let value = if raw {
                serde_json::json!({ "state": json, "raw": true })
            } else {
                json
            };
            Ok((value, None))
        }
        Err(_) => {
            let hex_str = hex::encode(state);
            let value = if raw {
                serde_json::json!({ "state_hex": hex_str })
            } else {
                serde_json::json!({
                    "state_hex": hex_str,
                    "bytes": state.len(),
                })
            };
            Ok((value, Some("state is binary; returning hex-encoded data".into())))
        }
    }
}
