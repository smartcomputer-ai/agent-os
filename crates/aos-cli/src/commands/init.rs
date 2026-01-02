//! `aos world init` command.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use aos_air_types::{Manifest, CURRENT_AIR_VERSION};
use aos_host::world_io::{GenesisImport, WorldBundle, import_genesis, write_air_layout};
use aos_store::FsStore;
use clap::Args;

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Path to create world (defaults to current directory)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Template to use (counter, http, llm-chat)
    #[arg(long)]
    pub template: Option<String>,
}

pub fn cmd_init(args: &InitArgs) -> Result<()> {
    let path = &args.path;

    fs::create_dir_all(path)?;
    fs::create_dir_all(path.join("modules"))?;
    fs::create_dir_all(path.join("reducer/src"))?;

    let manifest = Manifest {
        air_version: CURRENT_AIR_VERSION.to_string(),
        schemas: Vec::new(),
        modules: Vec::new(),
        plans: Vec::new(),
        effects: Vec::new(),
        caps: Vec::new(),
        policies: Vec::new(),
        secrets: Vec::new(),
        defaults: None,
        module_bindings: Default::default(),
        routing: None,
        triggers: Vec::new(),
    };
    let bundle = WorldBundle {
        manifest,
        schemas: Vec::new(),
        modules: Vec::new(),
        plans: Vec::new(),
        caps: Vec::new(),
        policies: Vec::new(),
        effects: Vec::new(),
        secrets: Vec::new(),
        wasm_blobs: None,
        source_bundle: None,
    };

    let store = FsStore::open(path).context("open store")?;
    let GenesisImport {
        manifest_hash: _,
        manifest_bytes,
    } = import_genesis(&store, &bundle)?;
    write_air_layout(&bundle, &manifest_bytes, path)?;

    // TODO: Support --template to scaffold different starter manifests

    println!("World initialized at {}", path.display());
    println!("  AIR assets: {}", path.join("air").display());
    println!("  Reducer:    {}", path.join("reducer").display());
    println!("  Modules:    {}", path.join("modules").display());
    println!("  Store:      {}", path.join(".aos").display());

    if args.template.is_some() {
        println!("\nNote: --template is not yet implemented; created minimal manifest.");
    }

    Ok(())
}
