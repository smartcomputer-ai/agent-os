//! `aos world head` command.

use anyhow::Result;
use aos_host::control::RequestEnvelope;
use aos_kernel::journal::fs::FsJournal;

use crate::opts::{resolve_dirs, WorldOpts};

use super::try_control_client;

pub async fn cmd_head(opts: &WorldOpts) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    // Try daemon first
    if let Some(mut client) = try_control_client(&dirs).await {
        let req = RequestEnvelope {
            v: 1,
            id: "cli-head".into(),
            cmd: "journal-head".into(),
            payload: serde_json::json!({}),
        };
        let resp = client.request(&req).await?;
        if !resp.ok {
            anyhow::bail!("journal-head failed: {:?}", resp.error);
        }
        if let Some(result) = resp.result {
            if let Some(head) = result.get("head").and_then(|v| v.as_u64()) {
                println!("{}", head);
            } else {
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
        }
        return Ok(());
    }

    // Fall back to reading from store directly
    let head = FsJournal::head(&dirs.store_root)?;
    println!("{}", head);

    Ok(())
}
