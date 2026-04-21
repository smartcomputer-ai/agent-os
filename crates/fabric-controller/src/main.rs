use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
use clap::Parser;
use fabric_controller::{
    FabricControllerConfig, FabricControllerService, FabricControllerState, http,
};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "fabric-controller", about = "Fabric controller")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8788")]
    bind: SocketAddr,

    #[arg(long, default_value = ".fabric-ctrl/controller.sqlite")]
    db_path: PathBuf,

    #[arg(long, default_value_t = 30_000_000_000)]
    host_heartbeat_timeout_ns: u128,

    #[arg(long, default_value_t = 5_000_000_000)]
    host_heartbeat_interval_ns: u128,

    #[arg(long, default_value_t = 86_400_000_000_000)]
    default_session_ttl_ns: u128,

    #[arg(long, default_value_t = true)]
    allow_unauthenticated_loopback: bool,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    tokio::runtime::Runtime::new()
        .context("create fabric controller runtime")?
        .block_on(run_controller(args))
}

async fn run_controller(args: Args) -> anyhow::Result<()> {
    let config = FabricControllerConfig {
        bind_addr: args.bind,
        db_path: args.db_path,
        host_heartbeat_timeout_ns: args.host_heartbeat_timeout_ns,
        host_heartbeat_interval_ns: args.host_heartbeat_interval_ns,
        default_session_ttl_ns: Some(args.default_session_ttl_ns),
        allow_unauthenticated_loopback: args.allow_unauthenticated_loopback,
    };

    let state = FabricControllerState::open(&config.db_path).context("open controller state")?;
    let service = Arc::new(FabricControllerService::new(config.clone(), state));
    let app = http::router(service);
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("bind {}", config.bind_addr))?;

    info!(
        addr = %config.bind_addr,
        db_path = %config.db_path.display(),
        "fabric controller listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serve fabric controller")?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
