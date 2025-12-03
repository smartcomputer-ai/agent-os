mod util;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_host::config::HostConfig;
use aos_host::control::ControlServer;
use aos_host::host::{ExternalEvent, WorldHost};
use aos_host::manifest_loader;
use aos_host::modes::batch::BatchRunner;
use aos_host::modes::daemon::WorldDaemon;
use aos_host::util::{has_placeholder_modules, reset_journal};
use aos_store::FsStore;
use clap::{Parser, Subcommand};
use serde_json::Value as JsonValue;
use tokio::sync::{broadcast, mpsc};

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
        /// Path to world directory
        path: PathBuf,
    },
    /// Run a single batch step
    Step {
        /// Path to world directory
        path: PathBuf,

        /// AIR assets directory (default: <path>/air)
        #[arg(long)]
        air: Option<PathBuf>,

        /// Reducer crate directory (default: <path>/reducer)
        #[arg(long)]
        reducer: Option<PathBuf>,

        /// Store/journal directory (default: <path>/.aos)
        #[arg(long)]
        store: Option<PathBuf>,

        /// Module name to patch with compiled WASM (default: all placeholders)
        #[arg(long)]
        module: Option<String>,

        /// Event schema to inject
        #[arg(long)]
        event: Option<String>,

        /// Event value as JSON
        #[arg(long)]
        value: Option<String>,

        /// Force reducer recompilation
        #[arg(long)]
        force_build: bool,

        /// Clear journal before step
        #[arg(long = "reset-journal")]
        do_reset_journal: bool,
    },
    /// Run world in daemon mode with real timers
    Run {
        /// Path to world directory
        path: PathBuf,

        /// AIR assets directory (default: <path>/air)
        #[arg(long)]
        air: Option<PathBuf>,

        /// Reducer crate directory (default: <path>/reducer)
        #[arg(long)]
        reducer: Option<PathBuf>,

        /// Store/journal directory (default: <path>/.aos)
        #[arg(long)]
        store: Option<PathBuf>,

        /// Module name to patch with compiled WASM (default: all placeholders)
        #[arg(long)]
        module: Option<String>,

        /// Force reducer recompilation
        #[arg(long)]
        force_build: bool,

        /// Clear journal before running
        #[arg(long = "reset-journal")]
        do_reset_journal: bool,

        /// Event schema to inject at startup
        #[arg(long)]
        event: Option<String>,

        /// Event value as JSON (for --event)
        #[arg(long)]
        value: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::World { cmd } => match cmd {
            WorldCommand::Init { path } => cmd_world_init(path),
            WorldCommand::Step {
                path,
                air,
                reducer,
                store,
                module,
                event,
                value,
                force_build,
                do_reset_journal,
            } => {
                cmd_world_step(
                    path,
                    air,
                    reducer,
                    store,
                    module,
                    event,
                    value,
                    force_build,
                    do_reset_journal,
                )
                .await
            }
            WorldCommand::Run {
                path,
                air,
                reducer,
                store,
                module,
                force_build,
                do_reset_journal,
                event,
                value,
            } => {
                cmd_world_run(
                    path,
                    air,
                    reducer,
                    store,
                    module,
                    force_build,
                    do_reset_journal,
                    event,
                    value,
                )
                .await
            }
        },
    }
}

fn cmd_world_init(path: PathBuf) -> Result<()> {
    fs::create_dir_all(&path)?;
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

    println!("World initialized at {}", path.display());
    println!("  AIR assets: {}", path.join("air").display());
    println!("  Reducer:    {}", path.join("reducer").display());
    println!("  Store:      {}", path.join(".aos").display());
    Ok(())
}

async fn cmd_world_step(
    path: PathBuf,
    air: Option<PathBuf>,
    reducer: Option<PathBuf>,
    store_path: Option<PathBuf>,
    module: Option<String>,
    event: Option<String>,
    value: Option<String>,
    force_build: bool,
    do_reset_journal: bool,
) -> Result<()> {
    // Validate world directory
    if !path.exists() {
        anyhow::bail!("world directory '{}' not found", path.display());
    }
    if !path.is_dir() {
        anyhow::bail!("'{}' is not a directory", path.display());
    }

    // Resolve directories with defaults
    // If paths are relative, make them relative to the world directory
    let air_dir = match air {
        Some(p) if p.is_relative() => path.join(p),
        Some(p) => p,
        None => path.join("air"),
    };
    let reducer_dir = match reducer {
        Some(p) if p.is_relative() => path.join(p),
        Some(p) => p,
        None => path.join("reducer"),
    };
    // store_root is where .aos/ will be created (defaults to world directory)
    let store_root = match store_path {
        Some(p) if p.is_relative() => path.join(p),
        Some(p) => p,
        None => path.clone(),
    };

    // Optionally reset journal (journal is at <store_root>/.aos/journal/)
    if do_reset_journal {
        reset_journal(&store_root)?;
        println!("Journal cleared");
    }

    // Open store (creates .aos/store/ inside store_root)
    let store = Arc::new(FsStore::open(&store_root).context("open store")?);

    // Compile reducer if present
    let wasm_hash = if reducer_dir.exists() {
        println!("Compiling reducer from {}...", reducer_dir.display());
        let hash = util::compile_reducer(&reducer_dir, &store_root, &store, force_build)?;
        println!("Reducer compiled: {}", hash.as_str());
        Some(hash)
    } else {
        None
    };

    // Load manifest from AIR assets
    let mut loaded = manifest_loader::load_from_assets(store.clone(), &air_dir)
        .context("load manifest from assets")?
        .ok_or_else(|| anyhow!("no manifest found in {}", air_dir.display()))?;

    // Patch module hashes
    if let Some(hash) = &wasm_hash {
        let patched = util::patch_module_hashes(&mut loaded, hash, module.as_deref())?;
        if patched > 0 {
            println!("Patched {} module(s) with WASM hash", patched);
        }
    } else if has_placeholder_modules(&loaded) {
        anyhow::bail!(
            "manifest has modules with placeholder hashes but no reducer/ found; \
             use --reducer to specify reducer crate"
        );
    }

    // Create host and run
    let host_config = HostConfig::default();
    let kernel_config = util::make_kernel_config(&store_root)?;
    let host =
        WorldHost::from_loaded_manifest(store, loaded, &store_root, host_config, kernel_config)?;
    let mut runner = BatchRunner::new(host);

    // Build events
    let mut events = Vec::new();
    if let Some(schema) = event {
        let json = value.unwrap_or_else(|| "{}".to_string());
        let parsed: JsonValue = serde_json::from_str(&json).context("parse event value as JSON")?;
        let cbor = serde_cbor::to_vec(&parsed).context("encode event value as CBOR")?;
        events.push(ExternalEvent::DomainEvent {
            schema,
            value: cbor,
        });
    }

    // Run step
    let res = runner.step(events).await?;
    println!(
        "Step complete: events={} effects={} receipts={}",
        res.events_injected, res.cycle.effects_dispatched, res.cycle.receipts_applied
    );
    Ok(())
}

/// Set up tracing subscriber for daemon logging.
fn setup_logging() {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(false)
        .with_level(true)
        .init();
}

async fn cmd_world_run(
    path: PathBuf,
    air: Option<PathBuf>,
    reducer: Option<PathBuf>,
    store_path: Option<PathBuf>,
    module: Option<String>,
    force_build: bool,
    do_reset_journal: bool,
    event: Option<String>,
    value: Option<String>,
) -> Result<()> {
    // Set up logging
    setup_logging();

    // Validate world directory
    if !path.exists() {
        anyhow::bail!("world directory '{}' not found", path.display());
    }
    if !path.is_dir() {
        anyhow::bail!("'{}' is not a directory", path.display());
    }

    // Resolve directories with defaults
    let air_dir = match air {
        Some(p) if p.is_relative() => path.join(p),
        Some(p) => p,
        None => path.join("air"),
    };
    let reducer_dir = match reducer {
        Some(p) if p.is_relative() => path.join(p),
        Some(p) => p,
        None => path.join("reducer"),
    };
    let store_root = match store_path {
        Some(p) if p.is_relative() => path.join(p),
        Some(p) => p,
        None => path.clone(),
    };

    // Optionally reset journal
    if do_reset_journal {
        reset_journal(&store_root)?;
        tracing::info!("Journal cleared");
    }

    // Open store
    let store = Arc::new(FsStore::open(&store_root).context("open store")?);

    // Compile reducer if present
    let wasm_hash = if reducer_dir.exists() {
        tracing::info!("Compiling reducer from {}...", reducer_dir.display());
        let hash = util::compile_reducer(&reducer_dir, &store_root, &store, force_build)?;
        tracing::info!("Reducer compiled: {}", hash.as_str());
        Some(hash)
    } else {
        None
    };

    // Load manifest from AIR assets
    let mut loaded = manifest_loader::load_from_assets(store.clone(), &air_dir)
        .context("load manifest from assets")?
        .ok_or_else(|| anyhow!("no manifest found in {}", air_dir.display()))?;

    // Patch module hashes
    if let Some(hash) = &wasm_hash {
        let patched = util::patch_module_hashes(&mut loaded, hash, module.as_deref())?;
        if patched > 0 {
            tracing::info!("Patched {} module(s) with WASM hash", patched);
        }
    } else if has_placeholder_modules(&loaded) {
        anyhow::bail!(
            "manifest has modules with placeholder hashes but no reducer/ found; \
             use --reducer to specify reducer crate"
        );
    }

    // Create host
    let host_config = HostConfig::default();
    let kernel_config = util::make_kernel_config(&store_root)?;
    let host =
        WorldHost::from_loaded_manifest(store, loaded, &store_root, host_config, kernel_config)?;

    // Set up channels
    let (control_tx, control_rx) = mpsc::channel(128);
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    // Handle Ctrl-C and SIGTERM for graceful shutdown
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Ctrl-C received, shutting down...");
            }
            _ = async {
                if let Some(ref mut t) = term { t.recv().await; }
            } => {
                tracing::info!("SIGTERM received, shutting down...");
            }
        }
        let _ = shutdown_tx_clone.send(());
    });

    // Start control server (Unix socket under store root)
    let control_path = store_root.join(".aos/control.sock");
    let server = ControlServer::new(
        control_path.clone(),
        control_tx.clone(),
        shutdown_tx.clone(),
    );
    let server_handle = tokio::spawn(async move {
        if let Err(e) = server.run().await {
            tracing::error!("control server error: {e}");
        }
    });

    // Create and run daemon
    let mut daemon = WorldDaemon::new(host, control_rx, shutdown_rx, Some(server_handle));

    // Inject startup event if provided - do this directly on the daemon's host
    // instead of through the control channel to avoid race conditions
    if let Some(schema) = event {
        let json = value.unwrap_or_else(|| "{}".to_string());
        let parsed: JsonValue = serde_json::from_str(&json).context("parse event value as JSON")?;
        let cbor = serde_cbor::to_vec(&parsed).context("encode event value as CBOR")?;
        tracing::info!("Injecting startup event: {}", schema);
        daemon
            .host_mut()
            .enqueue_external(ExternalEvent::DomainEvent {
                schema,
                value: cbor,
            })?;
    }

    // Run the daemon
    daemon.run().await?;

    Ok(())
}
