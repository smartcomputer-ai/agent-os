//! `aos journal head` command.

use anyhow::Result;
use aos_host::control::RequestEnvelope;
use aos_kernel::journal::fs::FsJournal;

use crate::opts::{Mode, WorldOpts, resolve_dirs};
use crate::output::print_success;

use super::{should_use_control, try_control_client};

pub async fn cmd_head(opts: &WorldOpts) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    // Try daemon first
    if should_use_control(opts) {
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
                let data = result.get("head").cloned().unwrap_or(result);
                return print_success(opts, data, None, vec![]);
            }
            return print_success(
                opts,
                serde_json::json!(null),
                None,
                vec!["missing head result".into()],
            );
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        } else if !opts.quiet {
            // fall through to local
        }
    }

    // Fall back to reading from store directly
    let head = FsJournal::head(&dirs.store_root)?;
    print_success(
        opts,
        serde_json::json!(head),
        None,
        if opts.quiet {
            vec![]
        } else {
            vec!["daemon unavailable; using journal fs head".into()]
        },
    )
}
