//! `aos run` command.

use anyhow::Result;
use aos_host::control::{ControlClient, ControlMode, ControlServer, RequestEnvelope};
use aos_host::modes::daemon::WorldDaemon;
use aos_host::util::reset_journal;
use clap::Args;
use tokio::sync::{broadcast, mpsc};

use crate::opts::{WorldOpts, resolve_dirs};
use crate::output::print_success;
use crate::util::load_world_env;

use super::{create_host, prepare_world};

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Run in batch mode: process until quiescent, then exit
    #[arg(long)]
    pub batch: bool,

    /// Clear journal before running
    #[arg(long = "reset-journal")]
    pub reset_journal: bool,
}

pub async fn cmd_run(opts: &WorldOpts, args: &RunArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;

    // Check for existing daemon
    let control_path = dirs.control_socket.clone();
    if control_path.exists() {
        match ControlClient::connect(&control_path).await {
            Ok(mut client) => {
                // Probe to see if it's healthy
                let probe = RequestEnvelope {
                    v: 1,
                    id: "probe".into(),
                    cmd: "journal-head".into(),
                    payload: serde_json::json!({}),
                };
                if let Ok(resp) = client.request(&probe).await {
                    if resp.ok {
                        if args.batch {
                            anyhow::bail!(
                                "A daemon is already running at {}. \
                                 --batch requires no daemon to be running.",
                                control_path.display()
                            );
                        } else {
                            return print_success(
                                opts,
                                serde_json::json!({
                                    "daemon": "running",
                                    "socket": control_path
                                }),
                                None,
                                vec![],
                            );
                        }
                    }
                }
                anyhow::bail!(
                    "Control socket {} exists but is unhealthy. \
                     If the daemon is not running, delete the socket and retry.",
                    control_path.display()
                );
            }
            Err(e) => {
                anyhow::bail!(
                    "Control socket {} exists but could not connect ({e}). \
                     If the daemon is not running, delete the socket and retry.",
                    control_path.display()
                );
            }
        }
    }

    // Load world-specific .env
    load_world_env(&dirs.world)?;

    // Optionally reset journal
    if args.reset_journal {
        reset_journal(&dirs.store_root)?;
        if !opts.quiet {
            eprintln!("notice: journal cleared");
        }
    }

    if args.batch {
        run_batch(opts, &dirs).await
    } else {
        run_daemon(opts, &dirs, control_path).await
    }
}

/// Run in batch mode: process until quiescent, then exit.
async fn run_batch(opts: &WorldOpts, dirs: &crate::opts::ResolvedDirs) -> Result<()> {
    use aos_host::modes::batch::BatchRunner;

    let (store, loaded) = prepare_world(dirs, opts)?;
    let host = create_host(store, loaded, dirs, opts)?;
    let mut runner = BatchRunner::new(host);

    // Run until quiescent (no events to inject)
    let res = runner.step(vec![]).await?;
    print_success(
        opts,
        serde_json::json!({
            "mode": "batch",
            "effects": res.cycle.effects_dispatched,
            "receipts": res.cycle.receipts_applied
        }),
        None,
        vec![],
    )
}

/// Run in daemon mode: long-lived with timers and control socket.
async fn run_daemon(
    opts: &WorldOpts,
    dirs: &crate::opts::ResolvedDirs,
    control_path: std::path::PathBuf,
) -> Result<()> {
    // Set up logging for daemon mode
    setup_logging();

    let (store, loaded) = prepare_world(dirs, opts)?;
    let host = create_host(store, loaded, dirs, opts)?;

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

    // Start control server
    let server = ControlServer::new(
        control_path.clone(),
        control_tx.clone(),
        shutdown_tx.clone(),
        ControlMode::Ndjson,
    );
    let server_handle = tokio::spawn(async move {
        if let Err(e) = server.run().await {
            tracing::error!("control server error: {e}");
        }
    });

    let http_config = host.config().http_server.clone();
    let http_bind = http_config.bind;
    let http_enabled = http_config.enabled;
    let http_handle =
        aos_host::http::spawn_http_server(http_config, control_tx.clone(), shutdown_tx.clone());
    if http_enabled {
        tracing::info!(
            "HTTP docs available at http://{}/api/docs/ (OpenAPI: /api/openapi.json)",
            http_bind
        );
    }

    // Create and run daemon
    let mut daemon = WorldDaemon::new(
        host,
        control_rx,
        shutdown_rx,
        Some(server_handle),
        http_handle,
    );
    daemon.run().await?;

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
