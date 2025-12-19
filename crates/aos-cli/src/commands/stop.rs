//! `aos stop` command.

use anyhow::Result;
use serde_json;

use crate::opts::{WorldOpts, resolve_dirs};
use crate::output::print_success;

use super::try_control_client;

pub async fn cmd_stop(opts: &WorldOpts) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    let mut client = try_control_client(&dirs)
        .await
        .ok_or_else(|| anyhow::anyhow!("No daemon running. Nothing to shut down."))?;

    let resp = client.shutdown("cli-shutdown").await?;
    if !resp.ok {
        anyhow::bail!("shutdown failed: {:?}", resp.error);
    }
    print_success(opts, serde_json::json!({ "stopped": true }), None, vec![])
}
