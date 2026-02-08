mod commands;
mod input;
mod key;
mod opts;
mod output;
mod util;

use anyhow::Result;
use clap::{Parser, Subcommand};

use commands::blob::BlobArgs;
use commands::cells::CellsArgs;
use commands::defs::DefsArgs;
use commands::event::EventArgs;
use commands::gov::GovArgs;
use commands::init::InitArgs;
use commands::manifest::ManifestArgs;
use commands::pull::PullArgs;
use commands::push::PushArgs;
use commands::run::RunArgs;
use commands::state::StateArgs;
use commands::trace::TraceArgs;
use commands::ui::UiArgs;
use commands::workspace::WorkspaceArgs;
use opts::{WorldOpts, resolve_world};

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

    /// Push filesystem changes into the world
    Push(PushArgs),

    /// Pull world state to the filesystem
    Pull(PullArgs),

    /// Event-related commands
    #[command(subcommand)]
    Event(EventCommand),

    /// Query reducer state
    #[command(subcommand)]
    State(StateCommand),

    /// Show journal information
    #[command(subcommand)]
    Journal(JournalCommand),

    /// Trace a request/event execution lineage
    Trace(TraceArgs),

    /// Display active manifest
    #[command(subcommand)]
    Manifest(ManifestCommand),

    /// Force a snapshot
    #[command(subcommand)]
    Snapshot(SnapshotCommand),

    /// Governance commands
    Gov(GovArgs),

    /// Definition commands
    Defs(DefsArgs),

    /// Blob commands
    Blob(BlobArgs),

    /// Workspace commands
    Ws(WorkspaceArgs),

    /// UI commands
    Ui(UiArgs),
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
    /// List keys (cells) for a keyed reducer
    Ls(CellsArgs),
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

    /// Show journal entries from a sequence
    Tail(commands::journal_tail::JournalTailArgs),

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

    if !matches!(cli.command, Command::Init(_)) {
        if let Ok(world) = resolve_world(opts) {
            let _ = crate::util::load_world_env(&world);
        }
    }

    match cli.command {
        Command::Init(args) => commands::init::cmd_init(opts, &args),
        Command::Status => commands::info::cmd_info(opts).await,
        Command::Run(args) => commands::run::cmd_run(opts, &args).await,
        Command::Stop => commands::stop::cmd_stop(opts).await,
        Command::Push(args) => commands::push::cmd_push(opts, &args).await,
        Command::Pull(args) => commands::pull::cmd_pull(opts, &args).await,
        Command::Event(cmd) => match cmd {
            EventCommand::Send(args) => commands::event::cmd_event(opts, &args).await,
        },
        Command::State(cmd) => match cmd {
            StateCommand::Get(args) => commands::state::cmd_state(opts, &args).await,
            StateCommand::Ls(args) => commands::cells::cmd_cells(opts, &args).await,
        },
        Command::Manifest(cmd) => match cmd {
            ManifestCommand::Get(args) => commands::manifest::cmd_manifest(opts, &args).await,
        },
        Command::Journal(cmd) => match cmd {
            JournalCommand::Head => commands::head::cmd_head(opts).await,
            JournalCommand::Tail(args) => {
                commands::journal_tail::cmd_journal_tail(opts, &args).await
            }
            JournalCommand::Replay(args) => commands::replay::cmd_replay(opts, &args).await,
        },
        Command::Trace(args) => commands::trace::cmd_trace(opts, &args).await,
        Command::Snapshot(cmd) => match cmd {
            SnapshotCommand::Create => commands::snapshot::cmd_snapshot(opts).await,
        },
        Command::Gov(args) => commands::gov::cmd_gov(opts, &args).await,
        Command::Defs(args) => commands::defs::cmd_defs(opts, &args).await,
        Command::Blob(args) => commands::blob::cmd_blob(opts, &args).await,
        Command::Ws(args) => commands::workspace::cmd_ws(opts, &args).await,
        Command::Ui(args) => commands::ui::cmd_ui(opts, &args).await,
    }
}
