use clap::{Args, Parser, Subcommand};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aos_fdb::{FdbRuntime, FdbWorldPersistence, UniverseId};
use aos_node_hosted::config::FdbWorkerConfig;
use aos_node_hosted::control::{ControlFacade, ControlHttpConfig, serve};
use aos_node_hosted::{FdbWorker, WorkerError};

const ABOUT: &str = "Run hosted AgentOS FoundationDB worker and control roles.";
const AFTER_HELP: &str = "\
Examples:
  aos-node-hosted control
  aos-node-hosted worker
  aos-node-hosted worker --universe-id 11111111-1111-1111-1111-111111111111
  aos-node-hosted node --bind 127.0.0.1:8080

Startup loads .env files from the workspace root, crate directory, and current directory
before parsing env-backed options.";

#[derive(Parser, Debug)]
#[command(name = "aos-node-hosted", version, about = ABOUT, after_help = AFTER_HELP)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run only the hosted worker role.
    Worker(WorkerCommand),
    /// Run only the hosted control HTTP API.
    Control(ControlCommand),
    /// Run worker and control roles in the same process.
    Node(NodeCommand),
}

#[derive(Args, Debug, Clone)]
struct WorkerCommand {
    #[command(flatten)]
    worker: WorkerArgs,
}

#[derive(Args, Debug, Clone)]
struct ControlCommand {
    #[command(flatten)]
    control: ControlArgs,
}

#[derive(Args, Debug, Clone)]
struct NodeCommand {
    #[command(flatten)]
    worker: WorkerArgs,

    #[command(flatten)]
    control: ControlArgs,
}

#[derive(Args, Debug, Clone)]
#[command(next_help_heading = "Worker options")]
struct WorkerArgs {
    #[arg(
        long = "universe-id",
        env = "AOS_UNIVERSE_IDS",
        value_delimiter = ',',
        help = "Optional universe filter; omit to supervise all universes"
    )]
    universe_ids: Vec<UniverseId>,

    #[arg(
        long,
        env = "AOS_WORKER_ID",
        default_value_t = default_worker_id(),
        help = "Stable worker identity recorded in leases and heartbeats"
    )]
    worker_id: String,

    #[arg(
        long,
        env = "AOS_WORKER_PINS",
        value_delimiter = ',',
        default_values_t = default_worker_pins(),
        help = "Worker pin labels used for rendezvous eligibility"
    )]
    worker_pins: Vec<String>,

    #[arg(
        long,
        env = "AOS_SUPERVISOR_POLL_INTERVAL_MS",
        help = "Supervisor loop poll interval in milliseconds"
    )]
    supervisor_poll_interval_ms: Option<u64>,

    #[arg(
        long,
        env = "AOS_HEARTBEAT_INTERVAL_MS",
        help = "Worker heartbeat cadence in milliseconds"
    )]
    heartbeat_interval_ms: Option<u64>,

    #[arg(
        long,
        env = "AOS_HEARTBEAT_TTL_MS",
        help = "Heartbeat TTL in milliseconds"
    )]
    heartbeat_ttl_ms: Option<u64>,

    #[arg(long, env = "AOS_LEASE_TTL_MS", help = "Lease TTL in milliseconds")]
    lease_ttl_ms: Option<u64>,

    #[arg(
        long,
        env = "AOS_LEASE_RENEW_INTERVAL_MS",
        help = "Lease renewal cadence in milliseconds"
    )]
    lease_renew_interval_ms: Option<u64>,

    #[arg(
        long,
        env = "AOS_IDLE_RELEASE_AFTER_MS",
        help = "Release an idle world after this many milliseconds"
    )]
    idle_release_after_ms: Option<u64>,

    #[arg(
        long,
        env = "AOS_MAINTENANCE_IDLE_AFTER_MS",
        help = "Delay snapshot/segment maintenance until a world has been idle this many milliseconds"
    )]
    maintenance_idle_after_ms: Option<u64>,

    #[arg(
        long,
        env = "AOS_WARM_RETAIN_AFTER_MS",
        help = "Keep a released world hot in memory for this many milliseconds"
    )]
    warm_retain_after_ms: Option<u64>,

    #[arg(
        long,
        env = "AOS_EFFECT_CLAIM_TIMEOUT_MS",
        help = "Effect claim timeout in milliseconds"
    )]
    effect_claim_timeout_ms: Option<u64>,

    #[arg(
        long,
        env = "AOS_TIMER_CLAIM_TIMEOUT_MS",
        help = "Timer claim timeout in milliseconds"
    )]
    timer_claim_timeout_ms: Option<u64>,

    #[arg(long, env = "AOS_SHARD_COUNT", help = "Number of hosted worker shards")]
    shard_count: Option<u32>,

    #[arg(
        long,
        env = "AOS_READY_SCAN_LIMIT",
        help = "Maximum ready worlds scanned per cycle"
    )]
    ready_scan_limit: Option<u32>,

    #[arg(
        long,
        env = "AOS_WORLD_SCAN_LIMIT",
        help = "Maximum assigned worlds scanned per cycle"
    )]
    world_scan_limit: Option<u32>,

    #[arg(
        long,
        env = "AOS_MAX_INBOX_BATCH",
        help = "Maximum inbox items decoded per world cycle"
    )]
    max_inbox_batch: Option<u32>,

    #[arg(
        long,
        env = "AOS_MAX_TICK_STEPS_PER_CYCLE",
        help = "Maximum workflow tick steps per cycle"
    )]
    max_tick_steps_per_cycle: Option<u32>,

    #[arg(
        long,
        env = "AOS_MAX_EFFECTS_PER_CYCLE",
        help = "Maximum effect intents published per cycle"
    )]
    max_effects_per_cycle: Option<u32>,

    #[arg(
        long,
        env = "AOS_MAX_TIMERS_PER_CYCLE",
        help = "Maximum timers published per cycle"
    )]
    max_timers_per_cycle: Option<u32>,

    #[arg(
        long,
        env = "AOS_DEDUPE_GC_SWEEP_LIMIT",
        help = "Maximum dedupe GC entries reclaimed per sweep"
    )]
    dedupe_gc_sweep_limit: Option<u32>,

    #[arg(
        long,
        env = "AOS_MAINTENANCE_UNIVERSE_PAGE_SIZE",
        help = "Maximum universes visited by a maintenance task per run"
    )]
    maintenance_universe_page_size: Option<u32>,

    #[arg(
        long,
        env = "AOS_EFFECT_CLAIM_REQUEUE_INTERVAL_MS",
        help = "Effect-claim requeue cadence in milliseconds"
    )]
    effect_claim_requeue_interval_ms: Option<u64>,

    #[arg(
        long,
        env = "AOS_TIMER_CLAIM_REQUEUE_INTERVAL_MS",
        help = "Timer-claim requeue cadence in milliseconds"
    )]
    timer_claim_requeue_interval_ms: Option<u64>,

    #[arg(
        long,
        env = "AOS_EFFECT_DEDUPE_GC_INTERVAL_MS",
        help = "Effect dedupe GC cadence in milliseconds"
    )]
    effect_dedupe_gc_interval_ms: Option<u64>,

    #[arg(
        long,
        env = "AOS_TIMER_DEDUPE_GC_INTERVAL_MS",
        help = "Timer dedupe GC cadence in milliseconds"
    )]
    timer_dedupe_gc_interval_ms: Option<u64>,

    #[arg(
        long,
        env = "AOS_PORTAL_DEDUPE_GC_INTERVAL_MS",
        help = "Portal dedupe GC cadence in milliseconds"
    )]
    portal_dedupe_gc_interval_ms: Option<u64>,

    #[arg(
        long,
        env = "AOS_CAS_CACHE_BYTES",
        help = "Process-local CAS cache capacity in bytes"
    )]
    cas_cache_bytes: Option<usize>,

    #[arg(
        long,
        env = "AOS_CAS_CACHE_ITEM_MAX_BYTES",
        help = "Maximum size of a single cached CAS item in bytes"
    )]
    cas_cache_item_max_bytes: Option<usize>,
}

#[derive(Args, Debug, Clone)]
#[command(next_help_heading = "Control options")]
struct ControlArgs {
    #[arg(
        long = "bind",
        env = "AOS_CONTROL_BIND",
        default_value_t = default_control_bind(),
        help = "Socket address for the control HTTP API"
    )]
    bind_addr: SocketAddr,
}

// `node` runs a busy worker loop and the control HTTP server in the same process.
// A multi-thread runtime keeps the worker from starving the control listener before
// it can bind and start serving requests.
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

    match cli.command {
        Commands::Worker(args) => {
            let persistence = open_persistence()?;
            tracing::info!(
                worker_id = %args.worker.worker_id,
                universe_filters = args.worker.universe_ids.len(),
                roles = "worker",
                "aos-node-hosted initialized"
            );
            run_worker(persistence, args.worker).await?;
        }
        Commands::Control(args) => {
            let persistence = open_persistence()?;
            tracing::info!(bind = %args.control.bind_addr, roles = "control", "aos-node-hosted initialized");
            run_control(persistence, args.control).await?;
        }
        Commands::Node(args) => {
            let persistence = open_persistence()?;
            tracing::info!(
                worker_id = %args.worker.worker_id,
                universe_filters = args.worker.universe_ids.len(),
                bind = %args.control.bind_addr,
                roles = "worker,control",
                "aos-node-hosted initialized"
            );
            let mut worker = tokio::spawn(run_worker(Arc::clone(&persistence), args.worker));
            let mut control = tokio::spawn(run_control(Arc::clone(&persistence), args.control));
            tokio::select! {
                worker_result = &mut worker => {
                    control.abort();
                    worker_result??;
                }
                control_result = &mut control => {
                    worker.abort();
                    control_result??;
                }
            }
        }
    }

    Ok(())
}

fn open_persistence() -> anyhow::Result<Arc<FdbWorldPersistence>> {
    let runtime = Arc::new(FdbRuntime::boot()?);
    let persistence = match std::env::var_os("FDB_CLUSTER_FILE") {
        Some(cluster_file) => FdbWorldPersistence::open(
            runtime,
            Some(PathBuf::from(cluster_file)),
            aos_fdb::PersistenceConfig::default(),
        )?,
        None => FdbWorldPersistence::open_default(runtime, aos_fdb::PersistenceConfig::default())?,
    };
    Ok(Arc::new(persistence))
}

async fn run_worker(
    persistence: Arc<FdbWorldPersistence>,
    args: WorkerArgs,
) -> Result<(), WorkerError> {
    let config = args.into_config();
    let poll_interval = config.supervisor_poll_interval;
    let worker = FdbWorker::new(config);
    let mut supervisor = worker.with_runtime(persistence);
    loop {
        supervisor.run_once().await?;
        tokio::time::sleep(poll_interval).await;
    }
}

async fn run_control(
    persistence: Arc<FdbWorldPersistence>,
    args: ControlArgs,
) -> anyhow::Result<()> {
    let facade = Arc::new(ControlFacade::new(persistence));
    let config = ControlHttpConfig {
        bind_addr: args.bind_addr,
    };
    serve(config, facade).await
}

impl WorkerArgs {
    fn into_config(self) -> FdbWorkerConfig {
        let defaults = FdbWorkerConfig::default();
        let pins = collect_worker_pins(self.worker_pins);
        FdbWorkerConfig {
            worker_id: self.worker_id,
            universe_filter: self.universe_ids.into_iter().collect(),
            worker_pins: if pins.is_empty() {
                defaults.worker_pins
            } else {
                pins
            },
            heartbeat_interval: self
                .heartbeat_interval_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.heartbeat_interval),
            heartbeat_ttl: self
                .heartbeat_ttl_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.heartbeat_ttl),
            lease_ttl: self
                .lease_ttl_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.lease_ttl),
            lease_renew_interval: self
                .lease_renew_interval_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.lease_renew_interval),
            maintenance_idle_after: self
                .maintenance_idle_after_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.maintenance_idle_after),
            idle_release_after: self
                .idle_release_after_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.idle_release_after),
            warm_retain_after: self
                .warm_retain_after_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.warm_retain_after),
            effect_claim_timeout: self
                .effect_claim_timeout_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.effect_claim_timeout),
            timer_claim_timeout: self
                .timer_claim_timeout_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.timer_claim_timeout),
            shard_count: self.shard_count.unwrap_or(defaults.shard_count),
            ready_scan_limit: self.ready_scan_limit.unwrap_or(defaults.ready_scan_limit),
            world_scan_limit: self.world_scan_limit.unwrap_or(defaults.world_scan_limit),
            max_inbox_batch: self.max_inbox_batch.unwrap_or(defaults.max_inbox_batch),
            max_tick_steps_per_cycle: self
                .max_tick_steps_per_cycle
                .unwrap_or(defaults.max_tick_steps_per_cycle),
            max_effects_per_cycle: self
                .max_effects_per_cycle
                .unwrap_or(defaults.max_effects_per_cycle),
            max_timers_per_cycle: self
                .max_timers_per_cycle
                .unwrap_or(defaults.max_timers_per_cycle),
            dedupe_gc_sweep_limit: self
                .dedupe_gc_sweep_limit
                .unwrap_or(defaults.dedupe_gc_sweep_limit),
            maintenance_universe_page_size: self
                .maintenance_universe_page_size
                .unwrap_or(defaults.maintenance_universe_page_size),
            supervisor_poll_interval: self
                .supervisor_poll_interval_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.supervisor_poll_interval),
            effect_claim_requeue_interval: self
                .effect_claim_requeue_interval_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.effect_claim_requeue_interval),
            timer_claim_requeue_interval: self
                .timer_claim_requeue_interval_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.timer_claim_requeue_interval),
            effect_dedupe_gc_interval: self
                .effect_dedupe_gc_interval_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.effect_dedupe_gc_interval),
            timer_dedupe_gc_interval: self
                .timer_dedupe_gc_interval_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.timer_dedupe_gc_interval),
            portal_dedupe_gc_interval: self
                .portal_dedupe_gc_interval_ms
                .map(duration_from_millis)
                .unwrap_or(defaults.portal_dedupe_gc_interval),
            cas_cache_bytes: self.cas_cache_bytes.unwrap_or(defaults.cas_cache_bytes),
            cas_cache_item_max_bytes: self
                .cas_cache_item_max_bytes
                .unwrap_or(defaults.cas_cache_item_max_bytes),
        }
    }
}

fn load_dotenv_candidates() -> anyhow::Result<()> {
    for path in dotenv_candidates() {
        if !path.exists() {
            continue;
        }
        for item in dotenvy::from_path_iter(&path)? {
            let (key, val) = item?;
            if std::env::var_os(&key).is_none() {
                unsafe {
                    std::env::set_var(&key, &val);
                }
            }
        }
    }
    Ok(())
}

fn dotenv_candidates() -> Vec<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    vec![
        workspace_root().join(".env"),
        manifest_dir.join(".env"),
        PathBuf::from(".env"),
    ]
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn duration_from_millis(value: u64) -> Duration {
    Duration::from_millis(value)
}

fn collect_worker_pins(pins: Vec<String>) -> BTreeSet<String> {
    pins.into_iter()
        .map(|pin| pin.trim().to_owned())
        .filter(|pin| !pin.is_empty())
        .collect()
}

fn default_worker_id() -> String {
    FdbWorkerConfig::default().worker_id
}

fn default_worker_pins() -> Vec<String> {
    FdbWorkerConfig::default().worker_pins.into_iter().collect()
}

fn default_control_bind() -> SocketAddr {
    ControlHttpConfig::default().bind_addr
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn help_mentions_worker_control_and_node() {
        let help = Cli::command().render_long_help().to_string();
        for needle in ["worker", "control", "node", "--universe-id", "--bind"] {
            assert!(help.contains(needle), "missing '{needle}' in help output");
        }
    }

    #[test]
    fn worker_command_defaults_to_all_universes() {
        let cli = Cli::try_parse_from(["aos-node-hosted", "worker"]).unwrap();
        match cli.command {
            Commands::Worker(args) => assert!(args.worker.universe_ids.is_empty()),
            other => panic!("expected worker command, got {other:?}"),
        }
    }

    #[test]
    fn node_command_accepts_worker_and_control_args() {
        let cli = Cli::try_parse_from([
            "aos-node-hosted",
            "node",
            "--bind",
            "127.0.0.1:9000",
            "--worker-id",
            "worker-a",
        ])
        .unwrap();

        match cli.command {
            Commands::Node(args) => {
                assert_eq!(args.worker.worker_id, "worker-a");
                assert_eq!(
                    args.control.bind_addr,
                    SocketAddr::from(([127, 0, 0, 1], 9000))
                );
            }
            other => panic!("expected node command, got {other:?}"),
        }
    }

    #[test]
    fn worker_args_map_env_style_values_into_config() {
        let args = WorkerArgs {
            universe_ids: vec![
                "11111111-1111-1111-1111-111111111111".parse().unwrap(),
                "22222222-2222-2222-2222-222222222222".parse().unwrap(),
            ],
            worker_id: String::from("worker-a"),
            worker_pins: vec![String::from("default"), String::from("blue")],
            supervisor_poll_interval_ms: Some(250),
            heartbeat_interval_ms: Some(1_000),
            heartbeat_ttl_ms: None,
            lease_ttl_ms: None,
            lease_renew_interval_ms: None,
            idle_release_after_ms: None,
            maintenance_idle_after_ms: None,
            warm_retain_after_ms: None,
            effect_claim_timeout_ms: None,
            timer_claim_timeout_ms: None,
            shard_count: Some(4),
            ready_scan_limit: None,
            world_scan_limit: None,
            max_inbox_batch: None,
            max_tick_steps_per_cycle: None,
            max_effects_per_cycle: None,
            max_timers_per_cycle: None,
            dedupe_gc_sweep_limit: None,
            maintenance_universe_page_size: None,
            effect_claim_requeue_interval_ms: None,
            timer_claim_requeue_interval_ms: None,
            effect_dedupe_gc_interval_ms: None,
            timer_dedupe_gc_interval_ms: None,
            portal_dedupe_gc_interval_ms: None,
            cas_cache_bytes: Some(123),
            cas_cache_item_max_bytes: Some(45),
        };

        let config = args.into_config();
        assert_eq!(config.worker_id, "worker-a");
        assert_eq!(config.universe_filter.len(), 2);
        assert_eq!(config.shard_count, 4);
        assert_eq!(config.supervisor_poll_interval, Duration::from_millis(250));
        assert_eq!(config.maintenance_idle_after, Duration::from_secs(10));
        assert!(config.worker_pins.contains("default"));
        assert!(config.worker_pins.contains("blue"));
        assert_eq!(config.cas_cache_bytes, 123);
        assert_eq!(config.cas_cache_item_max_bytes, 45);
    }
}
