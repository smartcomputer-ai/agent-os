//! `aos world info` command.

use anyhow::{Context, Result};
use aos_host::manifest_loader;
use aos_store::FsStore;

use crate::opts::{resolve_dirs, WorldOpts};

pub async fn cmd_info(opts: &WorldOpts) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    println!("World: {}", dirs.world.display());
    println!("  AIR:      {}", dirs.air_dir.display());
    println!("  Reducer:  {}", dirs.reducer_dir.display());
    println!("  Store:    {}", dirs.store_root.display());

    // Check if store exists
    let store_path = dirs.store_root.join(".aos/store");
    if !store_path.exists() {
        println!("\n  Status: Not initialized (no store found)");
        return Ok(());
    }

    // Try to load manifest
    let store = std::sync::Arc::new(FsStore::open(&dirs.store_root).context("open store")?);
    match manifest_loader::load_from_assets(store.clone(), &dirs.air_dir) {
        Ok(Some(loaded)) => {
            println!("\nManifest:");
            println!("  Schemas:  {}", loaded.manifest.schemas.len());
            println!("  Modules:  {}", loaded.manifest.modules.len());
            println!("  Plans:    {}", loaded.manifest.plans.len());
            println!("  Effects:  {}", loaded.manifest.effects.len());
            println!("  Triggers: {}", loaded.manifest.triggers.len());
        }
        Ok(None) => {
            println!("\n  Status: No manifest found in AIR directory");
        }
        Err(e) => {
            println!("\n  Status: Failed to load manifest: {e}");
        }
    }

    // Check for running daemon
    let control_path = dirs.control_socket();
    if control_path.exists() {
        println!("\nDaemon: Socket exists at {}", control_path.display());
    } else {
        println!("\nDaemon: Not running");
    }

    Ok(())
}
