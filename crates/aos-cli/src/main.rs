mod authoring;
mod client;
mod commands;
mod config;
mod output;
mod render;
mod workspace;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser};
use commands::Command;
use output::OutputOpts;

#[derive(Parser, Debug)]
#[command(name = "aos", version, about = "AgentOS node control-plane CLI")]
struct Cli {
    #[command(flatten)]
    global: GlobalOpts,

    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct GlobalOpts {
    /// Select a saved CLI profile.
    #[arg(long, global = true, env = "AOS_PROFILE")]
    profile: Option<String>,
    /// Override the control API base URL.
    #[arg(long, global = true, env = "AOS_API")]
    api: Option<String>,
    /// Override the bearer token used for API requests.
    #[arg(long, global = true, env = "AOS_TOKEN")]
    token: Option<String>,
    /// Add a custom HTTP header as `KEY=VALUE`.
    #[arg(long, global = true)]
    header: Vec<String>,
    /// Select the active node universe by UUID.
    #[arg(long, global = true, env = "AOS_UNIVERSE")]
    universe: Option<String>,
    /// Select the active world by UUID.
    #[arg(long, global = true, env = "AOS_WORLD")]
    world: Option<String>,
    /// Override the CLI config file path.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Emit compact JSON output.
    #[arg(long, global = true)]
    json: bool,
    /// Emit pretty-printed JSON output.
    #[arg(long, global = true)]
    pretty: bool,
    /// Suppress warning output where possible.
    #[arg(long, global = true)]
    quiet: bool,
    /// Omit metadata envelopes from JSON output.
    #[arg(long, global = true)]
    no_meta: bool,
    /// Print verbose request and workflow logs to stderr.
    #[arg(long, short = 'v', global = true)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let output = OutputOpts {
        json: cli.global.json,
        pretty: cli.global.pretty,
        quiet: cli.global.quiet,
        no_meta: cli.global.no_meta,
        verbose: cli.global.verbose,
    };
    commands::dispatch(&cli.global, output, cli.command).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn profile_clear_parse_does_not_conflict_with_global_universe() {
        let cli = Cli::try_parse_from(["aos", "profile", "clear"]).expect("parse profile clear");
        assert!(matches!(cli.command, Command::Profile(_)));
        assert_eq!(cli.global.universe, None);
        assert_eq!(cli.global.world, None);
    }

    #[test]
    fn profile_clear_targeted_flags_parse() {
        let cli = Cli::try_parse_from([
            "aos",
            "profile",
            "clear",
            "--clear-universe",
            "--clear-world",
        ])
        .expect("parse profile clear targeted flags");
        assert!(matches!(cli.command, Command::Profile(_)));
        assert_eq!(cli.global.universe, None);
        assert_eq!(cli.global.world, None);
    }

    #[test]
    fn air_generate_parse_accepts_export_binary_options() {
        let cli = Cli::try_parse_from([
            "aos",
            "air",
            "generate",
            "--world-root",
            "worlds/demo",
            "--manifest-path",
            "worlds/demo/workflow/Cargo.toml",
            "--package",
            "demo-workflow",
            "--bin",
            "export-air",
        ])
        .expect("parse air generate");
        assert!(matches!(cli.command, Command::Air(_)));
    }

    #[test]
    fn air_check_parse_accepts_export_binary_options() {
        let cli = Cli::try_parse_from([
            "aos",
            "air",
            "check",
            "--world-root",
            "crates/aos-agent",
            "--manifest-path",
            "Cargo.toml",
            "--package",
            "aos-agent",
            "--bin",
            "aos-air-export",
        ])
        .expect("parse air check");
        assert!(matches!(cli.command, Command::Air(_)));
    }
}
