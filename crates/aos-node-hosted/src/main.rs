use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use aos_node::UniverseId;
use aos_node_hosted::blobstore::BlobStoreConfig;
use aos_node_hosted::bootstrap::{
    build_control_deps_broker, build_materializer_deps, build_worker_runtime_broker,
};
use aos_node_hosted::config::HostedWorkerConfig;
use aos_node_hosted::control::{ControlFacade, ControlHttpConfig, serve as serve_control_http};
use aos_node_hosted::kafka::KafkaConfig;
use aos_node_hosted::load_dotenv_candidates;
use aos_node_hosted::materializer::{HostedMaterializer, HostedMaterializerConfig};
use aos_node_hosted::worker::{HostedWorker, HostedWorkerRuntime};
use clap::{Args, Parser, Subcommand};

const ABOUT: &str = "Run the experimental log-first hosted AgentOS node.";
const AFTER_HELP: &str = "\
Examples:
  aos-node-hosted
  aos-node-hosted control --bind 127.0.0.1:9011
  aos-node-hosted materializer --state-root /var/lib/aos-hosted
  aos-node-hosted worker --partition-count 4 --state-root /var/lib/aos-worker

Startup loads .env files from the workspace root, crate directory, and current directory
before parsing env-backed options.";

#[derive(Parser, Debug)]
#[command(name = "aos-node-hosted", version, about = ABOUT, after_help = AFTER_HELP)]
struct Cli {
    #[command(flatten)]
    worker: WorkerArgs,

    #[command(flatten)]
    control: ControlArgs,

    #[command(flatten)]
    runtime: RuntimeArgs,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run only the log-first shard workers.
    Worker,
    /// Run only the control HTTP API.
    Control,
    /// Run only the materializer.
    Materializer,
    /// Run control, worker, and materializer roles. This is also the default with no subcommand.
    #[command(alias = "node")]
    All,
}

#[derive(Args, Debug, Clone)]
#[command(next_help_heading = "Worker options")]
struct WorkerArgs {
    #[arg(
        long,
        env = "AOS_WORKER_ID",
        global = true,
        default_value_t = default_worker_id(),
        help = "Stable worker identity recorded in logs"
    )]
    worker_id: String,

    #[arg(
        long,
        env = "AOS_PARTITION_COUNT",
        global = true,
        default_value_t = 1,
        help = "Number of shard workers and journal partitions"
    )]
    partition_count: u32,

    #[arg(
        long,
        env = "AOS_WORKER_POLL_INTERVAL_MS",
        global = true,
        default_value_t = 500,
        help = "Shard worker poll interval in milliseconds"
    )]
    poll_interval_ms: u64,

    #[arg(
        long,
        env = "AOS_CHECKPOINT_INTERVAL_MS",
        global = true,
        default_value_t = 30_000,
        help = "Background checkpoint cadence in milliseconds"
    )]
    checkpoint_interval_ms: u64,

    #[arg(
        long,
        env = "AOS_CHECKPOINT_EVERY_EVENTS",
        global = true,
        default_value_t = 100,
        help = "Publish a checkpoint after this many committed ingress submissions per owned partition; set 0 to disable"
    )]
    checkpoint_every_events: u32,
}

#[derive(Args, Debug, Clone)]
#[command(next_help_heading = "Control options")]
struct ControlArgs {
    #[arg(
        long = "bind",
        env = "AOS_CONTROL_BIND",
        global = true,
        default_value_t = default_control_bind(),
        help = "Socket address for the control HTTP API"
    )]
    bind_addr: SocketAddr,
}

#[derive(Args, Debug, Clone)]
#[command(next_help_heading = "Runtime options")]
struct RuntimeArgs {
    #[arg(
        long = "state-root",
        env = "AOS_STATE_ROOT",
        global = true,
        default_value_os_t = default_state_root(),
        help = "Worker-local .aos-hosted state root for CAS, caches, and runtime files"
    )]
    state_root: PathBuf,

    #[arg(
        long = "default-universe-id",
        env = "AOS_DEFAULT_UNIVERSE_ID",
        global = true,
        default_value_t = default_universe_id(),
        help = "Configured singleton universe for non-routed hosted mode"
    )]
    default_universe_id: UniverseId,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("error: {err}");
        for cause in err.chain().skip(1) {
            eprintln!("  caused by: {cause}");
        }
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    load_dotenv_candidates()?;
    let cli = Cli::parse();
    let kafka_config = require_broker_kafka_config(cli.worker.partition_count)?;
    let blobstore_config = require_blobstore_config()?;

    match cli.command.unwrap_or(Commands::All) {
        Commands::Worker => {
            let state_root = cli.runtime.state_root.clone();
            let worker_runtime = build_worker_runtime_broker(
                cli.worker.partition_count,
                &state_root,
                cli.runtime.default_universe_id,
                kafka_config.clone(),
                blobstore_config.clone(),
            )?;
            tracing::info!(
                worker_id = %cli.worker.worker_id,
                partitions = cli.worker.partition_count,
                default_universe_id = %cli.runtime.default_universe_id,
                state_root = %state_root.display(),
                kafka_backend = "broker",
                kafka_bootstrap_servers = %kafka_config.bootstrap_servers.as_deref().unwrap_or("<missing>"),
                kafka_ingress_topic = %kafka_config.ingress_topic,
                kafka_journal_topic = %kafka_config.journal_topic,
                blobstore_bucket = %blobstore_config.bucket.as_deref().unwrap_or("<missing>"),
                blobstore_endpoint = %blobstore_config.endpoint.as_deref().unwrap_or("<auto>"),
                blobstore_region = %blobstore_config.region.as_deref().unwrap_or("<auto>"),
                blobstore_prefix = %blobstore_config.prefix,
                roles = "worker",
                "aos-node-hosted initialized"
            );
            serve_worker(worker_runtime, cli.worker.into_config()).await?;
        }
        Commands::Control => {
            let state_root = cli.runtime.state_root.clone();
            let control_deps = build_control_deps_broker(
                cli.worker.partition_count,
                &state_root,
                cli.runtime.default_universe_id,
                kafka_config.clone(),
                blobstore_config.clone(),
            )?;
            tracing::info!(
                bind = %cli.control.bind_addr,
                partitions = cli.worker.partition_count,
                default_universe_id = %cli.runtime.default_universe_id,
                state_root = %state_root.display(),
                kafka_backend = "broker",
                kafka_bootstrap_servers = %kafka_config.bootstrap_servers.as_deref().unwrap_or("<missing>"),
                kafka_ingress_topic = %kafka_config.ingress_topic,
                kafka_journal_topic = %kafka_config.journal_topic,
                blobstore_bucket = %blobstore_config.bucket.as_deref().unwrap_or("<missing>"),
                blobstore_endpoint = %blobstore_config.endpoint.as_deref().unwrap_or("<auto>"),
                blobstore_region = %blobstore_config.region.as_deref().unwrap_or("<auto>"),
                blobstore_prefix = %blobstore_config.prefix,
                roles = "control",
                "aos-node-hosted initialized"
            );
            serve_control(
                control_deps,
                ControlHttpConfig {
                    bind_addr: cli.control.bind_addr,
                },
            )
            .await?;
        }
        Commands::Materializer => {
            let materializer_deps = build_materializer_deps(
                cli.worker.partition_count.max(1),
                &cli.runtime.state_root,
                cli.runtime.default_universe_id,
                kafka_config.clone(),
                blobstore_config.clone(),
            )?;
            tracing::info!(
                partitions = cli.worker.partition_count,
                default_universe_id = %cli.runtime.default_universe_id,
                state_root = %cli.runtime.state_root.display(),
                kafka_backend = "broker",
                kafka_bootstrap_servers = %kafka_config.bootstrap_servers.as_deref().unwrap_or("<missing>"),
                kafka_ingress_topic = %kafka_config.ingress_topic,
                kafka_journal_topic = %kafka_config.journal_topic,
                blobstore_bucket = %blobstore_config.bucket.as_deref().unwrap_or("<missing>"),
                blobstore_endpoint = %blobstore_config.endpoint.as_deref().unwrap_or("<auto>"),
                blobstore_region = %blobstore_config.region.as_deref().unwrap_or("<auto>"),
                blobstore_prefix = %blobstore_config.prefix,
                roles = "materializer",
                "aos-node-hosted initialized"
            );
            serve_materializer(materializer_deps, HostedMaterializerConfig::default()).await?;
        }
        Commands::All => {
            let control_deps = build_control_deps_broker(
                cli.worker.partition_count,
                &cli.runtime.state_root,
                cli.runtime.default_universe_id,
                kafka_config.clone(),
                blobstore_config.clone(),
            )?;
            let worker_runtime = build_worker_runtime_broker(
                cli.worker.partition_count,
                &cli.runtime.state_root,
                cli.runtime.default_universe_id,
                kafka_config.clone(),
                blobstore_config.clone(),
            )?;
            tracing::info!(
                bind = %cli.control.bind_addr,
                worker_id = %cli.worker.worker_id,
                partitions = cli.worker.partition_count,
                default_universe_id = %cli.runtime.default_universe_id,
                state_root = %cli.runtime.state_root.display(),
                kafka_backend = "broker",
                kafka_bootstrap_servers = %kafka_config.bootstrap_servers.as_deref().unwrap_or("<missing>"),
                kafka_ingress_topic = %kafka_config.ingress_topic,
                kafka_journal_topic = %kafka_config.journal_topic,
                blobstore_bucket = %blobstore_config.bucket.as_deref().unwrap_or("<missing>"),
                blobstore_endpoint = %blobstore_config.endpoint.as_deref().unwrap_or("<auto>"),
                blobstore_region = %blobstore_config.region.as_deref().unwrap_or("<auto>"),
                blobstore_prefix = %blobstore_config.prefix,
                roles = "control,worker",
                "aos-node-hosted initialized"
            );
            let materializer_deps = build_materializer_deps(
                cli.worker.partition_count.max(1),
                &cli.runtime.state_root,
                cli.runtime.default_universe_id,
                kafka_config.clone(),
                blobstore_config.clone(),
            )?;
            serve_all(
                control_deps,
                worker_runtime,
                materializer_deps,
                cli.worker.into_config(),
                ControlHttpConfig {
                    bind_addr: cli.control.bind_addr,
                },
                HostedMaterializerConfig::default(),
            )
            .await?;
        }
    }

    Ok(())
}

fn require_broker_kafka_config(partition_count: u32) -> anyhow::Result<KafkaConfig> {
    let mut config = KafkaConfig::default();
    let bootstrap_servers = config
        .bootstrap_servers
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if bootstrap_servers.is_none() {
        return Err(anyhow!(
            "AOS_KAFKA_BOOTSTRAP_SERVERS must be set for aos-node-hosted; embedded Kafka is not supported in this binary"
        ));
    }
    config.direct_assigned_partitions = (0..partition_count.max(1)).collect();
    Ok(config)
}

fn require_blobstore_config() -> anyhow::Result<BlobStoreConfig> {
    let config = BlobStoreConfig::default();
    let bucket = config
        .bucket
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if bucket.is_none() {
        return Err(anyhow!(
            "AOS_BLOBSTORE_BUCKET or AOS_S3_BUCKET must be set for aos-node-hosted"
        ));
    }
    if config.prefix.trim().is_empty() {
        return Err(anyhow!("AOS_BLOBSTORE_PREFIX must not be empty"));
    }
    if config.pack_threshold_bytes == 0 || config.pack_target_bytes == 0 {
        return Err(anyhow!(
            "blobstore pack thresholds must be positive; check AOS_BLOBSTORE_PACK_THRESHOLD_BYTES and AOS_BLOBSTORE_PACK_TARGET_BYTES"
        ));
    }
    if config.pack_threshold_bytes > config.pack_target_bytes {
        return Err(anyhow!(
            "AOS_BLOBSTORE_PACK_THRESHOLD_BYTES must be <= AOS_BLOBSTORE_PACK_TARGET_BYTES"
        ));
    }
    Ok(config)
}

async fn serve_control(
    deps: aos_node_hosted::bootstrap::ControlDeps,
    config: ControlHttpConfig,
) -> anyhow::Result<()> {
    let facade = Arc::new(ControlFacade::new(deps)?);
    serve_control_http(config, facade).await
}

async fn serve_materializer(
    deps: aos_node_hosted::bootstrap::MaterializerDeps,
    config: HostedMaterializerConfig,
) -> anyhow::Result<()> {
    HostedMaterializer::new(deps, config).serve_forever().await;
    Ok(())
}

async fn serve_worker(
    runtime: HostedWorkerRuntime,
    config: HostedWorkerConfig,
) -> Result<(), aos_node_hosted::WorkerError> {
    let worker = HostedWorker::new(config);
    let mut supervisor = worker.with_worker_runtime(runtime);
    supervisor.serve_forever().await
}

async fn serve_all(
    control_deps: aos_node_hosted::bootstrap::ControlDeps,
    worker_runtime: HostedWorkerRuntime,
    materializer_deps: aos_node_hosted::bootstrap::MaterializerDeps,
    worker_config: HostedWorkerConfig,
    control_config: ControlHttpConfig,
    materializer_config: HostedMaterializerConfig,
) -> anyhow::Result<()> {
    // Worker and materializer both use blocking Kafka poll loops; run them on
    // dedicated blocking threads so the async control server can bind promptly.
    let mut worker_task = tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| anyhow!("build hosted worker runtime: {err}"))?;
        runtime
            .block_on(async move { serve_worker(worker_runtime, worker_config).await })
            .map_err(|err| anyhow!("hosted worker failed: {err}"))
    });
    let mut materializer_task = tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| anyhow!("build hosted materializer runtime: {err}"))?;
        runtime.block_on(
            async move { serve_materializer(materializer_deps, materializer_config).await },
        )
    });
    let mut control_task =
        tokio::spawn(async move { serve_control(control_deps, control_config).await });
    let mut worker_selected = false;
    let mut materializer_selected = false;
    let mut control_selected = false;

    let outcome = tokio::select! {
        result = &mut worker_task => {
            worker_selected = true;
            tracing::error!("hosted worker task completed; shutting down remaining roles");
            materializer_task.abort();
            control_task.abort();
            match result {
                Ok(Ok(())) => Err(anyhow!("hosted worker exited unexpectedly")),
                Ok(Err(err)) => Err(err),
                Err(err) => Err(anyhow!("hosted worker task join failed: {err}")),
            }
        }
        result = &mut materializer_task => {
            materializer_selected = true;
            tracing::error!("hosted materializer task completed; shutting down remaining roles");
            worker_task.abort();
            control_task.abort();
            match result {
                Ok(Ok(())) => Err(anyhow!("hosted materializer exited unexpectedly")),
                Ok(Err(err)) => Err(err),
                Err(err) => Err(anyhow!("hosted materializer task join failed: {err}")),
            }
        }
        result = &mut control_task => {
            control_selected = true;
            tracing::error!("hosted control task completed; shutting down remaining roles");
            worker_task.abort();
            materializer_task.abort();
            match result {
                Ok(result) => result,
                Err(err) => Err(anyhow!("hosted control task join failed: {err}")),
            }
        }
    };

    if !worker_selected {
        let _ = worker_task.await;
    }
    if !materializer_selected {
        let _ = materializer_task.await;
    }
    if !control_selected {
        let _ = control_task.await;
    }
    outcome
}

impl WorkerArgs {
    fn into_config(self) -> HostedWorkerConfig {
        HostedWorkerConfig {
            worker_id: self.worker_id,
            partition_count: self.partition_count.max(1),
            supervisor_poll_interval: Duration::from_millis(self.poll_interval_ms),
            checkpoint_interval: Duration::from_millis(self.checkpoint_interval_ms),
            checkpoint_every_events: (self.checkpoint_every_events > 0)
                .then_some(self.checkpoint_every_events),
            checkpoint_on_create: true,
        }
    }
}

fn default_worker_id() -> String {
    HostedWorkerConfig::default().worker_id
}

fn default_control_bind() -> SocketAddr {
    ControlHttpConfig::default().bind_addr
}

fn default_state_root() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".aos-hosted")
}

fn default_universe_id() -> UniverseId {
    aos_node::local_universe_id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn help_mentions_worker_control_and_all() {
        let help = Cli::command().render_long_help().to_string();
        for needle in [
            "worker",
            "control",
            "all",
            "--state-root",
            "--bind",
            "--checkpoint-every-events",
        ] {
            assert!(help.contains(needle), "missing '{needle}' in help output");
        }
    }

    #[test]
    fn worker_command_accepts_partition_count() {
        let cli = Cli::try_parse_from([
            "aos-node-hosted",
            "worker",
            "--partition-count",
            "4",
            "--checkpoint-every-events",
            "12",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::Worker) => {
                assert_eq!(cli.worker.partition_count, 4);
                assert_eq!(cli.worker.checkpoint_every_events, 12);
            }
            other => panic!("expected worker command, got {other:?}"),
        }
    }

    #[test]
    fn default_all_mode_accepts_worker_and_control_args() {
        let cli = Cli::try_parse_from([
            "aos-node-hosted",
            "--bind",
            "127.0.0.1:9000",
            "--worker-id",
            "worker-a",
            "--state-root",
            "worlds/demiurge",
            "--default-universe-id",
            "00000000-0000-0000-0000-000000000000",
        ])
        .unwrap();

        assert!(cli.command.is_none());
        assert_eq!(cli.worker.worker_id, "worker-a");
        assert_eq!(
            cli.control.bind_addr,
            SocketAddr::from(([127, 0, 0, 1], 9000))
        );
        assert_eq!(cli.runtime.state_root, PathBuf::from("worlds/demiurge"));
        assert_eq!(
            cli.runtime.default_universe_id,
            aos_node::local_universe_id()
        );
    }

    #[test]
    fn broker_kafka_config_sets_direct_assigned_partitions_from_partition_count() {
        unsafe {
            std::env::set_var("AOS_KAFKA_BOOTSTRAP_SERVERS", "localhost:19092");
        }
        let config = require_broker_kafka_config(3).expect("broker kafka config");
        assert_eq!(
            config.direct_assigned_partitions,
            [0_u32, 1, 2].into_iter().collect()
        );
    }

    #[test]
    fn all_command_accepts_node_alias() {
        let cli = Cli::try_parse_from([
            "aos-node-hosted",
            "node",
            "--bind",
            "127.0.0.1:9000",
            "--worker-id",
            "worker-a",
            "--state-root",
            "worlds/demiurge",
            "--default-universe-id",
            "00000000-0000-0000-0000-000000000000",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::All) => {
                assert_eq!(cli.worker.worker_id, "worker-a");
                assert_eq!(
                    cli.control.bind_addr,
                    SocketAddr::from(([127, 0, 0, 1], 9000))
                );
                assert_eq!(cli.runtime.state_root, PathBuf::from("worlds/demiurge"));
                assert_eq!(
                    cli.runtime.default_universe_id,
                    aos_node::local_universe_id()
                );
            }
            other => panic!("expected node command, got {other:?}"),
        }
    }
}
