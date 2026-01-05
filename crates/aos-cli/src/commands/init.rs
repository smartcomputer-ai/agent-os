//! `aos world init` command.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use aos_air_types::{AirNode, Manifest, CURRENT_AIR_VERSION};
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
    fs::create_dir_all(path.join(".aos"))?;
    fs::create_dir_all(path.join("air"))?;
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
    let node = AirNode::Manifest(manifest);
    let json = serde_json::to_string_pretty(&node).context("serialize manifest")?;
    fs::write(path.join("air/manifest.air.json"), json)
        .context("write manifest.air.json")?;

    let sync = serde_json::json!({
        "version": 1,
        "air": { "dir": "air" },
        "build": { "reducer_dir": "reducer" },
        "modules": { "pull": false },
        "workspaces": [
            {
                "ref": "reducer",
                "dir": "reducer",
                "ignore": ["target/", ".git/", ".aos/"]
            }
        ]
    });
    fs::write(
        path.join("aos.sync.json"),
        serde_json::to_string_pretty(&sync).context("serialize sync config")?,
    )
    .context("write aos.sync.json")?;

    // TODO: Support --template to scaffold different starter manifests

    println!("World initialized at {}", path.display());
    println!("  AIR assets: {}", path.join("air").display());
    println!("  Reducer:    {}", path.join("reducer").display());
    println!("  Modules:    {}", path.join("modules").display());
    println!("  Store:      {}", path.join(".aos").display());
    println!("  Sync:       {}", path.join("aos.sync.json").display());

    if args.template.is_some() {
        println!("\nNote: --template is not yet implemented; created minimal manifest.");
    }

    Ok(())
}
