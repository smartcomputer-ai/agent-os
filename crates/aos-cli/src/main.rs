use std::fs;
use std::path::PathBuf;

use aos_host::config::HostConfig;
use aos_host::error::HostError;
use aos_host::host::{ExternalEvent, WorldHost};
use aos_host::modes::batch::BatchRunner;
use aos_kernel::KernelConfig;
use aos_store::FsStore;
use clap::{Parser, Subcommand};
use serde_json::Value as JsonValue;

#[allow(unused_imports)]
use aos_store::Store; // Used for trait bound in WorldHost::<FsStore>

#[derive(Parser, Debug)]
#[command(name = "aos", version, about = "AgentOS CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// World management commands
    World {
        #[command(subcommand)]
        cmd: WorldCommand,
    },
}

#[derive(Subcommand, Debug)]
enum WorldCommand {
    /// Initialize a world directory
    Init {
        /// Path to world directory. Creates .aos subdirectory for store/journal.
        path: PathBuf,
    },
    /// Run a single batch step (P1 batch mode)
    Step {
        /// Path to world directory (containing air/ with AIR JSON files)
        path: PathBuf,
        /// Event schema to inject
        #[arg(long)]
        event: Option<String>,
        /// Event value as JSON
        #[arg(long)]
        value: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::World { cmd } => match cmd {
            WorldCommand::Init { path } => cmd_world_init(path),
            WorldCommand::Step { path, event, value } => cmd_world_step(path, event, value).await,
        },
    }
}

fn cmd_world_init(path: PathBuf) -> anyhow::Result<()> {
    fs::create_dir_all(&path)?;
    fs::create_dir_all(path.join(".aos"))?;
    fs::create_dir_all(path.join("air"))?;
    println!(
        "World init complete. Add AIR JSON files to {} and store under {}",
        path.join("air").display(),
        path.join(".aos").display()
    );
    Ok(())
}

async fn cmd_world_step(
    path: PathBuf,
    event: Option<String>,
    value: Option<String>,
) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("world directory '{}' not found", path.display());
    }
    if !path.is_dir() {
        anyhow::bail!("'{}' is not a directory", path.display());
    }

    let host_config = HostConfig::default();
    let kernel_config = KernelConfig::default();
    let host = WorldHost::<FsStore>::open_dir(&path, host_config, kernel_config)?;
    let mut runner = BatchRunner::new(host);

    let mut events = Vec::new();
    if let Some(schema) = event {
        let json = value.unwrap_or_else(|| "{}".to_string());
        let parsed: JsonValue = serde_json::from_str(&json)
            .map_err(|e| HostError::External(format!("invalid event JSON: {e}")))?;
        let cbor = serde_cbor::to_vec(&parsed)?;
        events.push(ExternalEvent::DomainEvent {
            schema,
            value: cbor,
        });
    }

    let res = runner.step(events).await?;
    println!(
        "events={} effects_dispatched={} receipts_applied={}",
        res.events_injected, res.cycle.effects_dispatched, res.cycle.receipts_applied
    );
    Ok(())
}
