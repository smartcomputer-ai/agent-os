use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

use aos_node_local::{BatchArgs, LocalControl, LocalHttpConfig, LocalStatePaths, run_batch, serve};

const ABOUT: &str = "Run the local AgentOS node or execute local persisted-world batch commands.";

#[derive(Parser, Debug)]
#[command(name = "aos-node-local", version, about = ABOUT)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the local HTTP API server.
    Serve(ServeArgs),
    /// Run one-off local persisted-world batch/dev operations.
    Batch(BatchArgs),
}

#[derive(Args, Debug, Clone)]
struct ServeArgs {
    #[arg(long, env = "AOS_LOCAL_STATE_ROOT", default_value = ".aos")]
    state_root: PathBuf,

    #[arg(long, env = "AOS_LOCAL_BIND", default_value = "127.0.0.1:9010")]
    bind: std::net::SocketAddr,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    match Cli::parse().command {
        Command::Serve(args) => {
            let paths = LocalStatePaths::new(args.state_root);
            paths.ensure_root()?;
            tracing::info!(
                bind = %args.bind,
                state_root = %paths.root().display(),
                roles = "supervisor,control",
                "aos-node-local initialized"
            );
            let control = LocalControl::open(paths.root())?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("build local node runtime")?;
            runtime.block_on(serve(
                LocalHttpConfig {
                    bind_addr: args.bind,
                },
                control,
            ))
        }
        Command::Batch(args) => run_batch(args),
    }
}
