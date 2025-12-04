//! `aos world put-blob` command.

use std::sync::Arc;

use anyhow::{Context, Result};
use aos_store::{FsStore, Store};
use clap::Args;

use crate::input::parse_input_bytes;
use crate::opts::{resolve_dirs, WorldOpts};

use super::try_control_client;

#[derive(Args, Debug)]
pub struct PutBlobArgs {
    /// File to upload (@file or @- for stdin)
    pub file: String,

    /// Namespace (future)
    #[arg(long)]
    pub namespace: Option<String>,
}

pub async fn cmd_put_blob(opts: &WorldOpts, args: &PutBlobArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    // Parse input
    let data = parse_input_bytes(&args.file)?;

    // Try daemon first
    if let Some(mut client) = try_control_client(&dirs).await {
        let resp = client.put_blob("cli-put-blob", &data).await?;
        if !resp.ok {
            anyhow::bail!("put-blob failed: {:?}", resp.error);
        }
        if let Some(result) = resp.result {
            if let Some(hash) = result.get("hash").and_then(|v| v.as_str()) {
                println!("{}", hash);
            }
        }
        return Ok(());
    }

    // Fall back to direct store access
    let store = Arc::new(FsStore::open(&dirs.store_root).context("open store")?);
    let hash = store.put_blob(&data).context("put blob")?;
    println!("{}", hash.to_hex());

    Ok(())
}
