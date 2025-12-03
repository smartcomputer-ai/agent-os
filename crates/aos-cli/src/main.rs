use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use aos_host::config::HostConfig;
use aos_host::error::HostError;
use aos_host::host::{ExternalEvent, WorldHost};
use aos_host::modes::batch::BatchRunner;
use aos_kernel::KernelConfig;
use aos_store::FsStore;
use clap::{Parser, Subcommand};
use serde_json::Value as JsonValue;

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
        /// Path to world manifest (file path). Parent dir will receive .aos store.
        path: PathBuf,
    },
    /// Run a single batch step (P1 batch mode)
    Step {
        /// Path to manifest file
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        fs::create_dir_all(parent.join(".aos"))?;
    }
    println!(
        "World init complete. Add or edit manifest at {} and store under {}",
        path.display(),
        path.parent()
            .map(|p| p.join(".aos").display().to_string())
            .unwrap_or_else(|| ".aos".to_string())
    );
    Ok(())
}

async fn cmd_world_step(
    path: PathBuf,
    event: Option<String>,
    value: Option<String>,
) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("manifest path '{}' not found", path.display());
    }

    let root = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let store = Arc::new(FsStore::open(&root)?);

    let host_config = HostConfig::default();
    let kernel_config = KernelConfig::default();
    let host = WorldHost::open(store, &path, host_config, kernel_config)?;
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
