//! `aos world snapshot` command.

use anyhow::Result;

use crate::opts::{resolve_dirs, WorldOpts};
use crate::util::load_world_env;

use super::{create_host, prepare_world, try_control_client};

pub async fn cmd_snapshot(opts: &WorldOpts) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    // Try daemon first
    if let Some(mut client) = try_control_client(&dirs).await {
        let resp = client.snapshot("cli-snapshot").await?;
        if !resp.ok {
            anyhow::bail!("snapshot failed: {:?}", resp.error);
        }
        println!("Snapshot created via daemon");
        return Ok(());
    }

    // Fall back to batch mode
    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let mut host = create_host(store, loaded, &dirs, opts)?;
    host.snapshot()?;
    println!("Snapshot created");

    Ok(())
}
