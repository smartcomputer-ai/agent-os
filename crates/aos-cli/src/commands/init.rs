//! `aos world init` command.

use std::fs;
use std::path::PathBuf;

use anyhow::Result;
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
    fs::create_dir_all(path.join("reducer/src"))?;

    // Write minimal manifest
    let manifest = r#"{
  "$kind": "manifest",
  "air_version": "1",
  "schemas": [],
  "modules": [],
  "plans": [],
  "caps": [],
  "policies": [],
  "effects": [],
  "triggers": []
}"#;
    fs::write(path.join("air/manifest.air.json"), manifest)?;

    // TODO: Support --template to scaffold different starter manifests

    println!("World initialized at {}", path.display());
    println!("  AIR assets: {}", path.join("air").display());
    println!("  Reducer:    {}", path.join("reducer").display());
    println!("  Store:      {}", path.join(".aos").display());

    if args.template.is_some() {
        println!("\nNote: --template is not yet implemented; created minimal manifest.");
    }

    Ok(())
}
