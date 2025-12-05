//! `aos world shutdown` command.

use anyhow::Result;

use crate::opts::{WorldOpts, resolve_dirs};

use super::try_control_client;

pub async fn cmd_shutdown(opts: &WorldOpts) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    let mut client = try_control_client(&dirs)
        .await
        .ok_or_else(|| anyhow::anyhow!("No daemon running. Nothing to shut down."))?;

    let resp = client.shutdown("cli-shutdown").await?;
    if !resp.ok {
        anyhow::bail!("shutdown failed: {:?}", resp.error);
    }
    println!("Daemon shutdown initiated");

    Ok(())
}
