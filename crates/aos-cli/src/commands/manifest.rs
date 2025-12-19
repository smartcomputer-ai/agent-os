//! `aos manifest get` command.

use std::sync::Arc;

use anyhow::{Context, Result};
use aos_host::manifest_loader;
use aos_kernel;
use aos_store::FsStore;
use clap::Args;

use crate::commands::try_control_client;
use crate::opts::{Mode, WorldOpts, resolve_dirs};
use crate::output::print_success;

#[derive(Args, Debug)]
pub struct ManifestArgs {
    /// Output raw canonical form without formatting
    #[arg(long)]
    pub raw: bool,
}

pub async fn cmd_manifest(opts: &WorldOpts, args: &ManifestArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    // Prefer daemon via control for consistency metadata
    if super::should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            match client.manifest_read("cli-manifest", None).await {
                Ok((meta, bytes)) => {
                    let manifest_json: serde_json::Value =
                        serde_cbor::from_slice(&bytes).context("decode manifest cbor")?;
                    return print_success(
                        opts,
                        if args.raw {
                            serde_json::json!({ "manifest": manifest_json, "raw": true })
                        } else {
                            manifest_json
                        },
                        Some(meta_to_json(&meta)),
                        vec![],
                    );
                }
                Err(err) => {
                    eprintln!("control manifest-get failed, falling back to local read: {err}");
                }
            }
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        } else if !opts.quiet {
            // fall through to local read
        }
    }

    // Load manifest from store
    let store = Arc::new(FsStore::open(&dirs.store_root).context("open store")?);
    let loaded = manifest_loader::load_from_assets(store.clone(), &dirs.air_dir)
        .context("load manifest from assets")?
        .ok_or_else(|| anyhow::anyhow!("no manifest found in {}", dirs.air_dir.display()))?;

    // Serialize and print
    let manifest_json = if args.raw {
        serde_json::json!({ "manifest": loaded.manifest, "raw": true })
    } else {
        serde_json::to_value(&loaded.manifest)?
    };
    print_success(
        opts,
        manifest_json,
        None,
        if opts.quiet {
            vec![]
        } else {
            vec!["daemon unavailable; using local manifest read".into()]
        },
    )
}

fn meta_to_json(meta: &aos_kernel::ReadMeta) -> serde_json::Value {
    serde_json::json!({
        "journal_height": meta.journal_height,
        "snapshot_hash": meta.snapshot_hash.as_ref().map(|h| h.to_hex()),
        "manifest_hash": meta.manifest_hash.to_hex(),
    })
}
