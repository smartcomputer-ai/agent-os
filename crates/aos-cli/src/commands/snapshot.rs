//! `aos snapshot create` command.

use anyhow::Result;

use crate::opts::{Mode, WorldOpts, resolve_dirs};
use crate::output::print_success;
use crate::util::load_world_env;

use super::{create_host, prepare_world, should_use_control, try_control_client};

pub async fn cmd_snapshot(opts: &WorldOpts) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    // Try daemon first
    if should_use_control(opts) {
        if let Some(mut client) = try_control_client(&dirs).await {
            let resp = client.snapshot("cli-snapshot").await?;
            if !resp.ok {
                anyhow::bail!("snapshot failed: {:?}", resp.error);
            }
            return print_success(
                opts,
                serde_json::json!({ "snapshot": "created" }),
                None,
                vec![],
            );
        } else if matches!(opts.mode, Mode::Daemon) {
            anyhow::bail!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            );
        } else if !opts.quiet {
            // fall through to batch
        }
    }

    // Fall back to batch mode
    load_world_env(&dirs.world)?;
    let (store, loaded) = prepare_world(&dirs, opts)?;
    let mut host = create_host(store, loaded, &dirs, opts)?;
    host.snapshot()?;
    print_success(
        opts,
        serde_json::json!({ "snapshot": "created" }),
        None,
        if opts.quiet {
            vec![]
        } else {
            vec!["daemon unavailable; created snapshot in batch mode".into()]
        },
    )
}
