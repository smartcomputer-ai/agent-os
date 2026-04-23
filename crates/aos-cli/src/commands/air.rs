use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use aos_authoring::{
    DEFAULT_AIR_EXPORT_BIN, default_world_module_dir, load_world_config, resolve_world_air_sources,
    write_generated_air_from_cargo_export,
};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::GlobalOpts;
use crate::authoring::discovered_air_packages_value;
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
    /// Check whether checked-in air/generated/ matches a Rust AIR export binary.
    Check(AirCheckArgs),
}

#[derive(Args, Debug)]
struct AirGenerateArgs {
    /// World root where generated AIR should be written.
    #[arg(long, default_value = ".")]
    world_root: PathBuf,
    /// Cargo manifest containing the AIR export binary. Defaults to <world-root>/Cargo.toml when present, else <world-root>/workflow/Cargo.toml.
    #[arg(long)]
    manifest_path: Option<PathBuf>,
    /// Cargo package to run when the manifest is a workspace.
    #[arg(long)]
    package: Option<String>,
    /// Export binary name.
    #[arg(long, default_value = DEFAULT_AIR_EXPORT_BIN)]
    bin: String,
}

#[derive(Args, Debug)]
struct AirCheckArgs {
    /// World root containing the checked-in air/generated/ directory.
    #[arg(long, default_value = ".")]
    world_root: PathBuf,
    /// Cargo manifest containing the AIR export binary. Defaults to <world-root>/Cargo.toml when present, else <world-root>/workflow/Cargo.toml.
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
        AirCommand::Check(args) => handle_check(output, args),
    }
}

fn handle_generate(output: OutputOpts, args: AirGenerateArgs) -> Result<()> {
    let manifest_path = args
        .manifest_path
        .unwrap_or_else(|| default_world_module_dir(&args.world_root).join("Cargo.toml"));
    let (discovered_air_packages, warnings) = discover_air_packages_for_output(&args.world_root)?;
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
            "discovered_air_packages": discovered_air_packages,
        }),
        None,
        warnings,
    )
}

fn handle_check(output: OutputOpts, args: AirCheckArgs) -> Result<()> {
    let manifest_path = args
        .manifest_path
        .clone()
        .unwrap_or_else(|| default_world_module_dir(&args.world_root).join("Cargo.toml"));
    let (discovered_air_packages, warnings) = discover_air_packages_for_output(&args.world_root)?;
    let temp = tempfile::tempdir().context("create temporary AIR check root")?;
    let written = write_generated_air_from_cargo_export(
        temp.path(),
        &manifest_path,
        args.package.as_deref(),
        Some(args.bin.as_str()),
    )?;
    let expected_root = args.world_root.join("air/generated");
    let actual_root = temp.path().join("air/generated");
    let expected_files = collect_generated_files(&expected_root)?;
    let actual_files = collect_generated_files(&actual_root)?;

    let missing = actual_files
        .iter()
        .filter(|relative| !expected_files.contains(*relative))
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let extra = expected_files
        .iter()
        .filter(|relative| !actual_files.contains(*relative))
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();

    let mut stale = Vec::new();
    for relative in actual_files
        .iter()
        .filter(|relative| expected_files.contains(*relative))
    {
        let expected_path = expected_root.join(relative);
        let actual_path = actual_root.join(relative);
        let expected = std::fs::read(&expected_path)
            .with_context(|| format!("read checked-in AIR {}", expected_path.display()))?;
        let actual = std::fs::read(&actual_path)
            .with_context(|| format!("read generated AIR {}", actual_path.display()))?;
        if expected != actual {
            stale.push(relative.display().to_string());
        }
    }

    if !stale.is_empty() || !missing.is_empty() || !extra.is_empty() {
        bail!(
            "generated AIR is stale (changed: [{}], missing: [{}], extra: [{}])",
            stale.join(", "),
            missing.join(", "),
            extra.join(", ")
        );
    }

    print_success(
        output,
        json!({
            "world_root": args.world_root.display().to_string(),
            "manifest_path": manifest_path.display().to_string(),
            "package": args.package,
            "bin": args.bin,
            "checked": written
                .into_iter()
                .filter_map(|path| path.strip_prefix(temp.path()).ok().map(|path| path.display().to_string()))
                .collect::<Vec<_>>(),
            "discovered_air_packages": discovered_air_packages,
        }),
        None,
        warnings,
    )
}

fn discover_air_packages_for_output(world_root: &std::path::Path) -> Result<(Value, Vec<String>)> {
    let (config_path, config) = load_world_config(world_root, None)?;
    let air_sources = resolve_world_air_sources(
        world_root,
        config_path.as_deref(),
        &config,
        &world_root.join("air"),
        &default_world_module_dir(world_root),
    )?;
    Ok((
        discovered_air_packages_value(&air_sources.packages),
        air_sources.warnings,
    ))
}

fn collect_generated_files(root: &std::path::Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }
    for entry in WalkDir::new(root) {
        let entry = entry.context("walk generated AIR dir")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        files.push(
            path.strip_prefix(root)
                .context("strip generated AIR root")?
                .to_path_buf(),
        );
    }
    files.sort();
    Ok(files)
}
