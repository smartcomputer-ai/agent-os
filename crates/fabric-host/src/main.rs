use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
use clap::{Parser, Subcommand};
use fabric_host::{
    FabricHostConfig, FabricHostService, http, smolvm::SmolvmRuntime, smolvm::SmolvmRuntimeConfig,
};
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "fabric-host", about = "Fabric host")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(long, default_value = "127.0.0.1:8791")]
    bind: SocketAddr,

    #[arg(long, default_value = ".fabric-host")]
    state_root: PathBuf,

    #[arg(long, default_value = "local-dev")]
    host_id: String,

    #[arg(long)]
    controller_url: Option<String>,

    #[arg(long)]
    advertise_url: Option<String>,

    #[arg(long, default_value_t = 5_000_000_000)]
    heartbeat_interval_ns: u128,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(name = "_boot-vm", hide = true)]
    BootVm { config: PathBuf },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    if let Some(command) = args.command {
        return match command {
            Command::BootVm { config } => fabric_host::smolvm::boot_vm_from_config_path(config)
                .context("boot smolvm VM subprocess"),
        };
    }

    tokio::runtime::Runtime::new()
        .context("create fabric host runtime")?
        .block_on(run_host(args))
}

async fn run_host(args: Args) -> anyhow::Result<()> {
    let config = FabricHostConfig {
        bind_addr: args.bind,
        state_root: args.state_root,
        host_id: args.host_id,
        controller_url: args.controller_url,
        advertise_url: args.advertise_url,
        heartbeat_interval_ns: args.heartbeat_interval_ns,
    };

    let runtime = Arc::new(
        SmolvmRuntime::open(SmolvmRuntimeConfig {
            state_root: config.state_root.clone(),
            host_id: config.host_id.clone(),
        })
        .context("open smolvm runtime")?,
    );
    let service = Arc::new(FabricHostService::new(config.clone(), runtime));
    fabric_host::controller::spawn_controller_registration(service.clone());
    let app = http::router(service);
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("bind {}", config.bind_addr))?;

    info!(addr = %config.bind_addr, "fabric host daemon listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serve fabric host daemon")?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
