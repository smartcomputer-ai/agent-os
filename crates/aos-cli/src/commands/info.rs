//! `aos status` command.

use anyhow::{Context, Result};
use aos_host::manifest_loader;
use aos_store::FsStore;
use serde_json;

use crate::opts::{WorldOpts, resolve_dirs};
use crate::output::print_success;

pub async fn cmd_info(opts: &WorldOpts) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let mut warnings = vec![];

    // Check if store exists
    let store_path = dirs.store_root.join(".aos/store");
    if !store_path.exists() {
        return print_success(
            opts,
            serde_json::json!({
                "world": dirs.world,
                "air": dirs.air_dir,
                "reducer": dirs.reducer_dir,
                "store": dirs.store_root,
                "status": "not-initialized",
            }),
            None,
            warnings,
        );
    }

    // Try to load manifest
    let store = std::sync::Arc::new(FsStore::open(&dirs.store_root).context("open store")?);
    let manifest_info = match manifest_loader::load_from_assets(store.clone(), &dirs.air_dir) {
        Ok(Some(loaded)) => serde_json::json!({
            "schemas": loaded.manifest.schemas.len(),
            "modules": loaded.manifest.modules.len(),
            "plans": loaded.manifest.plans.len(),
            "effects": loaded.manifest.effects.len(),
            "triggers": loaded.manifest.triggers.len(),
        }),
        Ok(None) => {
            warnings.push("no manifest found in AIR directory".into());
            serde_json::json!(null)
        }
        Err(e) => {
            warnings.push(format!("failed to load manifest: {e}"));
            serde_json::json!(null)
        }
    };

    let daemon = if dirs.control_socket.exists() {
        serde_json::json!({ "running": true, "socket": dirs.control_socket })
    } else {
        serde_json::json!({ "running": false })
    };

    print_success(
        opts,
        serde_json::json!({
            "world": dirs.world,
            "air": dirs.air_dir,
            "reducer": dirs.reducer_dir,
            "store": dirs.store_root,
            "manifest": manifest_info,
            "daemon": daemon,
        }),
        None,
        warnings,
    )
}
