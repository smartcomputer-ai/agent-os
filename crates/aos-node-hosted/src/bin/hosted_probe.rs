use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow};
use aos_air_types::{AirNode, HashRef};
use aos_authoring::{WorldBundle, build_bundle_from_local_world};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_fdb::{
    CborPayload, FdbRuntime, FdbWorldPersistence, HostedTimerQueueStore, NodeCatalog,
    SnapshotRecord, UniverseId, WorldId, WorldStore,
};
use aos_kernel::Store;
use aos_node::HostedStore;
use aos_node_hosted::config::FdbWorkerConfig;
use aos_node_hosted::control::{
    ControlError, ControlFacade, CreateUniverseBody, JournalEntriesResponse, PutSecretBindingBody,
};
use aos_node_hosted::{
    ActiveWorkflowDebugState, ActiveWorldRef, PendingReceiptDebugState, QueuedEffectDebugState,
};
use aos_node_hosted::{FdbWorker, WorkerSupervisor};
use aos_sqlite::FsCas;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

const ABOUT: &str = "Run an in-process hosted worker probe against a selected live world.";

#[derive(Parser, Debug)]
#[command(name = "hosted_probe", version, about = ABOUT)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Dump low-level journal/snapshot layout for a hosted world without opening it.
    Layout(LayoutCommand),
    /// Read decoded journal entries directly without starting the node.
    Journal(JournalCommand),
    /// Read the current manifest for a hosted world without starting the node.
    Manifest(ManifestCommand),
    /// Read hosted workflow state directly without starting the node.
    State(StateCommand),
    /// Build a local authored world, upload it to hosted CAS, and create a fresh world.
    CreateWorld(CreateWorldCommand),
    /// Start the worker directly and measure world-open / replay / maintenance behavior.
    Startup(StartupCommand),
    /// Inject a Demiurge task event and step the worker directly until the task finishes or times out.
    DemiurgeTask(DemiurgeTaskCommand),
    /// Create a fresh hosted Demiurge world and run one task through it end to end.
    DemiurgeSmoke(DemiurgeSmokeCommand),
}

#[derive(Args, Debug, Clone)]
struct StartupCommand {
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    worker: ProbeWorkerArgs,

    #[arg(
        long,
        help = "Force hosted open to use this snapshot height as the replay seed"
    )]
    seed_height: Option<u64>,
}

#[derive(Args, Debug, Clone)]
struct LayoutCommand {
    #[command(flatten)]
    target: TargetArgs,

    #[arg(
        long,
        default_value_t = 32,
        help = "Number of segment index records to list"
    )]
    segment_limit: u32,

    #[arg(long, default_value_t = 0, help = "Hot journal scan start height")]
    hot_from: u64,

    #[arg(
        long,
        default_value_t = 32,
        help = "Number of hot journal entries to inspect"
    )]
    hot_limit: u32,

    #[arg(
        long,
        help = "Optional snapshot height to inspect in both the index and journal"
    )]
    snapshot_height: Option<u64>,

    #[arg(
        long,
        default_value_t = 16,
        help = "Number of recent snapshot records to list"
    )]
    recent_snapshot_limit: usize,

    #[arg(
        long,
        default_value_t = 16,
        help = "Number of recent manifest records to list"
    )]
    recent_manifest_limit: usize,
}

#[derive(Args, Debug, Clone)]
struct JournalCommand {
    #[command(flatten)]
    target: TargetArgs,

    #[arg(long, default_value_t = 0, help = "First journal sequence to read")]
    from: u64,

    #[arg(long, default_value_t = 32, help = "Maximum number of entries to read")]
    limit: u32,
}

#[derive(Args, Debug, Clone)]
struct CreateWorldCommand {
    #[arg(long, help = "Universe id or handle")]
    universe: String,

    #[arg(long, help = "Local world root to build and create from")]
    local_root: PathBuf,

    #[arg(
        long,
        help = "Optional world handle; defaults to local root directory name"
    )]
    handle: Option<String>,

    #[arg(long, help = "Force a fresh workflow build instead of reusing cache")]
    force_build: bool,
}

#[derive(Args, Debug, Clone)]
struct StateCommand {
    #[command(flatten)]
    target: TargetArgs,

    #[arg(long, help = "Workflow schema name, e.g. demiurge/Demiurge@1")]
    workflow: String,

    #[arg(help = "State key as a plain string; encoded as canonical CBOR text")]
    key: Option<String>,
}

#[derive(Args, Debug, Clone)]
struct ManifestCommand {
    #[command(flatten)]
    target: TargetArgs,
}

#[derive(Args, Debug, Clone)]
struct DemiurgeTaskCommand {
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    worker: ProbeWorkerArgs,

    #[arg(long, help = "Task prompt to send to demiurge/TaskSubmitted@1")]
    task: String,

    #[arg(long, help = "Optional task/session id; defaults to a random uuid4")]
    task_id: Option<String>,

    #[arg(
        long,
        default_value_os_t = default_workdir(),
        help = "Absolute workdir passed to Demiurge"
    )]
    workdir: PathBuf,

    #[arg(
        long,
        default_value = "openai-responses",
        help = "Provider in task config"
    )]
    provider: String,

    #[arg(long, default_value = "gpt-5.3-codex", help = "Model in task config")]
    model: String,

    #[arg(long, default_value_t = 4096, help = "Max tokens in task config")]
    max_tokens: u64,

    #[arg(long, default_value = "openai", help = "Tool profile in task config")]
    tool_profile: String,

    #[arg(
        long,
        default_value_t = 0,
        help = "Continue pumping this many extra cycles after the task first reaches finished=true"
    )]
    continue_after_finish_cycles: u32,

    #[arg(
        long,
        help = "After finish, keep pumping until a new segment is exported before stopping"
    )]
    wait_for_segment_after_finish: bool,

    #[arg(
        long,
        default_value_t = 4,
        help = "After the first post-finish segment export, continue pumping this many more cycles"
    )]
    continue_after_segment_cycles: u32,

    #[arg(
        long,
        default_value_t = 8,
        help = "Include this many journal entries before the finish head when attaching the post-finish journal window"
    )]
    journal_before_finish: u32,

    #[arg(
        long,
        default_value_t = 64,
        help = "Maximum number of decoded journal entries to attach around the finish/post-finish window"
    )]
    journal_limit: u32,
}

#[derive(Args, Debug, Clone)]
struct DemiurgeSmokeCommand {
    #[arg(
        long,
        default_value_os_t = default_demiurge_world_root(),
        help = "Local Demiurge world root to build and upload"
    )]
    local_root: PathBuf,

    #[arg(
        long,
        default_value = "demiurge-smoke",
        help = "Hosted universe handle to create or reuse"
    )]
    universe: String,

    #[arg(
        long,
        help = "Optional explicit world handle; defaults to demiurge-<uuid8>"
    )]
    handle: Option<String>,

    #[arg(long, help = "Force a fresh workflow build instead of reusing cache")]
    force_build: bool,

    #[command(flatten)]
    worker: ProbeWorkerArgs,

    #[arg(long, help = "Task prompt to send to demiurge/TaskSubmitted@1")]
    task: String,

    #[arg(long, help = "Optional task/session id; defaults to a random uuid4")]
    task_id: Option<String>,

    #[arg(
        long,
        default_value_os_t = default_workdir(),
        help = "Absolute workdir passed to Demiurge"
    )]
    workdir: PathBuf,

    #[arg(
        long,
        default_value = "openai-responses",
        help = "Provider in task config"
    )]
    provider: String,

    #[arg(long, default_value = "gpt-5.3-codex", help = "Model in task config")]
    model: String,

    #[arg(long, default_value_t = 4096, help = "Max tokens in task config")]
    max_tokens: u64,

    #[arg(long, default_value = "openai", help = "Tool profile in task config")]
    tool_profile: String,

    #[arg(
        long,
        default_value_t = 0,
        help = "Continue pumping this many extra cycles after the task first reaches finished=true"
    )]
    continue_after_finish_cycles: u32,

    #[arg(
        long,
        help = "After finish, keep pumping until a new segment is exported before stopping"
    )]
    wait_for_segment_after_finish: bool,

    #[arg(
        long,
        default_value_t = 4,
        help = "After the first post-finish segment export, continue pumping this many more cycles"
    )]
    continue_after_segment_cycles: u32,

    #[arg(
        long,
        default_value_t = 8,
        help = "Include this many journal entries before the finish head when attaching the post-finish journal window"
    )]
    journal_before_finish: u32,

    #[arg(
        long,
        default_value_t = 64,
        help = "Maximum number of decoded journal entries to attach around the finish/post-finish window"
    )]
    journal_limit: u32,
}

#[derive(Args, Debug, Clone)]
struct TargetArgs {
    #[arg(long, help = "Universe id or handle")]
    universe: String,

    #[arg(long, help = "World id or handle")]
    world: String,
}

#[derive(Args, Debug, Clone)]
struct ProbeWorkerArgs {
    #[arg(
        long,
        default_value_t = default_worker_id(),
        help = "Worker id used by the in-process probe"
    )]
    worker_id: String,

    #[arg(
        long,
        value_delimiter = ',',
        default_values_t = default_worker_pins(),
        help = "Worker pin labels for rendezvous eligibility"
    )]
    worker_pins: Vec<String>,

    #[arg(long, default_value_t = 512, help = "Maximum supervisor cycles to run")]
    max_cycles: u32,

    #[arg(
        long,
        default_value_t = default_sleep_ms(),
        help = "Sleep between supervisor cycles in milliseconds"
    )]
    sleep_ms: u64,

    #[arg(long, help = "Override max effects claimed per cycle")]
    max_effects_per_cycle: Option<u32>,

    #[arg(long, help = "Override max inbox batch per cycle")]
    max_inbox_batch: Option<u32>,

    #[arg(long, help = "Override max tick steps per cycle")]
    max_tick_steps_per_cycle: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
struct SnapshotMeta {
    height: u64,
    receipt_horizon_height: Option<u64>,
    snapshot_ref: String,
    manifest_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ManifestRecordMeta {
    seq: u64,
    manifest_hash: String,
}

#[derive(Debug, Clone, Serialize)]
struct LayoutReport {
    universe_id: UniverseId,
    world_id: WorldId,
    world_handle: String,
    journal_head: u64,
    runtime_manifest_hash: Option<String>,
    active_baseline: SnapshotMeta,
    latest_snapshot: Option<SnapshotMeta>,
    indexed_snapshot_at_height: Option<SnapshotMeta>,
    journal_snapshot_at_height: Option<SnapshotMeta>,
    recent_snapshots: Vec<SnapshotMeta>,
    recent_manifests: Vec<ManifestRecordMeta>,
    segments: Vec<aos_fdb::SegmentIndexRecord>,
    hot_entries: Vec<(u64, usize)>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskState {
    status: Option<String>,
    finished: Option<bool>,
    state: Value,
}

#[derive(Debug, Clone, Serialize)]
struct CycleSample {
    cycle: u32,
    elapsed_ms: u128,
    run_once_ms: u128,
    worlds_started: usize,
    worlds_released: usize,
    worlds_fenced: usize,
    active_worlds: usize,
    journal_head: u64,
    segment_count: usize,
    active_baseline_height: Option<u64>,
    latest_snapshot_height: Option<u64>,
    active_world_loaded: bool,
    has_pending_inbox: bool,
    has_pending_effects: bool,
    has_pending_maintenance: bool,
    persisted_outstanding_intent_hashes: Vec<String>,
    runtime_known_intent_hashes: Vec<String>,
    pending_receipt_intent_hashes: Vec<String>,
    pending_receipts: Vec<PendingReceiptDebugState>,
    queued_effect_intent_hashes: Vec<String>,
    queued_effects: Vec<QueuedEffectDebugState>,
    inflight_workflow_instances: Vec<ActiveWorkflowDebugState>,
    task_status: Option<String>,
    task_finished: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct StartupSummary {
    first_world_started_ms: Option<u128>,
    first_latest_snapshot_advance_ms: Option<u128>,
    first_active_baseline_advance_ms: Option<u128>,
    first_idle_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize)]
struct ProbeReport {
    mode: &'static str,
    started_at_ms: u128,
    universe_id: UniverseId,
    world_id: WorldId,
    world_handle: String,
    worker_id: String,
    initial_runtime: aos_fdb::WorldRuntimeInfo,
    initial_active_baseline: SnapshotMeta,
    initial_latest_snapshot: Option<SnapshotMeta>,
    final_runtime: aos_fdb::WorldRuntimeInfo,
    final_active_baseline: SnapshotMeta,
    final_latest_snapshot: Option<SnapshotMeta>,
    startup: StartupSummary,
    task_id: Option<String>,
    task_event_seq: Option<String>,
    task_state: Option<TaskState>,
    post_finish: Option<PostFinishSummary>,
    cycles: Vec<CycleSample>,
}

#[derive(Debug, Clone, Serialize)]
struct PostFinishSummary {
    finish_cycle: Option<u32>,
    finish_elapsed_ms: Option<u128>,
    finish_journal_head: Option<u64>,
    finish_segment_count: Option<usize>,
    first_post_finish_activity_cycle: Option<u32>,
    first_post_finish_segment_cycle: Option<u32>,
    diagnosis: Option<PostFinishDiagnosis>,
    journal_window: Option<JournalEntriesResponse>,
}

#[derive(Debug, Clone, Serialize)]
struct PostFinishDiagnosis {
    pre_segment: Option<CycleFocus>,
    first_segment: Option<CycleFocus>,
    first_activity: Option<CycleFocus>,
    resurrection_classification: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
struct CycleFocus {
    cycle: u32,
    elapsed_ms: u128,
    journal_head: u64,
    segment_count: usize,
    active_world_loaded: bool,
    has_pending_inbox: bool,
    has_pending_effects: bool,
    has_pending_maintenance: bool,
    persisted_outstanding_intent_hashes: Vec<String>,
    runtime_known_intent_hashes: Vec<String>,
    runtime_only_intent_hashes: Vec<String>,
    persisted_only_intent_hashes: Vec<String>,
    pending_receipt_intent_hashes: Vec<String>,
    pending_receipts: Vec<PendingReceiptDebugState>,
    queued_effect_intent_hashes: Vec<String>,
    queued_effects: Vec<QueuedEffectDebugState>,
    inflight_intent_hashes: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct PostFinishProbeConfig {
    continue_after_finish_cycles: u32,
    wait_for_segment_after_finish: bool,
    continue_after_segment_cycles: u32,
    journal_before_finish: u32,
    journal_limit: u32,
}

#[derive(Debug, Clone, Default)]
struct PostFinishTracker {
    finish_cycle: Option<u32>,
    finish_elapsed_ms: Option<u128>,
    finish_journal_head: Option<u64>,
    finish_segment_count: Option<usize>,
    first_post_finish_activity_cycle: Option<u32>,
    first_post_finish_segment_cycle: Option<u32>,
}

impl PostFinishTracker {
    fn observe(&mut self, sample: &CycleSample, task_finished: bool) {
        if task_finished && self.finish_cycle.is_none() {
            self.finish_cycle = Some(sample.cycle);
            self.finish_elapsed_ms = Some(sample.elapsed_ms);
            self.finish_journal_head = Some(sample.journal_head);
            self.finish_segment_count = Some(sample.segment_count);
            return;
        }

        let Some(finish_cycle) = self.finish_cycle else {
            return;
        };
        if sample.cycle <= finish_cycle {
            return;
        }

        if self.first_post_finish_activity_cycle.is_none()
            && (self
                .finish_journal_head
                .is_some_and(|head| sample.journal_head > head)
                || sample.has_pending_inbox
                || sample.has_pending_effects
                || !sample.pending_receipt_intent_hashes.is_empty()
                || !sample.queued_effect_intent_hashes.is_empty()
                || !sample.inflight_workflow_instances.is_empty())
        {
            self.first_post_finish_activity_cycle = Some(sample.cycle);
        }

        if self.first_post_finish_segment_cycle.is_none()
            && self
                .finish_segment_count
                .is_some_and(|count| sample.segment_count > count)
        {
            self.first_post_finish_segment_cycle = Some(sample.cycle);
        }
    }

    fn should_continue(&self, config: PostFinishProbeConfig, cycle: u32) -> bool {
        let Some(finish_cycle) = self.finish_cycle else {
            return false;
        };
        if config.wait_for_segment_after_finish {
            let Some(segment_cycle) = self.first_post_finish_segment_cycle else {
                return true;
            };
            return cycle < segment_cycle.saturating_add(config.continue_after_segment_cycles);
        }
        cycle < finish_cycle.saturating_add(config.continue_after_finish_cycles)
    }

    fn build_summary(
        &self,
        cycles: &[CycleSample],
        facade: &ControlFacade<FdbWorldPersistence>,
        universe: UniverseId,
        world: WorldId,
        config: PostFinishProbeConfig,
    ) -> anyhow::Result<PostFinishSummary> {
        let journal_window = self
            .finish_journal_head
            .map(|head| {
                let from = head.saturating_sub(config.journal_before_finish as u64);
                facade
                    .journal_entries(universe, world, from, config.journal_limit)
                    .map_err(anyhow::Error::from)
            })
            .transpose()?;
        Ok(PostFinishSummary {
            finish_cycle: self.finish_cycle,
            finish_elapsed_ms: self.finish_elapsed_ms,
            finish_journal_head: self.finish_journal_head,
            finish_segment_count: self.finish_segment_count,
            first_post_finish_activity_cycle: self.first_post_finish_activity_cycle,
            first_post_finish_segment_cycle: self.first_post_finish_segment_cycle,
            diagnosis: build_post_finish_diagnosis(
                cycles,
                self.finish_cycle,
                self.first_post_finish_activity_cycle,
                self.first_post_finish_segment_cycle,
            ),
            journal_window,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
struct CreateWorldReport {
    universe_id: UniverseId,
    world_id: WorldId,
    world_handle: String,
    manifest_hash: String,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        for cause in err.chain().skip(1) {
            eprintln!("  caused by: {cause}");
        }
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    load_dotenv_candidates()?;
    let cli = Cli::parse();
    match cli.command {
        Commands::Layout(args) => {
            let report = run_layout_probe(args)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Commands::Journal(args) => {
            let report = run_journal_probe(args)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Commands::Manifest(args) => {
            let report = run_manifest_probe(args)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Commands::State(args) => {
            let report = run_state_probe(args)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Commands::CreateWorld(args) => {
            let report = run_create_world(args)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Commands::Startup(args) => {
            let report = run_startup_probe(args)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Commands::DemiurgeTask(args) => {
            let report = run_demiurge_task_probe(args)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Commands::DemiurgeSmoke(args) => {
            let report = run_demiurge_smoke_probe(args)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}

fn run_layout_probe(args: LayoutCommand) -> anyhow::Result<LayoutReport> {
    let persistence = open_persistence()?;
    let facade = ControlFacade::new(Arc::clone(&persistence));
    let resolved = resolve_target(&facade, &args.target)?;
    let recent_records = scan_journal_metadata(
        &*persistence,
        resolved.universe_id,
        resolved.world_id,
        args.recent_snapshot_limit,
        args.recent_manifest_limit,
    )?;
    let indexed_snapshot_at_height = args
        .snapshot_height
        .map(|height| {
            persistence.snapshot_at_height(resolved.universe_id, resolved.world_id, height)
        })
        .transpose()?
        .map(to_snapshot_meta);
    let journal_snapshot_at_height = args
        .snapshot_height
        .map(|height| {
            find_snapshot_record_in_journal(
                &*persistence,
                resolved.universe_id,
                resolved.world_id,
                height,
            )
        })
        .transpose()?
        .flatten()
        .map(|record| SnapshotMeta {
            height: record.height,
            receipt_horizon_height: record.receipt_horizon_height,
            snapshot_ref: record.snapshot_ref,
            manifest_hash: record.manifest_hash,
        });
    Ok(LayoutReport {
        universe_id: resolved.universe_id,
        world_id: resolved.world_id,
        world_handle: resolved.world_handle,
        journal_head: persistence.journal_head(resolved.universe_id, resolved.world_id)?,
        runtime_manifest_hash: persistence
            .world_runtime_info(
                resolved.universe_id,
                resolved.world_id,
                aos_runtime::now_wallclock_ns(),
            )?
            .meta
            .manifest_hash,
        active_baseline: to_snapshot_meta(
            persistence.snapshot_active_baseline(resolved.universe_id, resolved.world_id)?,
        ),
        latest_snapshot: snapshot_latest_meta(
            &*persistence,
            resolved.universe_id,
            resolved.world_id,
        )?,
        indexed_snapshot_at_height,
        journal_snapshot_at_height,
        recent_snapshots: recent_records.snapshots,
        recent_manifests: recent_records.manifests,
        segments: persistence.segment_index_read_from(
            resolved.universe_id,
            resolved.world_id,
            0,
            args.segment_limit,
        )?,
        hot_entries: persistence.debug_journal_hot_window(
            resolved.universe_id,
            resolved.world_id,
            args.hot_from,
            args.hot_limit,
        )?,
    })
}

fn run_journal_probe(args: JournalCommand) -> anyhow::Result<Value> {
    let persistence = open_persistence()?;
    let facade = ControlFacade::new(Arc::clone(&persistence));
    let resolved = resolve_target(&facade, &args.target)?;
    Ok(serde_json::to_value(facade.journal_entries(
        resolved.universe_id,
        resolved.world_id,
        args.from,
        args.limit,
    )?)?)
}

fn run_state_probe(args: StateCommand) -> anyhow::Result<Value> {
    let persistence = open_persistence()?;
    let facade = ControlFacade::new(Arc::clone(&persistence));
    let resolved = resolve_target(&facade, &args.target)?;
    let key = args
        .key
        .as_ref()
        .map(|value| to_canonical_cbor(value))
        .transpose()?;
    let response = facade.state_get(
        resolved.universe_id,
        resolved.world_id,
        &args.workflow,
        key,
        None,
    )?;
    let mut out = serde_json::to_value(&response)?;
    if let Some(state_b64) = out.get("state_b64").and_then(Value::as_str) {
        let bytes = BASE64_STANDARD.decode(state_b64)?;
        out["state_expanded"] =
            serde_cbor::from_slice::<Value>(&bytes).context("decode state_b64 as CBOR")?;
    }
    Ok(out)
}

fn run_manifest_probe(args: ManifestCommand) -> anyhow::Result<Value> {
    let persistence = open_persistence()?;
    let facade = ControlFacade::new(Arc::clone(&persistence));
    let resolved = resolve_target(&facade, &args.target)?;
    Ok(serde_json::to_value(
        facade.manifest(resolved.universe_id, resolved.world_id)?,
    )?)
}

fn run_create_world(args: CreateWorldCommand) -> anyhow::Result<CreateWorldReport> {
    let persistence = open_persistence()?;
    let facade = ControlFacade::new(Arc::clone(&persistence));
    let universe = resolve_universe(&facade, &args.universe)?;
    let (store, bundle) = build_local_bundle(&args.local_root, args.force_build)?;
    let manifest_hash = store_bundle_to_hosted(
        Arc::clone(&persistence) as Arc<dyn WorldStore>,
        universe.record.universe_id,
        &store,
        &bundle,
    )?;
    let handle = args.handle.unwrap_or_else(|| {
        args.local_root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("world")
            .to_string()
    });
    let created = facade.create_world(
        universe.record.universe_id,
        aos_fdb::CreateWorldRequest {
            world_id: None,
            handle: Some(handle.clone()),
            placement_pin: None,
            created_at_ns: 0,
            source: aos_fdb::CreateWorldSource::Manifest {
                manifest_hash: manifest_hash.clone(),
            },
        },
    )?;
    Ok(CreateWorldReport {
        universe_id: universe.record.universe_id,
        world_id: created.record.world_id,
        world_handle: handle,
        manifest_hash,
    })
}

fn build_local_bundle(
    local_root: &Path,
    force_build: bool,
) -> anyhow::Result<(FsCas, WorldBundle)> {
    let (store, bundle, _warnings) = build_bundle_from_local_world(local_root, force_build)?;
    Ok((store, bundle))
}

fn store_bundle_to_hosted(
    persistence: Arc<dyn WorldStore>,
    universe: UniverseId,
    local_store: &impl Store,
    bundle: &WorldBundle,
) -> anyhow::Result<String> {
    let hosted_store = HostedStore::new(persistence, universe);
    let mut manifest = bundle.manifest.clone();

    for entry in &mut manifest.schemas {
        entry.hash = resolve_manifest_ref(&hosted_store, bundle, "schema", &entry.name)?;
    }
    for entry in &mut manifest.modules {
        entry.hash = resolve_manifest_ref(&hosted_store, bundle, "module", &entry.name)?;
    }
    for entry in &mut manifest.caps {
        entry.hash = resolve_manifest_ref(&hosted_store, bundle, "cap", &entry.name)?;
    }
    for entry in &mut manifest.effects {
        entry.hash = resolve_manifest_ref(&hosted_store, bundle, "effect", &entry.name)?;
    }
    for entry in &mut manifest.policies {
        entry.hash = resolve_manifest_ref(&hosted_store, bundle, "policy", &entry.name)?;
    }
    for entry in &mut manifest.secrets {
        if let aos_air_types::SecretEntry::Ref(reference) = entry {
            reference.hash =
                resolve_manifest_ref(&hosted_store, bundle, "secret", &reference.name)?;
        }
    }

    for module in &bundle.modules {
        let wasm_hash = Hash::from_hex_str(module.wasm_hash.as_str())
            .with_context(|| format!("parse module wasm hash for {}", module.name))?;
        let bytes = local_store
            .get_blob(wasm_hash)
            .with_context(|| format!("load local wasm blob for {}", module.name))?;
        hosted_store
            .put_blob(&bytes)
            .with_context(|| format!("store hosted wasm blob for {}", module.name))?;
    }

    Ok(hosted_store
        .put_node(&AirNode::Manifest(manifest))
        .context("store hosted manifest")?
        .to_hex())
}

fn resolve_manifest_ref(
    hosted_store: &HostedStore,
    bundle: &WorldBundle,
    kind: &str,
    name: &str,
) -> anyhow::Result<HashRef> {
    let (hash, bytes) =
        builtin_node_bytes(kind, name).or_else(|_| bundle_node_bytes(bundle, kind, name))?;
    let stored = hosted_store
        .put_blob(&bytes)
        .with_context(|| format!("store hosted {kind} {name}"))?;
    if stored != hash {
        return Err(anyhow!(
            "hosted {kind} {name} hash mismatch: expected {}, got {}",
            hash.to_hex(),
            stored.to_hex()
        ));
    }
    HashRef::new(hash.to_hex()).context("create hash ref")
}

fn bundle_node_bytes(
    bundle: &WorldBundle,
    kind: &str,
    name: &str,
) -> anyhow::Result<(Hash, Vec<u8>)> {
    let node = match kind {
        "schema" => bundle
            .schemas
            .iter()
            .find(|value| value.name == name)
            .map(|value| AirNode::Defschema(value.clone())),
        "module" => bundle
            .modules
            .iter()
            .find(|value| value.name == name)
            .map(|value| AirNode::Defmodule(value.clone())),
        "cap" => bundle
            .caps
            .iter()
            .find(|value| value.name == name)
            .map(|value| AirNode::Defcap(value.clone())),
        "effect" => bundle
            .effects
            .iter()
            .find(|value| value.name == name)
            .map(|value| AirNode::Defeffect(value.clone())),
        "policy" => bundle
            .policies
            .iter()
            .find(|value| value.name == name)
            .map(|value| AirNode::Defpolicy(value.clone())),
        "secret" => bundle
            .secrets
            .iter()
            .find(|value| value.name == name)
            .map(|value| AirNode::Defsecret(value.clone())),
        _ => None,
    }
    .ok_or_else(|| anyhow!("missing {kind} definition for {name} in local bundle"))?;
    let bytes = to_canonical_cbor(&node).with_context(|| format!("encode {kind} {name}"))?;
    Ok((Hash::of_bytes(&bytes), bytes))
}

fn builtin_node_bytes(kind: &str, name: &str) -> anyhow::Result<(Hash, Vec<u8>)> {
    match kind {
        "schema" => {
            let builtin = aos_air_types::builtins::find_builtin_schema(name)
                .ok_or_else(|| anyhow!("missing local or builtin {kind} definition for {name}"))?;
            let bytes = to_canonical_cbor(&builtin.schema)
                .with_context(|| format!("encode builtin {kind} {name}"))?;
            Ok((builtin.hash, bytes))
        }
        "effect" => {
            let builtin = aos_air_types::builtins::find_builtin_effect(name)
                .ok_or_else(|| anyhow!("missing local or builtin {kind} definition for {name}"))?;
            let bytes = to_canonical_cbor(&builtin.effect)
                .with_context(|| format!("encode builtin {kind} {name}"))?;
            Ok((builtin.hash, bytes))
        }
        "cap" => {
            let builtin = aos_air_types::builtins::find_builtin_cap(name)
                .ok_or_else(|| anyhow!("missing local or builtin {kind} definition for {name}"))?;
            let bytes = to_canonical_cbor(&builtin.cap)
                .with_context(|| format!("encode builtin {kind} {name}"))?;
            Ok((builtin.hash, bytes))
        }
        "module" => {
            let builtin = aos_air_types::builtins::find_builtin_module(name)
                .ok_or_else(|| anyhow!("missing local or builtin {kind} definition for {name}"))?;
            let bytes = to_canonical_cbor(&builtin.module)
                .with_context(|| format!("encode builtin {kind} {name}"))?;
            Ok((builtin.hash, bytes))
        }
        _ => Err(anyhow!(
            "missing local or builtin {kind} definition for {name}"
        )),
    }
}

fn find_snapshot_record_in_journal(
    persistence: &dyn WorldStore,
    universe: UniverseId,
    world: WorldId,
    snapshot_height: u64,
) -> anyhow::Result<Option<aos_kernel::journal::SnapshotRecord>> {
    let mut from = 0;
    let mut found = None;
    loop {
        let entries = persistence.journal_read_range(universe, world, from, 1024)?;
        if entries.is_empty() {
            break;
        }
        for (seq, payload) in entries {
            let parsed =
                match serde_cbor::from_slice::<aos_kernel::journal::JournalRecord>(&payload) {
                    Ok(aos_kernel::journal::JournalRecord::Snapshot(record)) => Some(record),
                    Ok(_) => None,
                    Err(_) => serde_cbor::from_slice::<aos_fdb::SnapshotRecord>(&payload)
                        .ok()
                        .map(|record| aos_kernel::journal::SnapshotRecord {
                            snapshot_ref: record.snapshot_ref,
                            height: record.height,
                            logical_time_ns: record.logical_time_ns,
                            receipt_horizon_height: record.receipt_horizon_height,
                            manifest_hash: record.manifest_hash,
                        }),
                };
            if let Some(record) = parsed
                && record.height == snapshot_height
            {
                found = Some(record);
            }
            from = seq.saturating_add(1);
        }
    }
    Ok(found)
}

#[derive(Default)]
struct JournalMetadataScan {
    snapshots: Vec<SnapshotMeta>,
    manifests: Vec<ManifestRecordMeta>,
}

fn scan_journal_metadata(
    persistence: &dyn WorldStore,
    universe: UniverseId,
    world: WorldId,
    snapshot_limit: usize,
    manifest_limit: usize,
) -> anyhow::Result<JournalMetadataScan> {
    let mut scan = JournalMetadataScan::default();
    let mut from = 0;
    loop {
        let entries = persistence.journal_read_range(universe, world, from, 2048)?;
        if entries.is_empty() {
            break;
        }
        for (seq, payload) in entries {
            if let Ok(record) =
                serde_cbor::from_slice::<aos_kernel::journal::JournalRecord>(&payload)
            {
                match record {
                    aos_kernel::journal::JournalRecord::Snapshot(snapshot) => {
                        scan.snapshots.push(SnapshotMeta {
                            height: snapshot.height,
                            receipt_horizon_height: snapshot.receipt_horizon_height,
                            snapshot_ref: snapshot.snapshot_ref,
                            manifest_hash: snapshot.manifest_hash,
                        });
                    }
                    aos_kernel::journal::JournalRecord::Manifest(record) => {
                        scan.manifests.push(ManifestRecordMeta {
                            seq,
                            manifest_hash: record.manifest_hash,
                        });
                    }
                    _ => {}
                }
            } else if let Ok(snapshot) = serde_cbor::from_slice::<aos_fdb::SnapshotRecord>(&payload)
            {
                scan.snapshots.push(to_snapshot_meta(snapshot));
            }
            from = seq.saturating_add(1);
        }
    }
    if scan.snapshots.len() > snapshot_limit {
        scan.snapshots = scan
            .snapshots
            .split_off(scan.snapshots.len().saturating_sub(snapshot_limit));
    }
    if scan.manifests.len() > manifest_limit {
        scan.manifests = scan
            .manifests
            .split_off(scan.manifests.len().saturating_sub(manifest_limit));
    }
    Ok(scan)
}

fn run_startup_probe(args: StartupCommand) -> anyhow::Result<ProbeReport> {
    let persistence = open_persistence()?;
    let facade = ControlFacade::new(Arc::clone(&persistence));
    let resolved = resolve_target(&facade, &args.target)?;
    let mut world_config = aos_runtime::WorldConfig::from_env();
    world_config.forced_replay_seed_height = args.seed_height;
    let mut supervisor = build_supervisor(
        Arc::clone(&persistence),
        &args.worker,
        resolved.universe_id,
        world_config,
    );
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before unix epoch")?
        .as_millis();
    let start = Instant::now();

    let initial_runtime = persistence.world_runtime_info(
        resolved.universe_id,
        resolved.world_id,
        aos_runtime::now_wallclock_ns(),
    )?;
    let initial_active_baseline = to_snapshot_meta(
        persistence.snapshot_active_baseline(resolved.universe_id, resolved.world_id)?,
    );
    let initial_latest_snapshot = snapshot_latest_meta(
        persistence.as_ref(),
        resolved.universe_id,
        resolved.world_id,
    )?;

    let result = pump_world(
        &facade,
        &persistence,
        &mut supervisor,
        &resolved,
        &args.worker,
        start,
        None,
        None,
        None,
    )?;

    Ok(ProbeReport {
        mode: "startup",
        started_at_ms: started,
        universe_id: resolved.universe_id,
        world_id: resolved.world_id,
        world_handle: resolved.world_handle,
        worker_id: args.worker.worker_id.clone(),
        initial_runtime,
        initial_active_baseline,
        initial_latest_snapshot,
        final_runtime: result.final_runtime,
        final_active_baseline: result.final_active_baseline,
        final_latest_snapshot: result.final_latest_snapshot,
        startup: result.startup,
        task_id: None,
        task_event_seq: None,
        task_state: None,
        post_finish: None,
        cycles: result.cycles,
    })
}

fn run_demiurge_task_probe(args: DemiurgeTaskCommand) -> anyhow::Result<ProbeReport> {
    let persistence = open_persistence()?;
    let facade = ControlFacade::new(Arc::clone(&persistence));
    let resolved = resolve_target(&facade, &args.target)?;
    run_demiurge_task_probe_for_target(&persistence, &facade, resolved, &args)
}

fn run_demiurge_smoke_probe(args: DemiurgeSmokeCommand) -> anyhow::Result<ProbeReport> {
    load_world_dotenv(&args.local_root)?;
    let persistence = open_persistence()?;
    let facade = ControlFacade::new(Arc::clone(&persistence));
    let universe = ensure_universe_handle(&facade, &args.universe)?;
    ensure_demiurge_secret_bindings(&facade, universe.record.universe_id, &args.provider)?;

    let (store, bundle) = build_local_bundle(&args.local_root, args.force_build)?;
    let manifest_hash = store_bundle_to_hosted(
        Arc::clone(&persistence) as Arc<dyn WorldStore>,
        universe.record.universe_id,
        &store,
        &bundle,
    )?;
    let world_handle = args
        .handle
        .clone()
        .unwrap_or_else(|| format!("demiurge-{}", &Uuid::new_v4().to_string()[..8]));
    let created = facade.create_world(
        universe.record.universe_id,
        aos_fdb::CreateWorldRequest {
            world_id: None,
            handle: Some(world_handle.clone()),
            placement_pin: None,
            created_at_ns: 0,
            source: aos_fdb::CreateWorldSource::Manifest { manifest_hash },
        },
    )?;
    let target = ResolvedTarget {
        universe_id: universe.record.universe_id,
        world_id: created.record.world_id,
        world_handle,
    };
    let task_args = DemiurgeTaskCommand {
        target: TargetArgs {
            universe: universe.record.universe_id.to_string(),
            world: created.record.world_id.to_string(),
        },
        worker: args.worker,
        task: args.task,
        task_id: args.task_id,
        workdir: args.workdir,
        provider: args.provider,
        model: args.model,
        max_tokens: args.max_tokens,
        tool_profile: args.tool_profile,
        continue_after_finish_cycles: args.continue_after_finish_cycles,
        wait_for_segment_after_finish: args.wait_for_segment_after_finish,
        continue_after_segment_cycles: args.continue_after_segment_cycles,
        journal_before_finish: args.journal_before_finish,
        journal_limit: args.journal_limit,
    };
    run_demiurge_task_probe_for_target(&persistence, &facade, target, &task_args)
}

fn run_demiurge_task_probe_for_target(
    persistence: &Arc<FdbWorldPersistence>,
    facade: &ControlFacade<FdbWorldPersistence>,
    resolved: ResolvedTarget,
    args: &DemiurgeTaskCommand,
) -> anyhow::Result<ProbeReport> {
    let mut supervisor = build_supervisor(
        Arc::clone(&persistence),
        &args.worker,
        resolved.universe_id,
        aos_runtime::WorldConfig::from_env(),
    );
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before unix epoch")?
        .as_millis();
    let start = Instant::now();

    let initial_runtime = persistence.world_runtime_info(
        resolved.universe_id,
        resolved.world_id,
        aos_runtime::now_wallclock_ns(),
    )?;
    let initial_active_baseline = to_snapshot_meta(
        persistence.snapshot_active_baseline(resolved.universe_id, resolved.world_id)?,
    );
    let initial_latest_snapshot = snapshot_latest_meta(
        persistence.as_ref(),
        resolved.universe_id,
        resolved.world_id,
    )?;

    let task_id = args
        .task_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let ingress = aos_fdb::DomainEventIngress {
        schema: "demiurge/TaskSubmitted@1".into(),
        value: CborPayload::inline(build_demiurge_task_event(&task_id, &args)?),
        key: None,
        correlation_id: None,
    };
    let enqueued = facade.enqueue_event(resolved.universe_id, resolved.world_id, ingress)?;
    let task_key = to_canonical_cbor(&task_id)?;

    let result = pump_world(
        &facade,
        &persistence,
        &mut supervisor,
        &resolved,
        &args.worker,
        start,
        Some(task_id.clone()),
        Some(task_key),
        Some(PostFinishProbeConfig {
            continue_after_finish_cycles: args.continue_after_finish_cycles,
            wait_for_segment_after_finish: args.wait_for_segment_after_finish,
            continue_after_segment_cycles: args.continue_after_segment_cycles,
            journal_before_finish: args.journal_before_finish,
            journal_limit: args.journal_limit,
        }),
    )?;

    Ok(ProbeReport {
        mode: "demiurge_task",
        started_at_ms: started,
        universe_id: resolved.universe_id,
        world_id: resolved.world_id,
        world_handle: resolved.world_handle,
        worker_id: args.worker.worker_id.clone(),
        initial_runtime,
        initial_active_baseline,
        initial_latest_snapshot,
        final_runtime: result.final_runtime,
        final_active_baseline: result.final_active_baseline,
        final_latest_snapshot: result.final_latest_snapshot,
        startup: result.startup,
        task_id: Some(task_id),
        task_event_seq: Some(base64::engine::general_purpose::STANDARD.encode(enqueued.as_bytes())),
        task_state: result.task_state,
        post_finish: result.post_finish,
        cycles: result.cycles,
    })
}

struct PumpResult {
    cycles: Vec<CycleSample>,
    startup: StartupSummary,
    final_runtime: aos_fdb::WorldRuntimeInfo,
    final_active_baseline: SnapshotMeta,
    final_latest_snapshot: Option<SnapshotMeta>,
    task_state: Option<TaskState>,
    post_finish: Option<PostFinishSummary>,
}

fn pump_world(
    facade: &ControlFacade<FdbWorldPersistence>,
    persistence: &Arc<FdbWorldPersistence>,
    supervisor: &mut WorkerSupervisor<FdbWorldPersistence>,
    resolved: &ResolvedTarget,
    worker_args: &ProbeWorkerArgs,
    start: Instant,
    task_id: Option<String>,
    task_key: Option<Vec<u8>>,
    post_finish_config: Option<PostFinishProbeConfig>,
) -> anyhow::Result<PumpResult> {
    let mut cycles = Vec::new();
    let mut startup = StartupSummary {
        first_world_started_ms: None,
        first_latest_snapshot_advance_ms: None,
        first_active_baseline_advance_ms: None,
        first_idle_ms: None,
    };
    let initial_active_baseline = persistence
        .snapshot_active_baseline(resolved.universe_id, resolved.world_id)?
        .height;
    let initial_latest_snapshot = persistence
        .snapshot_latest(resolved.universe_id, resolved.world_id)
        .ok()
        .map(|record| record.height);
    let mut last_task_status: Option<String> = None;
    let mut final_task_state = None;
    let mut post_finish_tracker = post_finish_config.map(|_| PostFinishTracker::default());

    for cycle_idx in 0..worker_args.max_cycles {
        let cycle_started = Instant::now();
        let outcome = supervisor.run_once_blocking()?;
        let run_once_ms = cycle_started.elapsed().as_millis();

        let runtime = persistence.world_runtime_info(
            resolved.universe_id,
            resolved.world_id,
            aos_runtime::now_wallclock_ns(),
        )?;
        let active_baseline =
            persistence.snapshot_active_baseline(resolved.universe_id, resolved.world_id)?;
        let latest_snapshot = persistence
            .snapshot_latest(resolved.universe_id, resolved.world_id)
            .ok();
        let segment_count = persistence
            .segment_index_read_from(resolved.universe_id, resolved.world_id, 0, u32::MAX)?
            .len();

        if startup.first_world_started_ms.is_none() && outcome.worlds_started > 0 {
            startup.first_world_started_ms = Some(start.elapsed().as_millis());
        }
        if startup.first_active_baseline_advance_ms.is_none()
            && active_baseline.height > initial_active_baseline
        {
            startup.first_active_baseline_advance_ms = Some(start.elapsed().as_millis());
        }
        if startup.first_latest_snapshot_advance_ms.is_none()
            && latest_snapshot
                .as_ref()
                .is_some_and(|record| Some(record.height) > initial_latest_snapshot)
        {
            startup.first_latest_snapshot_advance_ms = Some(start.elapsed().as_millis());
        }

        let task_state = if let Some(key) = task_key.as_ref() {
            read_demiurge_task_state(facade, resolved.universe_id, resolved.world_id, key)?
        } else {
            None
        };
        let debug_state = supervisor.active_world_debug_state(ActiveWorldRef {
            universe_id: resolved.universe_id,
            world_id: resolved.world_id,
        });
        let persisted_outstanding_intent_hashes = persistence
            .outstanding_intent_hashes_for_world(
                resolved.universe_id,
                resolved.world_id,
                aos_runtime::now_wallclock_ns(),
            )?
            .into_iter()
            .map(|hash: [u8; 32]| format_hash(&hash))
            .collect::<Vec<_>>();
        let runtime_known_intent_hashes =
            runtime_known_intent_hashes_from_debug_state(debug_state.as_ref());
        if let Some(state) = task_state.as_ref() {
            let status_changed = state.status != last_task_status;
            last_task_status = state.status.clone();
            if status_changed {
                tracing::info!(
                    cycle = cycle_idx,
                    elapsed_ms = start.elapsed().as_millis(),
                    task_id = task_id.as_deref().unwrap_or(""),
                    task_status = state.status.as_deref().unwrap_or("unknown"),
                    task_finished = state.finished.unwrap_or(false),
                    "hosted probe observed task state change"
                );
            }
        }

        let sample = CycleSample {
            cycle: cycle_idx,
            elapsed_ms: start.elapsed().as_millis(),
            run_once_ms,
            worlds_started: outcome.worlds_started,
            worlds_released: outcome.worlds_released,
            worlds_fenced: outcome.worlds_fenced,
            active_worlds: outcome.active_worlds,
            journal_head: persistence.journal_head(resolved.universe_id, resolved.world_id)?,
            segment_count,
            active_baseline_height: Some(active_baseline.height),
            latest_snapshot_height: latest_snapshot.as_ref().map(|record| record.height),
            active_world_loaded: debug_state.is_some(),
            has_pending_inbox: runtime.has_pending_inbox,
            has_pending_effects: runtime.has_pending_effects,
            has_pending_maintenance: runtime.has_pending_maintenance,
            persisted_outstanding_intent_hashes,
            runtime_known_intent_hashes,
            pending_receipt_intent_hashes: debug_state
                .as_ref()
                .map(|state| state.pending_receipt_intent_hashes.clone())
                .unwrap_or_default(),
            pending_receipts: debug_state
                .as_ref()
                .map(|state| state.pending_receipts.clone())
                .unwrap_or_default(),
            queued_effect_intent_hashes: debug_state
                .as_ref()
                .map(|state| state.queued_effect_intent_hashes.clone())
                .unwrap_or_default(),
            queued_effects: debug_state
                .as_ref()
                .map(|state| state.queued_effects.clone())
                .unwrap_or_default(),
            inflight_workflow_instances: debug_state
                .as_ref()
                .map(|state| state.workflow_instances.clone())
                .unwrap_or_default(),
            task_status: task_state.as_ref().and_then(|state| state.status.clone()),
            task_finished: task_state.as_ref().and_then(|state| state.finished),
        };
        let task_finished = sample.task_finished.unwrap_or(false);
        if let Some(tracker) = post_finish_tracker.as_mut() {
            tracker.observe(&sample, task_finished);
        }
        cycles.push(sample);

        let idle = !runtime.has_pending_inbox
            && !runtime.has_pending_effects
            && !runtime.has_pending_maintenance
            && outcome.active_worlds == 0;
        if startup.first_idle_ms.is_none() && idle {
            startup.first_idle_ms = Some(start.elapsed().as_millis());
        }

        if let Some(state) = task_state {
            final_task_state = Some(state.clone());
            if state.finished.unwrap_or(false)
                && post_finish_tracker
                    .as_ref()
                    .zip(post_finish_config)
                    .is_none_or(|(tracker, config)| !tracker.should_continue(config, cycle_idx))
            {
                break;
            }
        } else if task_key.is_none() && idle {
            break;
        }

        if worker_args.sleep_ms > 0 {
            std::thread::sleep(Duration::from_millis(worker_args.sleep_ms));
        }
    }

    let post_finish = match (post_finish_tracker, post_finish_config) {
        (Some(tracker), Some(config)) => Some(tracker.build_summary(
            &cycles,
            facade,
            resolved.universe_id,
            resolved.world_id,
            config,
        )?),
        _ => None,
    };

    Ok(PumpResult {
        cycles,
        startup,
        final_runtime: persistence.world_runtime_info(
            resolved.universe_id,
            resolved.world_id,
            aos_runtime::now_wallclock_ns(),
        )?,
        final_active_baseline: to_snapshot_meta(
            persistence.snapshot_active_baseline(resolved.universe_id, resolved.world_id)?,
        ),
        final_latest_snapshot: snapshot_latest_meta(
            &**persistence,
            resolved.universe_id,
            resolved.world_id,
        )?,
        task_state: final_task_state,
        post_finish,
    })
}

fn read_demiurge_task_state(
    facade: &ControlFacade<FdbWorldPersistence>,
    universe: UniverseId,
    world: WorldId,
    key: &[u8],
) -> anyhow::Result<Option<TaskState>> {
    let response = match facade.state_get(
        universe,
        world,
        "demiurge/Demiurge@1",
        Some(key.to_vec()),
        None,
    ) {
        Ok(response) => response,
        Err(ControlError::NotFound(_)) => return Ok(None),
        Err(ControlError::Persist(aos_fdb::PersistError::NotFound(_))) => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let Some(state_b64) = response.state_b64 else {
        return Ok(None);
    };
    let bytes = BASE64_STANDARD.decode(state_b64)?;
    let state: Value = serde_cbor::from_slice(&bytes).context("decode demiurge task state CBOR")?;
    let status = state.get("status").and_then(extract_tagged_name);
    let finished = state.get("finished").and_then(Value::as_bool);
    Ok(Some(TaskState {
        status,
        finished,
        state,
    }))
}

fn extract_tagged_name(value: &Value) -> Option<String> {
    if let Some(tag) = value.as_str() {
        return Some(tag.to_owned());
    }
    value
        .get("$tag")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn build_demiurge_task_event(task_id: &str, args: &DemiurgeTaskCommand) -> anyhow::Result<Vec<u8>> {
    if !args.workdir.is_absolute() {
        return Err(anyhow!("--workdir must be an absolute path"));
    }
    Ok(serde_cbor::to_vec(&json!({
        "task_id": task_id,
        "observed_at_ns": 1u64,
        "workdir": args.workdir,
        "task": args.task,
        "config": {
            "provider": args.provider,
            "model": args.model,
            "reasoning_effort": Value::Null,
            "max_tokens": args.max_tokens,
            "tool_profile": args.tool_profile,
            "allowed_tools": Value::Null,
            "tool_enable": Value::Null,
            "tool_disable": Value::Null,
            "tool_force": Value::Null,
            "session_ttl_ns": Value::Null
        }
    }))?)
}

fn build_supervisor(
    persistence: Arc<FdbWorldPersistence>,
    args: &ProbeWorkerArgs,
    universe: UniverseId,
    world_config: aos_runtime::WorldConfig,
) -> WorkerSupervisor<FdbWorldPersistence> {
    let mut config = FdbWorkerConfig::from_env().unwrap_or_default();
    config.worker_id = args.worker_id.clone();
    config.universe_filter = BTreeSet::from([universe]);
    let pins = collect_worker_pins(args.worker_pins.clone());
    if !pins.is_empty() {
        config.worker_pins = pins;
    }
    if let Some(value) = args.max_effects_per_cycle {
        config.max_effects_per_cycle = value;
    }
    if let Some(value) = args.max_inbox_batch {
        config.max_inbox_batch = value;
    }
    if let Some(value) = args.max_tick_steps_per_cycle {
        config.max_tick_steps_per_cycle = value;
    }
    let mut worker = FdbWorker::new(config);
    worker.world_config = world_config;
    worker.with_runtime(persistence)
}

struct ResolvedTarget {
    universe_id: UniverseId,
    world_id: WorldId,
    world_handle: String,
}

fn resolve_target(
    facade: &ControlFacade<FdbWorldPersistence>,
    target: &TargetArgs,
) -> anyhow::Result<ResolvedTarget> {
    let universe_id = resolve_universe(facade, &target.universe)?
        .record
        .universe_id;
    let world = target
        .world
        .parse::<WorldId>()
        .map(|world_id| facade.get_world(universe_id, world_id))
        .unwrap_or_else(|_| facade.get_world_by_handle(universe_id, &target.world))
        .with_context(|| format!("resolve world '{}'", target.world))?;
    Ok(ResolvedTarget {
        universe_id,
        world_id: world.runtime.world_id,
        world_handle: world.runtime.meta.handle,
    })
}

fn resolve_universe(
    facade: &ControlFacade<FdbWorldPersistence>,
    universe: &str,
) -> anyhow::Result<aos_node_hosted::control::UniverseSummaryResponse> {
    universe
        .parse::<UniverseId>()
        .map(|universe_id| facade.get_universe(universe_id))
        .unwrap_or_else(|_| facade.get_universe_by_handle(universe))
        .with_context(|| format!("resolve universe '{universe}'"))
        .map_err(Into::into)
}

fn ensure_universe_handle(
    facade: &ControlFacade<FdbWorldPersistence>,
    handle: &str,
) -> anyhow::Result<aos_node_hosted::control::UniverseSummaryResponse> {
    match facade.get_universe_by_handle(handle) {
        Ok(record) => Ok(record),
        Err(ControlError::NotFound(_)) => facade
            .create_universe(CreateUniverseBody {
                universe_id: Some(UniverseId::from(Uuid::new_v4())),
                handle: Some(handle.to_string()),
                created_at_ns: 0,
            })
            .map(
                |created| aos_node_hosted::control::UniverseSummaryResponse {
                    record: created.record,
                },
            )
            .map_err(Into::into),
        Err(err) => Err(err.into()),
    }
}

fn ensure_demiurge_secret_bindings(
    facade: &ControlFacade<FdbWorldPersistence>,
    universe: UniverseId,
    provider: &str,
) -> anyhow::Result<()> {
    ensure_worker_env_secret_binding(facade, universe, "llm/openai_api", "OPENAI_API_KEY", false)?;
    ensure_worker_env_secret_binding(
        facade,
        universe,
        "llm/anthropic_api",
        "ANTHROPIC_API_KEY",
        false,
    )?;

    if provider.starts_with("openai") {
        ensure_worker_env_secret_binding(
            facade,
            universe,
            "llm/openai_api",
            "OPENAI_API_KEY",
            true,
        )?;
    } else if provider.starts_with("anthropic") {
        ensure_worker_env_secret_binding(
            facade,
            universe,
            "llm/anthropic_api",
            "ANTHROPIC_API_KEY",
            true,
        )?;
    }
    Ok(())
}

fn ensure_worker_env_secret_binding(
    facade: &ControlFacade<FdbWorldPersistence>,
    universe: UniverseId,
    binding_id: &str,
    env_var: &str,
    required: bool,
) -> anyhow::Result<()> {
    let env_present = std::env::var_os(env_var).is_some();
    if required && !env_present {
        return Err(anyhow!(
            "required env var {env_var} is not set for binding {binding_id}"
        ));
    }
    if !env_present {
        return Ok(());
    }
    facade.put_secret_binding(
        universe,
        binding_id,
        PutSecretBindingBody {
            source_kind: aos_fdb::SecretBindingSourceKind::WorkerEnv,
            env_var: Some(env_var.to_string()),
            required_placement_pin: None,
            created_at_ns: 0,
            updated_at_ns: 0,
            status: Some(aos_fdb::SecretBindingStatus::Active),
            actor: Some("hosted_probe".into()),
        },
    )?;
    Ok(())
}

fn snapshot_latest_meta(
    persistence: &dyn WorldStore,
    universe: UniverseId,
    world: WorldId,
) -> anyhow::Result<Option<SnapshotMeta>> {
    match persistence.snapshot_latest(universe, world) {
        Ok(record) => Ok(Some(to_snapshot_meta(record))),
        Err(aos_fdb::PersistError::NotFound(_)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn to_snapshot_meta(record: SnapshotRecord) -> SnapshotMeta {
    SnapshotMeta {
        height: record.height,
        receipt_horizon_height: record.receipt_horizon_height,
        snapshot_ref: record.snapshot_ref,
        manifest_hash: record.manifest_hash,
    }
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

fn load_world_dotenv(world_root: &Path) -> anyhow::Result<()> {
    let path = world_root.join(".env");
    if !path.exists() {
        return Ok(());
    }
    for item in dotenvy::from_path_iter(&path)? {
        let (key, val) = item?;
        if std::env::var_os(&key).is_none() {
            unsafe {
                std::env::set_var(&key, &val);
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

fn collect_worker_pins(pins: Vec<String>) -> BTreeSet<String> {
    pins.into_iter()
        .map(|pin| pin.trim().to_owned())
        .filter(|pin| !pin.is_empty())
        .collect()
}

fn default_worker_pins() -> Vec<String> {
    FdbWorkerConfig::default().worker_pins.into_iter().collect()
}

fn default_worker_id() -> String {
    FdbWorkerConfig::default().worker_id
}

fn default_workdir() -> PathBuf {
    workspace_root()
}

fn default_demiurge_world_root() -> PathBuf {
    workspace_root().join("worlds/demiurge")
}

fn default_sleep_ms() -> u64 {
    FdbWorkerConfig::default()
        .supervisor_poll_interval
        .as_millis() as u64
}

fn format_hash(bytes: &[u8]) -> String {
    Hash::from_bytes(bytes)
        .expect("intent hash is 32 bytes")
        .to_hex()
}

fn runtime_known_intent_hashes_from_debug_state(
    debug_state: Option<&aos_node_hosted::ActiveWorldDebugState>,
) -> Vec<String> {
    let mut hashes = BTreeSet::new();
    if let Some(state) = debug_state {
        hashes.extend(state.pending_receipt_intent_hashes.iter().cloned());
        hashes.extend(state.queued_effect_intent_hashes.iter().cloned());
        for workflow in &state.workflow_instances {
            hashes.extend(workflow.inflight_intent_hashes.iter().cloned());
        }
    }
    hashes.into_iter().collect()
}

fn inflight_intent_hashes(sample: &CycleSample) -> Vec<String> {
    let mut hashes = BTreeSet::new();
    for workflow in &sample.inflight_workflow_instances {
        hashes.extend(workflow.inflight_intent_hashes.iter().cloned());
    }
    hashes.into_iter().collect()
}

fn hash_diff(left: &[String], right: &[String]) -> Vec<String> {
    let right: BTreeSet<_> = right.iter().collect();
    left.iter()
        .filter(|hash| !right.contains(hash))
        .cloned()
        .collect()
}

fn cycle_focus(sample: &CycleSample) -> CycleFocus {
    let inflight_intent_hashes = inflight_intent_hashes(sample);
    CycleFocus {
        cycle: sample.cycle,
        elapsed_ms: sample.elapsed_ms,
        journal_head: sample.journal_head,
        segment_count: sample.segment_count,
        active_world_loaded: sample.active_world_loaded,
        has_pending_inbox: sample.has_pending_inbox,
        has_pending_effects: sample.has_pending_effects,
        has_pending_maintenance: sample.has_pending_maintenance,
        persisted_outstanding_intent_hashes: sample.persisted_outstanding_intent_hashes.clone(),
        runtime_known_intent_hashes: sample.runtime_known_intent_hashes.clone(),
        runtime_only_intent_hashes: hash_diff(
            &sample.runtime_known_intent_hashes,
            &sample.persisted_outstanding_intent_hashes,
        ),
        persisted_only_intent_hashes: hash_diff(
            &sample.persisted_outstanding_intent_hashes,
            &sample.runtime_known_intent_hashes,
        ),
        pending_receipt_intent_hashes: sample.pending_receipt_intent_hashes.clone(),
        pending_receipts: sample.pending_receipts.clone(),
        queued_effect_intent_hashes: sample.queued_effect_intent_hashes.clone(),
        queued_effects: sample.queued_effects.clone(),
        inflight_intent_hashes,
    }
}

fn classify_resurrection(focus: &CycleFocus) -> Option<&'static str> {
    let runtime = !focus.runtime_known_intent_hashes.is_empty();
    let persisted = !focus.persisted_outstanding_intent_hashes.is_empty();
    match (runtime, persisted) {
        (false, false) => None,
        (true, false) => Some("runtime_only_reappeared"),
        (false, true) => Some("persisted_only_reappeared"),
        (true, true) if !focus.runtime_only_intent_hashes.is_empty() => {
            Some("runtime_and_persisted_reappeared_with_runtime_only_hashes")
        }
        (true, true) if !focus.persisted_only_intent_hashes.is_empty() => {
            Some("runtime_and_persisted_reappeared_with_persisted_only_hashes")
        }
        (true, true) => Some("runtime_and_persisted_reappeared_with_matching_hashes"),
    }
}

fn build_post_finish_diagnosis(
    cycles: &[CycleSample],
    finish_cycle: Option<u32>,
    first_post_finish_activity_cycle: Option<u32>,
    first_post_finish_segment_cycle: Option<u32>,
) -> Option<PostFinishDiagnosis> {
    let finish_cycle = finish_cycle?;
    let pre_segment = first_post_finish_segment_cycle.and_then(|segment_cycle| {
        cycles
            .iter()
            .rev()
            .find(|sample| sample.cycle > finish_cycle && sample.cycle < segment_cycle)
            .map(cycle_focus)
    });
    let first_segment = first_post_finish_segment_cycle.and_then(|segment_cycle| {
        cycles
            .iter()
            .find(|sample| sample.cycle == segment_cycle)
            .map(cycle_focus)
    });
    let first_activity = first_post_finish_activity_cycle.and_then(|activity_cycle| {
        cycles
            .iter()
            .find(|sample| sample.cycle == activity_cycle)
            .map(cycle_focus)
    });
    let resurrection_classification = first_activity
        .as_ref()
        .and_then(classify_resurrection)
        .or_else(|| first_segment.as_ref().and_then(classify_resurrection));
    Some(PostFinishDiagnosis {
        pre_segment,
        first_segment,
        first_activity,
        resurrection_classification,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(cycle: u32, journal_head: u64, segment_count: usize) -> CycleSample {
        CycleSample {
            cycle,
            elapsed_ms: cycle as u128,
            run_once_ms: 0,
            worlds_started: 0,
            worlds_released: 0,
            worlds_fenced: 0,
            active_worlds: 0,
            journal_head,
            segment_count,
            active_baseline_height: None,
            latest_snapshot_height: None,
            active_world_loaded: false,
            has_pending_inbox: false,
            has_pending_effects: false,
            has_pending_maintenance: false,
            persisted_outstanding_intent_hashes: Vec::new(),
            runtime_known_intent_hashes: Vec::new(),
            pending_receipt_intent_hashes: Vec::new(),
            pending_receipts: Vec::new(),
            queued_effect_intent_hashes: Vec::new(),
            queued_effects: Vec::new(),
            inflight_workflow_instances: Vec::new(),
            task_status: None,
            task_finished: None,
        }
    }

    #[test]
    fn post_finish_tracker_waits_requested_cycles_after_finish() {
        let config = PostFinishProbeConfig {
            continue_after_finish_cycles: 2,
            wait_for_segment_after_finish: false,
            continue_after_segment_cycles: 0,
            journal_before_finish: 0,
            journal_limit: 0,
        };
        let mut tracker = PostFinishTracker::default();

        tracker.observe(&sample(4, 40, 0), true);
        assert!(tracker.should_continue(config, 4));
        tracker.observe(&sample(5, 40, 0), true);
        assert!(tracker.should_continue(config, 5));
        tracker.observe(&sample(6, 40, 0), true);
        assert!(!tracker.should_continue(config, 6));
    }

    #[test]
    fn post_finish_tracker_waits_for_segment_then_extra_cycles() {
        let config = PostFinishProbeConfig {
            continue_after_finish_cycles: 0,
            wait_for_segment_after_finish: true,
            continue_after_segment_cycles: 2,
            journal_before_finish: 0,
            journal_limit: 0,
        };
        let mut tracker = PostFinishTracker::default();

        tracker.observe(&sample(3, 30, 0), true);
        assert!(tracker.should_continue(config, 3));
        tracker.observe(&sample(4, 30, 0), true);
        assert!(tracker.should_continue(config, 4));

        tracker.observe(&sample(5, 31, 1), true);
        assert_eq!(tracker.first_post_finish_activity_cycle, Some(5));
        assert_eq!(tracker.first_post_finish_segment_cycle, Some(5));
        assert!(tracker.should_continue(config, 5));

        tracker.observe(&sample(6, 31, 1), true);
        assert!(tracker.should_continue(config, 6));
        tracker.observe(&sample(7, 31, 1), true);
        assert!(!tracker.should_continue(config, 7));
    }

    #[test]
    fn build_post_finish_diagnosis_classifies_runtime_only_reappearance() {
        let mut cycles = vec![sample(3, 30, 0), sample(4, 30, 0), sample(5, 30, 1)];
        cycles[1].has_pending_maintenance = true;
        cycles[2].active_world_loaded = true;
        cycles[2].runtime_known_intent_hashes = vec!["abc".into()];
        cycles[2].pending_receipt_intent_hashes = vec!["abc".into()];

        let diagnosis = build_post_finish_diagnosis(&cycles, Some(3), Some(5), Some(5))
            .expect("diagnosis should be present");
        assert_eq!(
            diagnosis.resurrection_classification,
            Some("runtime_only_reappeared")
        );
        assert_eq!(
            diagnosis
                .first_activity
                .as_ref()
                .expect("first activity focus")
                .runtime_only_intent_hashes,
            vec!["abc".to_string()]
        );
    }
}
