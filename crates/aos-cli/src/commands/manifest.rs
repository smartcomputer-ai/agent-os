//! `aos world manifest` command.

use std::sync::Arc;

use anyhow::{Context, Result};
use aos_host::manifest_loader;
use aos_store::FsStore;
use clap::Args;

use crate::opts::{WorldOpts, resolve_dirs};

#[derive(Args, Debug)]
pub struct ManifestArgs {
    /// Output raw canonical form without formatting
    #[arg(long)]
    pub raw: bool,
}

pub async fn cmd_manifest(opts: &WorldOpts, args: &ManifestArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

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
