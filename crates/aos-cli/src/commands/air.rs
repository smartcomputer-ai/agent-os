use std::path::PathBuf;

use anyhow::Result;
use aos_authoring::{DEFAULT_AIR_EXPORT_BIN, write_generated_air_from_cargo_export};
use clap::{Args, Subcommand};
use serde_json::json;

use crate::GlobalOpts;
use crate::output::{OutputOpts, print_success};

#[derive(Args, Debug)]
#[command(about = "Generate and inspect authored AIR")]
pub(crate) struct AirArgs {
    #[command(subcommand)]
    cmd: AirCommand,
}

#[derive(Subcommand, Debug)]
enum AirCommand {
    /// Run a Rust AIR export binary and write generated AIR under air/generated/.
    Generate(AirGenerateArgs),
}

#[derive(Args, Debug)]
struct AirGenerateArgs {
    /// World root where generated AIR should be written.
    #[arg(long, default_value = ".")]
    world_root: PathBuf,
    /// Cargo manifest containing the AIR export binary. Defaults to <world-root>/workflow/Cargo.toml.
    #[arg(long)]
    manifest_path: Option<PathBuf>,
    /// Cargo package to run when the manifest is a workspace.
    #[arg(long)]
    package: Option<String>,
    /// Export binary name.
    #[arg(long, default_value = DEFAULT_AIR_EXPORT_BIN)]
    bin: String,
}

pub(crate) async fn handle(_global: &GlobalOpts, output: OutputOpts, args: AirArgs) -> Result<()> {
    match args.cmd {
        AirCommand::Generate(args) => handle_generate(output, args),
    }
}

fn handle_generate(output: OutputOpts, args: AirGenerateArgs) -> Result<()> {
    let manifest_path = args
        .manifest_path
        .unwrap_or_else(|| args.world_root.join("workflow/Cargo.toml"));
    let written = write_generated_air_from_cargo_export(
        &args.world_root,
        &manifest_path,
        args.package.as_deref(),
        Some(args.bin.as_str()),
    )?;
    print_success(
        output,
        json!({
            "world_root": args.world_root.display().to_string(),
            "manifest_path": manifest_path.display().to_string(),
            "package": args.package,
            "bin": args.bin,
            "written": written
                .into_iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>(),
        }),
        None,
        vec![],
    )
}
