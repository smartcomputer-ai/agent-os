//! `aos world cells` command: list keyed workflow cells via CellIndex.

use anyhow::Result;
use aos_cbor::Hash;
use aos_kernel::cell_index::CellMeta;
use base64::Engine;
use clap::Args;
use serde::Serialize;

use crate::opts::{WorldOpts, resolve_dirs};
use crate::util::load_world_env;

use super::{create_host, prepare_world, try_control_client};

#[derive(Args, Debug)]
pub struct CellsArgs {
    /// Workflow name (keyed)
    pub workflow_name: String,

    /// Output raw JSON without formatting
    #[arg(long)]
    pub raw: bool,
}

#[derive(Debug, Serialize)]
struct CellEntry {
    key_b64: String,
    key_display: String,
    state_hash_hex: String,
    size: u64,
    last_active_ns: u64,
}

pub async fn cmd_cells(opts: &WorldOpts, args: &CellsArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    let cells = if let Some(mut client) = try_control_client(&dirs).await {
        let (_meta, entries) = client
            .list_cells_decoded("cli-list-cells", &args.workflow_name)
            .await?;
        if entries.is_empty() {
            println!("(no cells for workflow '{}')", args.workflow_name);
            return Ok(());
        }
        entries
            .into_iter()
            .map(|entry| {
                json_to_entry(serde_json::json!({
                    "key_b64": entry.key_b64,
                    "state_hash_hex": entry.state_hash_hex,
                    "size": entry.size,
                    "last_active_ns": entry.last_active_ns,
                }))
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        // Batch mode: load world and inspect store directly via CellIndex.
        load_world_env(&dirs.world)?;
        let (store, loaded) = prepare_world(&dirs, opts)?;
        let host = create_host(store, loaded, &dirs, opts)?;
        let metas = host.list_cells(&args.workflow_name)?;
        metas.into_iter().map(meta_to_entry).collect()
    };

    render_cells(&cells, args.raw, &args.workflow_name);
    Ok(())
}

fn json_to_entry(val: serde_json::Value) -> Result<CellEntry> {
    let key_b64 = val
        .get("key_b64")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(&key_b64)
        .unwrap_or_default();
    let key_display = display_key(&key_bytes);

    let state_hash_hex = val
        .get("state_hash_hex")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let size = val.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
    let last_active_ns = val
        .get("last_active_ns")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    Ok(CellEntry {
        key_b64,
        key_display,
        state_hash_hex,
        size,
        last_active_ns,
    })
}

fn meta_to_entry(meta: CellMeta) -> CellEntry {
    let key_b64 = base64::engine::general_purpose::STANDARD.encode(&meta.key_bytes);
    let key_display = display_key(&meta.key_bytes);
    let state_hash_hex = Hash::from_bytes(&meta.state_hash)
        .map(|h| h.to_hex())
        .unwrap_or_else(|_| hex::encode(meta.state_hash));

    CellEntry {
        key_b64,
        key_display,
        state_hash_hex,
        size: meta.size,
        last_active_ns: meta.last_active_ns,
    }
}

fn display_key(bytes: &[u8]) -> String {
    // Try decode as CBOR -> JSON string (handles self-describe tags, etc.)
    if let Ok(val) = serde_cbor::from_slice::<serde_json::Value>(bytes) {
        if let Some(s) = val.as_str() {
            return s.to_string();
        }
    }
    // Fallback: UTF-8
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    // Otherwise hex
    format!("0x{}", hex::encode(bytes))
}

fn render_cells(cells: &[CellEntry], raw: bool, workflow: &str) {
    if cells.is_empty() {
        println!("(no cells for workflow '{}')", workflow);
        return;
    }

    if raw {
        let json = serde_json::to_string_pretty(cells).unwrap();
        println!("{}", json);
        return;
    }

    println!("Cells for '{}':", workflow);
    for c in cells {
        println!(
            "- key: {} (b64: {}), state: {}, size: {} bytes, last_active_ns: {}",
            c.key_display, c.key_b64, c.state_hash_hex, c.size, c.last_active_ns
        );
    }
}
