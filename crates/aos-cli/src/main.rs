mod commands;
mod input;
mod output;
mod opts;
mod util;

use anyhow::Result;
use clap::{Parser, Subcommand};

use commands::event::EventArgs;
use commands::gov::GovArgs;
use commands::init::InitArgs;
use commands::manifest::ManifestArgs;
use commands::run::RunArgs;
use commands::state::StateArgs;
use opts::WorldOpts;

#[derive(Parser, Debug)]
#[command(name = "aos", version, about = "AgentOS CLI")]
struct Cli {
    #[command(flatten)]
    opts: WorldOpts,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Initialize a world directory
    Init(InitArgs),

    /// Show world status
    Status,

    /// Run world (daemon mode by default, --batch for batch mode)
    Run(RunArgs),

    /// Stop a running daemon
    Stop,

    /// Event-related commands
    #[command(subcommand)]
    Event(EventCommand),

    /// Query reducer state
    #[command(subcommand)]
    State(StateCommand),

    /// Show journal information
    #[command(subcommand)]
    Journal(JournalCommand),

    /// Display active manifest
    #[command(subcommand)]
    Manifest(ManifestCommand),

    /// Force a snapshot
    #[command(subcommand)]
    Snapshot(SnapshotCommand),

    /// Governance commands
    Gov(GovArgs),
}

#[derive(Subcommand, Debug)]
enum EventCommand {
    /// Send a domain event
    Send(EventArgs),
}

#[derive(Subcommand, Debug)]
enum StateCommand {
    /// Get reducer state
    Get(StateArgs),
}

#[derive(Subcommand, Debug)]
enum ManifestCommand {
    /// Fetch the active manifest
    Get(ManifestArgs),
}

#[derive(Subcommand, Debug)]
enum JournalCommand {
    /// Show journal head
    Head,

    /// Replay journal to head (experimental)
    Replay(commands::replay::ReplayArgs),
}

#[derive(Subcommand, Debug)]
enum SnapshotCommand {
    /// Create a snapshot
    Create,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let opts = &cli.opts;

    match cli.command {
        Command::Init(args) => commands::init::cmd_init(&args),
        Command::Status => commands::info::cmd_info(opts).await,
        Command::Run(args) => commands::run::cmd_run(opts, &args).await,
        Command::Stop => commands::stop::cmd_stop(opts).await,
        Command::Event(cmd) => match cmd {
            EventCommand::Send(args) => commands::event::cmd_event(opts, &args).await,
        },
        Command::State(cmd) => match cmd {
            StateCommand::Get(args) => commands::state::cmd_state(opts, &args).await,
        },
        Command::Manifest(cmd) => match cmd {
            ManifestCommand::Get(args) => commands::manifest::cmd_manifest(opts, &args).await,
        },
        Command::Journal(cmd) => match cmd {
            JournalCommand::Head => commands::head::cmd_head(opts).await,
            JournalCommand::Replay(args) => commands::replay::cmd_replay(opts, &args).await,
        },
        Command::Snapshot(cmd) => match cmd {
            SnapshotCommand::Create => commands::snapshot::cmd_snapshot(opts).await,
        },
        Command::Gov(args) => commands::gov::cmd_gov(opts, &args).await,
    }
}
