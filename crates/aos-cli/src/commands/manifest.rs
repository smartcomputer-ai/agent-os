//! `aos world manifest` command.

use std::sync::Arc;

use anyhow::{Context, Result};
use aos_host::manifest_loader;
use aos_store::FsStore;
use clap::Args;

use crate::commands::try_control_client;
use crate::opts::{WorldOpts, resolve_dirs};

#[derive(Args, Debug)]
pub struct ManifestArgs {
    /// Output raw canonical form without formatting
    #[arg(long)]
    pub raw: bool,
}

pub async fn cmd_manifest(opts: &WorldOpts, args: &ManifestArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    // Prefer daemon via control for consistency metadata
    if let Some(mut client) = try_control_client(&dirs).await {
        match client.manifest_read("cli-manifest", None).await {
            Ok((meta, bytes)) => {
                println!(
                    "meta: {}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "journal_height": meta.journal_height,
                        "snapshot_hash": meta.snapshot_hash.as_ref().map(|h| h.to_hex()),
                        "manifest_hash": meta.manifest_hash.to_hex(),
                    }))?
                );
                // Decode CBOR to JSON for human-friendly output
                let manifest_json: serde_json::Value =
                    serde_cbor::from_slice(&bytes).context("decode manifest cbor")?;
                if args.raw {
                    println!("{}", serde_json::to_string(&manifest_json)?);
                } else {
                    println!("{}", serde_json::to_string_pretty(&manifest_json)?);
                }
                return Ok(());
            }
            Err(err) => {
                eprintln!("control manifest-read failed, falling back to local read: {err}");
            }
        }
    }

    // Load manifest from store
    let store = Arc::new(FsStore::open(&dirs.store_root).context("open store")?);
    let loaded = manifest_loader::load_from_assets(store.clone(), &dirs.air_dir)
        .context("load manifest from assets")?
        .ok_or_else(|| anyhow::anyhow!("no manifest found in {}", dirs.air_dir.display()))?;

    // Serialize and print
    if args.raw {
        // Output canonical JSON
        println!("{}", serde_json::to_string(&loaded.manifest)?);
    } else {
        // Pretty-print
        println!("{}", serde_json::to_string_pretty(&loaded.manifest)?);
    }

    Ok(())
}
