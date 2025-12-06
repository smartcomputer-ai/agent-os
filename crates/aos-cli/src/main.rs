mod commands;
mod input;
mod opts;
mod util;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

use commands::event::EventArgs;
use commands::gov::GovArgs;
use commands::init::InitArgs;
use commands::manifest::ManifestArgs;
use commands::put_blob::PutBlobArgs;
use commands::run::RunArgs;
use commands::state::StateArgs;
use opts::WorldOpts;

#[derive(Parser, Debug)]
#[command(name = "aos", version, about = "AgentOS CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// World management commands
    World(WorldCommand),
}

#[derive(Args, Debug)]
struct WorldCommand {
    #[command(flatten)]
    opts: WorldOpts,

    #[command(subcommand)]
    cmd: WorldSubcommand,
}

#[derive(Subcommand, Debug)]
enum WorldSubcommand {
    /// Initialize a world directory
    Init(InitArgs),

    /// Display world summary info
    Info,

    /// Run world (daemon mode by default, --batch for batch mode)
    Run(RunArgs),

    /// Send a domain event
    Event(EventArgs),

    /// Query reducer state
    State(StateArgs),

    /// Force a snapshot
    Snapshot,

    /// Replay journal to head (experimental)
    Replay(commands::replay::ReplayArgs),

    /// Show journal head
    Head,

    /// Display active manifest
    Manifest(ManifestArgs),

    /// Upload a blob to the CAS
    #[command(name = "put-blob")]
    PutBlob(PutBlobArgs),

    /// Shutdown running daemon
    Shutdown,

    /// Governance commands
    Gov(GovArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::World(world) => {
            let opts = &world.opts;
            match world.cmd {
                WorldSubcommand::Init(args) => commands::init::cmd_init(&args),
                WorldSubcommand::Info => commands::info::cmd_info(opts).await,
                WorldSubcommand::Run(args) => commands::run::cmd_run(opts, &args).await,
                WorldSubcommand::Event(args) => commands::event::cmd_event(opts, &args).await,
                WorldSubcommand::State(args) => commands::state::cmd_state(opts, &args).await,
                WorldSubcommand::Snapshot => commands::snapshot::cmd_snapshot(opts).await,
                WorldSubcommand::Replay(args) => commands::replay::cmd_replay(opts, &args).await,
                WorldSubcommand::Head => commands::head::cmd_head(opts).await,
                WorldSubcommand::Manifest(args) => {
                    commands::manifest::cmd_manifest(opts, &args).await
                }
                WorldSubcommand::PutBlob(args) => {
                    commands::put_blob::cmd_put_blob(opts, &args).await
                }
                WorldSubcommand::Shutdown => commands::shutdown::cmd_shutdown(opts).await,
                WorldSubcommand::Gov(args) => commands::gov::cmd_gov(opts, &args).await,
            }
        }
    }
}
