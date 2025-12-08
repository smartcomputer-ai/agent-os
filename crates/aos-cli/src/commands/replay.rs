//! `aos world replay` command (experimental).
//!
//! Opens a world, replays journal + snapshot to head, and reports heights/state hashes.

use anyhow::Result;
use clap::Args;

use crate::opts::{WorldOpts, resolve_dirs};
use crate::util::load_world_env;

use super::{create_host, prepare_world};

#[derive(Args, Debug)]
pub struct ReplayArgs {}

pub async fn cmd_replay(opts: &WorldOpts, _args: &ReplayArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    load_world_env(&dirs.world)?;

    let (store, loaded) = prepare_world(&dirs, opts)?;
    let mut host = create_host(store, loaded, &dirs, opts)?;

    // Replaying happens on open; run a drain to ensure idle.
    let _ = host.drain()?;

    let heights = host.heights();
    println!(
        "replay ok: head={}, snapshot={:?}",
        heights.head, heights.snapshot
    );

    Ok(())
}
