//! `aos export` command.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use serde_json::json;

use aos_host::manifest_loader::ZERO_HASH_SENTINEL;
use aos_host::world_io::{
    ExportOptions, WriteOptions, export_bundle, resolve_base_manifest, write_air_layout_with_options,
};
use aos_store::FsStore;

use crate::opts::WorldOpts;
use crate::output::print_success;
use crate::opts::resolve_dirs;

use super::{should_use_control, try_control_client};

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// Output directory (defaults to current directory)
    #[arg(long)]
    pub out: Option<PathBuf>,

    /// Export module WASM blobs into modules/
    #[arg(long, conflicts_with = "air_only")]
    pub with_modules: bool,

    /// Export source bundle into sources/ if available
    #[arg(long, conflicts_with = "air_only")]
    pub with_sources: bool,

    /// Include built-in sys/* defs as air/sys.air.json
    #[arg(long)]
    pub with_sys: bool,

    /// Manifest hash override (defaults to current world manifest)
    #[arg(long)]
    pub manifest: Option<String>,

    /// Export AIR only (no modules/sources)
    #[arg(long)]
    pub air_only: bool,
}

pub async fn cmd_export(opts: &WorldOpts, args: &ExportArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let store = FsStore::open(&dirs.store_root).context("open store")?;
    let mut warnings = Vec::new();

    let out_dir = args.out.clone().unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&out_dir).context("create export dir")?;

    let manifest_hash = if let Some(hash) = &args.manifest {
        hash.clone()
    } else {
        let mut control = if should_use_control(opts) {
            try_control_client(&dirs).await
        } else {
            None
        };
        let manifest_path = dirs.store_root.join(".aos/manifest.air.cbor");
        let base =
            resolve_base_manifest(&store, None, control.as_mut(), &manifest_path).await?;
        base.hash
    };

    let exported = export_bundle(
        &store,
        &manifest_hash,
        ExportOptions {
            include_sys: args.with_sys,
        },
    )?;

    write_air_layout_with_options(
        &exported.bundle,
        &exported.manifest_bytes,
        &out_dir,
        WriteOptions {
            include_sys: args.with_sys,
        },
    )?;

    if args.with_modules {
        export_modules(&exported.bundle, &out_dir, &mut warnings)?;
    }
    if args.with_sources {
        export_sources(&exported.bundle, &out_dir, &mut warnings)?;
    }

    print_success(
        opts,
        json!({
            "manifest_hash": exported.manifest_hash,
            "out_dir": out_dir.display().to_string(),
        }),
        None,
        warnings,
    )
}

fn export_modules(
    bundle: &aos_host::world_io::WorldBundle,
    out_dir: &Path,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let Some(wasm_blobs) = &bundle.wasm_blobs else {
        warnings.push("module export requested but no wasm blobs available".into());
        return Ok(());
    };
    if bundle.modules.is_empty() {
        warnings.push("module export requested but manifest has no modules".into());
        return Ok(());
    }
    let modules_dir = out_dir.join("modules");
    fs::create_dir_all(&modules_dir).context("create modules dir")?;

    for module in &bundle.modules {
        if module.name.as_str().starts_with("sys/") {
            continue;
        }
        let hash = module.wasm_hash.as_str();
        if hash == ZERO_HASH_SENTINEL {
            warnings.push(format!(
                "module '{}' has placeholder wasm_hash; skipping export",
                module.name
            ));
            continue;
        }
        let Some(bytes) = wasm_blobs.get(hash) else {
            warnings.push(format!(
                "module '{}' missing wasm blob for {}",
                module.name, hash
            ));
            continue;
        };
        let path = modules_dir.join(format!("{}-{}.wasm", module.name, hash));
        fs::write(&path, bytes)
            .with_context(|| format!("write module {}", path.display()))?;
    }
    Ok(())
}

fn export_sources(
    bundle: &aos_host::world_io::WorldBundle,
    out_dir: &Path,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let Some(source) = &bundle.source_bundle else {
        warnings.push("source export requested but no source bundle available".into());
        return Ok(());
    };
    let sources_dir = out_dir.join("sources");
    fs::create_dir_all(&sources_dir).context("create sources dir")?;
    let cursor = std::io::Cursor::new(&source.bytes);
    let mut archive = tar::Archive::new(cursor);
    archive.unpack(&sources_dir).context("unpack source bundle")?;
    Ok(())
}
