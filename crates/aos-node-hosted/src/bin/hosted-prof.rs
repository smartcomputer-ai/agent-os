use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Once, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use aos_air_types::{AirNode, DefModule, EffectKind, Manifest, builtins};
use aos_authoring::bundle::import_genesis;
use aos_authoring::{WorkflowBuildProfile, build_bundle_from_local_world_with_profile};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effect_adapters::adapters::stub::StubHttpAdapter;
use aos_effect_adapters::adapters::traits::AsyncEffectAdapter;
use aos_effect_types::{HashRef as ReceiptHashRef, HttpRequestReceipt, RequestTimings};
use aos_effects::{EffectIntent, ReceiptStatus};
use aos_kernel::ManifestLoader;
use aos_kernel::Store;
use aos_kernel::journal::JournalRecord;
use aos_node::{CborPayload, CreateWorldRequest, CreateWorldSource, ReceiptIngress, UniverseId};
use aos_node_hosted::blobstore::BlobStoreConfig;
use aos_node_hosted::config::{HostedWorkerConfig, ProjectionCommitMode};
use aos_node_hosted::kafka::{HostedKafkaBackend, KafkaConfig};
use aos_node_hosted::{
    HostedWorker, HostedWorkerRuntime, HostedWorldSummary, SubmitEventRequest,
    SupervisorRunProfile, WorkerSupervisorHandle, load_dotenv_candidates,
};
use clap::{Parser, ValueEnum};
use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::config::ClientConfig;
use rdkafka::error::RDKafkaErrorCode;
use serde_json::json;
use tokio::task::JoinHandle;

const WAIT_DEADLINE: Duration = Duration::from_secs(120);
const WAIT_SLEEP: Duration = Duration::from_millis(1);

#[derive(Parser, Debug)]
#[command(name = "hosted-prof")]
#[command(about = "Profile hosted worker paths with per-stage and per-run-loop timing.")]
struct Args {
    #[arg(long, value_enum, default_value_t = Scenario::CounterStart)]
    scenario: Scenario,

    #[arg(long, value_enum, default_value_t = RuntimeKind::Direct)]
    runtime: RuntimeKind,

    #[arg(long, default_value_t = 1)]
    iterations: usize,

    #[arg(long, default_value_t = 1)]
    partition_count: u32,

    #[arg(long, default_value_t = 100)]
    messages: usize,

    #[arg(long, default_value_t = 0)]
    checkpoint_every_events: u32,

    #[arg(long, default_value_t = 64)]
    max_local_continuation_slices_per_flush: u32,

    #[arg(long, default_value_t = 256)]
    max_uncommitted_slices_per_world: u32,

    #[arg(long, value_enum, default_value_t = ProjectionCommitModeArg::Background)]
    projection_commit_mode: ProjectionCommitModeArg,

    #[arg(long, default_value_t = false)]
    unsafe_no_flush: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Scenario {
    CounterStart,
    FetchNotifyReceipt,
    FetchNotifyThroughput,
    CounterThroughput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum RuntimeKind {
    Embedded,
    Broker,
    Direct,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum ProjectionCommitModeArg {
    Inline,
    Background,
}

impl From<ProjectionCommitModeArg> for ProjectionCommitMode {
    fn from(value: ProjectionCommitModeArg) -> Self {
        match value {
            ProjectionCommitModeArg::Inline => ProjectionCommitMode::Inline,
            ProjectionCommitModeArg::Background => ProjectionCommitMode::Background,
        }
    }
}

#[derive(Clone)]
struct PreparedAuthoredManifest {
    blobs: Vec<Vec<u8>>,
    manifest_bytes: Vec<u8>,
}

#[derive(Clone, Default)]
struct RunProfileTotals {
    calls: u64,
    total: Duration,
    sync_assignments: Duration,
    sync_active_worlds: Duration,
    run_partitions: Duration,
    publish_checkpoints: Duration,
    partition_drain_submissions: Duration,
    partition_process_create: Duration,
    partition_process_existing: Duration,
    partition_activate_world: Duration,
    partition_apply_submission: Duration,
    partition_build_external_event: Duration,
    partition_host_drain: Duration,
    partition_post_apply: Duration,
    partition_commit_batch: Duration,
    partition_commit_command_records: Duration,
    partition_promote_worlds: Duration,
    partition_inline_checkpoint: Duration,
}

impl RunProfileTotals {
    fn add(&mut self, profile: SupervisorRunProfile) {
        self.calls += 1;
        self.total += profile.total;
        self.sync_assignments += profile.sync_assignments;
        self.sync_active_worlds += profile.sync_active_worlds;
        self.run_partitions += profile.run_partitions;
        self.publish_checkpoints += profile.publish_checkpoints;
        self.partition_drain_submissions += profile.partition_drain_submissions;
        self.partition_process_create += profile.partition_process_create;
        self.partition_process_existing += profile.partition_process_existing;
        self.partition_activate_world += profile.partition_activate_world;
        self.partition_apply_submission += profile.partition_apply_submission;
        self.partition_build_external_event += profile.partition_build_external_event;
        self.partition_host_drain += profile.partition_host_drain;
        self.partition_post_apply += profile.partition_post_apply;
        self.partition_commit_batch += profile.partition_commit_batch;
        self.partition_commit_command_records += profile.partition_commit_command_records;
        self.partition_promote_worlds += profile.partition_promote_worlds;
        self.partition_inline_checkpoint += profile.partition_inline_checkpoint;
    }
}

#[derive(Clone, Default)]
struct StageTiming {
    name: String,
    elapsed: Duration,
    cycles: u64,
    probe_time: Duration,
    sleep_time: Duration,
    run: RunProfileTotals,
    message_count: Option<usize>,
    throughput_msgs_per_sec: Option<f64>,
    note: Option<String>,
}

impl StageTiming {
    fn new(name: impl Into<String>, elapsed: Duration) -> Self {
        Self {
            name: name.into(),
            elapsed,
            ..Self::default()
        }
    }
}

#[derive(Clone, Default)]
struct IterationReport {
    stages: Vec<StageTiming>,
    total: Duration,
}

impl IterationReport {
    fn push(&mut self, stage: StageTiming) {
        self.total += stage.elapsed;
        self.stages.push(stage);
    }
}

struct RuntimePair {
    control: HostedWorkerRuntime,
    worker: HostedWorkerRuntime,
    ctx: Option<BrokerRuntimeContext>,
}

#[derive(Clone)]
struct BrokerRuntimeContext {
    kafka_config: KafkaConfig,
    blobstore_config: BlobStoreConfig,
    partition_count: u32,
}

impl BrokerRuntimeContext {
    fn worker_runtime(&self, label: &str) -> Result<HostedWorkerRuntime> {
        self.runtime_with_kafka(label, worker_kafka_config_for_label(self, label))
    }

    fn control_runtime(&self, label: &str) -> Result<HostedWorkerRuntime> {
        self.runtime_with_kafka(label, control_kafka_config_for_label(self, label))
    }

    fn direct_worker_runtime(
        &self,
        label: &str,
        partitions: &[u32],
    ) -> Result<HostedWorkerRuntime> {
        let mut kafka_config = worker_kafka_config_for_label(self, label);
        kafka_config.direct_assigned_partitions = partitions.iter().copied().collect();
        kafka_config.submission_group_prefix =
            format!("{}-direct-{label}", kafka_config.submission_group_prefix);
        kafka_config.transactional_id = format!("{}-direct-{label}", kafka_config.transactional_id);
        self.runtime_with_kafka(label, kafka_config)
    }

    fn runtime_with_kafka(
        &self,
        label: &str,
        kafka_config: KafkaConfig,
    ) -> Result<HostedWorkerRuntime> {
        Ok(HostedWorkerRuntime::new_broker_with_state_root(
            self.partition_count,
            temp_state_root(&format!("prof-{label}")),
            kafka_config,
            self.blobstore_config.clone(),
        )?)
    }
}

#[derive(Debug, Clone, Copy)]
struct LatencyStats {
    min: Duration,
    avg: Duration,
    p50: Duration,
    p95: Duration,
    p99: Duration,
    max: Duration,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    load_dotenv_candidates()?;
    let args = Args::parse();

    if args.unsafe_no_flush
        && matches!(
            args.scenario,
            Scenario::FetchNotifyReceipt | Scenario::FetchNotifyThroughput
        )
    {
        bail!("--unsafe-no-flush is only supported for counter scenarios");
    }

    let mut reports = Vec::with_capacity(args.iterations);
    for iteration in 0..args.iterations {
        let report = run_iteration(&args, iteration).await?;
        print_iteration(iteration, &report);
        reports.push(report);
    }
    print_summary(&reports);
    Ok(())
}

async fn run_iteration(args: &Args, iteration: usize) -> Result<IterationReport> {
    let mut report = IterationReport::default();
    let universe_id = UniverseId::from(uuid::Uuid::new_v4());

    let runtime_started = Instant::now();
    let runtimes = create_runtimes(args.runtime, args.partition_count, iteration).await?;
    report.push(StageTiming::new(
        "runtime_create",
        runtime_started.elapsed(),
    ));

    if args.unsafe_no_flush {
        let unsafe_started = Instant::now();
        runtimes.worker.debug_skip_flush_commit()?;
        let mut stage = StageTiming::new("mode.unsafe_no_flush", unsafe_started.elapsed());
        stage.note = Some(
            "Kafka flush commit bypassed; results are not durable and are only for profiling"
                .into(),
        );
        report.push(stage);
    }

    let worker = HostedWorker::new(worker_config(args));
    match args.scenario {
        Scenario::CounterStart => {
            run_counter_start(
                args,
                &runtimes,
                &worker,
                universe_id,
                iteration,
                &mut report,
            )
            .await?
        }
        Scenario::FetchNotifyReceipt => {
            run_fetch_notify_receipt(&runtimes, &worker, universe_id, iteration, &mut report)
                .await?
        }
        Scenario::FetchNotifyThroughput => {
            run_fetch_notify_throughput(
                args,
                &runtimes,
                &worker,
                universe_id,
                iteration,
                &mut report,
            )
            .await?
        }
        Scenario::CounterThroughput => {
            run_counter_throughput(
                args,
                &runtimes,
                &worker,
                universe_id,
                iteration,
                &mut report,
            )
            .await?
        }
    }

    Ok(report)
}

async fn run_counter_start(
    args: &Args,
    runtimes: &RuntimePair,
    worker: &HostedWorker,
    universe_id: UniverseId,
    iteration: usize,
    report: &mut IterationReport,
) -> Result<()> {
    let control_runtime = profiling_control_runtime(args, runtimes);
    let mut supervisor = spawn_profiled_worker(worker, runtimes.worker.clone(), report)?;

    let manifest_started = Instant::now();
    let manifest_hash = upload_counter_manifest(control_runtime, universe_id)?;
    report.push(StageTiming::new(
        "manifest_upload.counter",
        manifest_started.elapsed(),
    ));

    let create_started = Instant::now();
    let accepted = control_runtime.create_world(
        universe_id,
        CreateWorldRequest {
            world_id: None,
            universe_id,
            created_at_ns: 1,
            source: CreateWorldSource::Manifest { manifest_hash },
        },
    )?;
    report.push(StageTiming::new(
        "world_create_submit",
        create_started.elapsed(),
    ));

    let (send_world, stage) =
        wait_stage(
            "sender_route_sync",
            &mut supervisor,
            || match control_runtime.get_world(universe_id, accepted.world_id) {
                Ok(world) => Ok(Some(world)),
                Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
                Err(err) => Err(err.into()),
            },
        )
        .await?;
    report.push(stage);

    let (world, stage) = wait_stage("world_register_wait", &mut supervisor, || {
        match runtimes.worker.get_world(universe_id, accepted.world_id) {
            Ok(world) => Ok(Some(world)),
            Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
            Err(err) => Err(err.into()),
        }
    })
    .await?;
    report.push(stage);

    let submit_started = Instant::now();
    submit_counter_start(
        control_runtime,
        &send_world,
        format!("prof-counter-{iteration}"),
        1,
    )?;
    report.push(StageTiming::new("event_submit", submit_started.elapsed()));

    let (_state, stage) = wait_stage("event_to_state_wait", &mut supervisor, || {
        let state = runtimes.worker.state_json(
            world.universe_id,
            world.world_id,
            "demo/CounterSM@1",
            None,
        )?;
        Ok(state.filter(|state| {
            state["pc"] == json!({ "$tag": "Counting" }) && state["remaining"] == json!(1)
        }))
    })
    .await?;
    report.push(stage);

    supervisor.shutdown().await?;
    Ok(())
}

async fn run_fetch_notify_receipt(
    runtimes: &RuntimePair,
    worker: &HostedWorker,
    universe_id: UniverseId,
    iteration: usize,
    report: &mut IterationReport,
) -> Result<()> {
    let ctx = runtimes.ctx.as_ref().ok_or_else(|| {
        anyhow!("fetch-notify receipt profiling requires broker or direct runtime")
    })?;
    let mut supervisor = spawn_profiled_worker(worker, runtimes.worker.clone(), report)?;

    let manifest_started = Instant::now();
    let manifest_hash = upload_fetch_notify_manifest(&runtimes.control, universe_id)?;
    report.push(StageTiming::new(
        "manifest_upload.fetch_notify",
        manifest_started.elapsed(),
    ));

    let create_started = Instant::now();
    let accepted = runtimes.control.create_world(
        universe_id,
        CreateWorldRequest {
            world_id: None,
            universe_id,
            created_at_ns: 1,
            source: CreateWorldSource::Manifest { manifest_hash },
        },
    )?;
    report.push(StageTiming::new(
        "world_create_submit",
        create_started.elapsed(),
    ));

    let (send_world, stage) = wait_stage("sender_route_sync", &mut supervisor, || {
        match runtimes.control.get_world(universe_id, accepted.world_id) {
            Ok(world) => Ok(Some(world)),
            Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
            Err(err) => Err(err.into()),
        }
    })
    .await?;
    report.push(stage);

    let (world, stage) = wait_stage("world_register_wait", &mut supervisor, || {
        match runtimes.worker.get_world(universe_id, accepted.world_id) {
            Ok(world) => Ok(Some(world)),
            Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
            Err(err) => Err(err.into()),
        }
    })
    .await?;
    report.push(stage);

    let submit_started = Instant::now();
    submit_fetch_notify_start(
        &runtimes.control,
        &send_world,
        format!("prof-fetch-start-{iteration}"),
    )?;
    report.push(StageTiming::new("event_submit", submit_started.elapsed()));

    let (_trace, stage) = wait_stage("event_to_pending_receipt_wait", &mut supervisor, || {
        let trace = runtimes
            .worker
            .trace_summary(world.universe_id, world.world_id)?;
        Ok(trace["runtime_wait"]["pending_workflow_receipts"]
            .as_u64()
            .is_some_and(|count| count > 0)
            .then_some(trace))
    })
    .await?;
    report.push(stage);

    let intent_started = Instant::now();
    let intent_hash = wait_for_effect_intent_hash(ctx, &world).await?;
    report.push(StageTiming::new(
        "intent_hash_discovery",
        intent_started.elapsed(),
    ));

    let receipt_started = Instant::now();
    let receipt_payload = HttpRequestReceipt {
        status: 200,
        headers: Default::default(),
        body_ref: Some(
            ReceiptHashRef::new(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
        ),
        timings: RequestTimings {
            start_ns: 10,
            end_ns: 20,
        },
        adapter_id: "adapter.http.prof".into(),
    };
    runtimes.control.submit_receipt(
        world.universe_id,
        world.world_id,
        ReceiptIngress {
            intent_hash: intent_hash.to_vec(),
            effect_kind: "http.request".into(),
            adapter_id: "adapter.http.prof".into(),
            status: ReceiptStatus::Ok,
            payload: CborPayload::inline(serde_cbor::to_vec(&receipt_payload)?),
            cost_cents: Some(0),
            signature: vec![1, 2, 3],
            correlation_id: Some(format!("prof-fetch-receipt-{iteration}")),
        },
    )?;
    report.push(StageTiming::new(
        "receipt_submit",
        receipt_started.elapsed(),
    ));

    let (_state, stage) = wait_stage("receipt_to_done_wait", &mut supervisor, || {
        let state = runtimes.worker.state_json(
            world.universe_id,
            world.world_id,
            "demo/FetchNotify@1",
            None,
        )?;
        Ok(state.filter(|state| {
            state["pc"] == json!({ "$tag": "Done" }) && state["last_status"] == json!(200)
        }))
    })
    .await?;
    report.push(stage);

    supervisor.shutdown().await?;
    Ok(())
}

async fn run_fetch_notify_throughput(
    args: &Args,
    runtimes: &RuntimePair,
    worker: &HostedWorker,
    universe_id: UniverseId,
    iteration: usize,
    report: &mut IterationReport,
) -> Result<()> {
    if args.runtime != RuntimeKind::Broker {
        bail!("fetch-notify throughput scenario requires --runtime broker");
    }
    if args.messages == 0 {
        bail!("fetch-notify throughput scenario requires --messages >= 1");
    }
    let ctx = runtimes
        .ctx
        .as_ref()
        .ok_or_else(|| anyhow!("fetch-notify throughput scenario requires broker runtime"))?;
    let mut supervisor = spawn_profiled_worker(worker, runtimes.worker.clone(), report)?;

    let manifest_started = Instant::now();
    let manifest_hash = upload_fetch_notify_manifest(&runtimes.control, universe_id)?;
    report.push(StageTiming::new(
        "manifest_upload.fetch_notify",
        manifest_started.elapsed(),
    ));

    let create_started = Instant::now();
    let accepted = runtimes.control.create_world(
        universe_id,
        CreateWorldRequest {
            world_id: None,
            universe_id,
            created_at_ns: 1,
            source: CreateWorldSource::Manifest { manifest_hash },
        },
    )?;
    report.push(StageTiming::new(
        "world_create_submit",
        create_started.elapsed(),
    ));

    let (send_world, stage) = wait_stage("sender_route_sync", &mut supervisor, || {
        match runtimes.control.get_world(universe_id, accepted.world_id) {
            Ok(world) => Ok(Some(world)),
            Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
            Err(err) => Err(err.into()),
        }
    })
    .await?;
    report.push(stage);

    let (world, stage) = wait_stage("world_register_wait", &mut supervisor, || {
        match runtimes.worker.get_world(universe_id, accepted.world_id) {
            Ok(world) => Ok(Some(world)),
            Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
            Err(err) => Err(err.into()),
        }
    })
    .await?;
    report.push(stage);

    let adapter_started = Instant::now();
    let adapter_handle =
        spawn_stub_http_adapter_pump(ctx.clone(), runtimes.control.clone(), world.clone());
    report.push(StageTiming::new(
        "adapter_task_spawn",
        adapter_started.elapsed(),
    ));

    let result = async {
        let warm_submit_started = Instant::now();
        submit_fetch_notify_start(
            &runtimes.control,
            &send_world,
            format!("prof-fetch-throughput-warm-{iteration}"),
        )?;
        report.push(StageTiming::new(
            "warm_event_submit",
            warm_submit_started.elapsed(),
        ));

        let (_, stage) = wait_stage("warm_event_end_to_end", &mut supervisor, || {
            let state = runtimes.worker.state_json(
                world.universe_id,
                world.world_id,
                "demo/FetchNotify@1",
                None,
            )?;
            Ok(state.filter(|state| fetch_notify_done(state, 1)))
        })
        .await?;
        report.push(stage);

        let stream_started = Instant::now();
        let mut stream_stage = StageTiming {
            name: "steady_state_stream".into(),
            ..StageTiming::default()
        };
        let mut latencies = Vec::with_capacity(args.messages);
        for index in 0..args.messages {
            let event_started = Instant::now();
            submit_fetch_notify_start(
                &runtimes.control,
                &send_world,
                format!("prof-fetch-throughput-{iteration}-{index}"),
            )?;
            wait_until_fetch_notify_done_profiled(
                &runtimes.worker,
                &world,
                (index + 2) as u64,
                &mut supervisor,
                &mut stream_stage,
            )
            .await?;
            latencies.push(event_started.elapsed());
        }
        stream_stage.elapsed = stream_started.elapsed();
        let stats = latency_stats(&latencies);
        let throughput = if stream_stage.elapsed.is_zero() {
            0.0
        } else {
            args.messages as f64 / stream_stage.elapsed.as_secs_f64()
        };
        stream_stage.message_count = Some(args.messages);
        stream_stage.throughput_msgs_per_sec = Some(throughput);
        stream_stage.note = Some(format!(
            "latency_ms(min={} avg={} p50={} p95={} p99={} max={})",
            stats.min.as_millis(),
            stats.avg.as_millis(),
            stats.p50.as_millis(),
            stats.p95.as_millis(),
            stats.p99.as_millis(),
            stats.max.as_millis(),
        ));
        report.push(stream_stage);
        Result::<(), anyhow::Error>::Ok(())
    }
    .await;

    adapter_handle.abort();
    let _ = adapter_handle.await;
    supervisor.shutdown().await?;
    result
}

async fn run_counter_throughput(
    args: &Args,
    runtimes: &RuntimePair,
    worker: &HostedWorker,
    universe_id: UniverseId,
    iteration: usize,
    report: &mut IterationReport,
) -> Result<()> {
    if args.runtime != RuntimeKind::Broker {
        bail!("counter throughput scenario requires --runtime broker");
    }
    if args.messages == 0 {
        bail!("counter throughput scenario requires --messages >= 1");
    }

    let control_runtime = profiling_control_runtime(args, runtimes);
    let mut supervisor = spawn_profiled_worker(worker, runtimes.worker.clone(), report)?;

    let manifest_started = Instant::now();
    let manifest_hash = upload_counter_manifest(control_runtime, universe_id)?;
    report.push(StageTiming::new(
        "manifest_upload.counter",
        manifest_started.elapsed(),
    ));

    let create_started = Instant::now();
    let accepted = control_runtime.create_world(
        universe_id,
        CreateWorldRequest {
            world_id: None,
            universe_id,
            created_at_ns: 1,
            source: CreateWorldSource::Manifest { manifest_hash },
        },
    )?;
    report.push(StageTiming::new(
        "world_create_submit",
        create_started.elapsed(),
    ));

    let (send_world, stage) =
        wait_stage(
            "sender_route_sync",
            &mut supervisor,
            || match control_runtime.get_world(universe_id, accepted.world_id) {
                Ok(world) => Ok(Some(world)),
                Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
                Err(err) => Err(err.into()),
            },
        )
        .await?;
    report.push(stage);

    let (world, stage) = wait_stage("world_register_wait", &mut supervisor, || {
        match runtimes.worker.get_world(universe_id, accepted.world_id) {
            Ok(world) => Ok(Some(world)),
            Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
            Err(err) => Err(err.into()),
        }
    })
    .await?;
    report.push(stage);

    let warm_submit_started = Instant::now();
    submit_counter_start(
        control_runtime,
        &send_world,
        format!("prof-counter-warm-{iteration}"),
        args.messages as u64,
    )?;
    report.push(StageTiming::new(
        "warm_event_submit",
        warm_submit_started.elapsed(),
    ));

    let (_, stage) = wait_stage("warm_event_end_to_end", &mut supervisor, || {
        let state = runtimes.worker.state_json(
            world.universe_id,
            world.world_id,
            "demo/CounterSM@1",
            None,
        )?;
        Ok(state.filter(|state| {
            state["pc"] == json!({ "$tag": "Counting" })
                && state["remaining"] == json!(args.messages)
        }))
    })
    .await?;
    report.push(stage);

    let message_count = args.messages;
    let sender_runtime = control_runtime.clone();
    let sender_world = send_world.clone();
    let mut sender_handle = Some(tokio::task::spawn_blocking(move || {
        let send_started = Instant::now();
        let mut submit_times = Vec::with_capacity(message_count);
        for index in 0..message_count {
            submit_times.push(Instant::now());
            submit_counter_tick(
                &sender_runtime,
                &sender_world,
                format!("prof-counter-tick-{iteration}-{index}"),
            )?;
        }
        Result::<_, anyhow::Error>::Ok((send_started, send_started.elapsed(), submit_times))
    }));

    let mut completion_times = vec![None; args.messages];
    let mut observed = 0usize;
    let mut observe_stage = StageTiming {
        name: "stream_complete_wait".into(),
        ..StageTiming::default()
    };
    let observe_started = Instant::now();
    let deadline = observe_started + WAIT_DEADLINE;
    let mut sender_result = None;
    while observed < args.messages && Instant::now() < deadline {
        if sender_result.is_none()
            && let Some(handle) = sender_handle.as_ref()
            && handle.is_finished()
        {
            sender_result = Some(match sender_handle.take().expect("sender handle").await {
                Ok(Ok(result)) => result,
                Ok(Err(err)) => return Err(err),
                Err(err) => return Err(err.into()),
            });
        }
        let profile = supervisor.observe_interval(WAIT_SLEEP).await?;
        observe_stage.cycles += 1;
        observe_stage.run.add(profile);
        let probe_started = Instant::now();
        let state = runtimes.worker.state_json(
            world.universe_id,
            world.world_id,
            "demo/CounterSM@1",
            None,
        )?;
        observe_stage.probe_time += probe_started.elapsed();
        let processed = state
            .as_ref()
            .map(|state| counter_ticks_processed(state, args.messages as u64))
            .unwrap_or(0);
        let now = Instant::now();
        while observed < processed.min(args.messages as u64) as usize {
            completion_times[observed] = Some(now);
            observed += 1;
        }
        if observed >= args.messages {
            break;
        }
        observe_stage.sleep_time += WAIT_SLEEP;
    }
    if observed < args.messages {
        bail!(
            "timed out waiting for counter completion: observed {observed} of {}",
            args.messages
        );
    }
    observe_stage.elapsed = observe_started.elapsed();
    report.push(observe_stage);

    let (stream_started, submit_elapsed, submit_times) = match sender_result {
        Some(result) => result,
        None => sender_handle
            .take()
            .expect("sender handle")
            .await
            .map_err(anyhow::Error::from)??,
    };
    report.push(StageTiming::new("stream_submit", submit_elapsed));

    let latencies = submit_times
        .iter()
        .zip(completion_times.into_iter())
        .map(|(submitted_at, completed_at)| {
            completed_at
                .ok_or_else(|| anyhow!("missing completion timestamp"))
                .map(|completed_at| completed_at.duration_since(*submitted_at))
        })
        .collect::<Result<Vec<_>>>()?;
    let total = stream_started.elapsed();
    let stats = latency_stats(&latencies);
    let throughput = if total.is_zero() {
        0.0
    } else {
        args.messages as f64 / total.as_secs_f64()
    };
    let mut stage = StageTiming::new("steady_state_stream", total);
    stage.message_count = Some(args.messages);
    stage.throughput_msgs_per_sec = Some(throughput);
    stage.note = Some(format!(
        "latency_ms(min={} avg={} p50={} p95={} p99={} max={})",
        stats.min.as_millis(),
        stats.avg.as_millis(),
        stats.p50.as_millis(),
        stats.p95.as_millis(),
        stats.p99.as_millis(),
        stats.max.as_millis(),
    ));
    report.push(stage);

    supervisor.shutdown().await?;
    Ok(())
}

async fn create_runtimes(
    kind: RuntimeKind,
    partition_count: u32,
    iteration: usize,
) -> Result<RuntimePair> {
    match kind {
        RuntimeKind::Embedded => {
            let runtime = HostedWorkerRuntime::new_embedded_with_state_root(
                partition_count.max(1),
                temp_state_root(&format!("prof-embedded-{iteration}")),
            )?;
            Ok(RuntimePair {
                control: runtime.clone(),
                worker: runtime,
                ctx: None,
            })
        }
        RuntimeKind::Broker => {
            let ctx = broker_runtime_context("prof-broker", partition_count.max(1)).await?;
            Ok(RuntimePair {
                control: ctx.control_runtime(&format!("control-{iteration}"))?,
                worker: ctx.worker_runtime(&format!("worker-{iteration}"))?,
                ctx: Some(ctx),
            })
        }
        RuntimeKind::Direct => {
            let ctx = broker_runtime_context("prof-direct", partition_count.max(1)).await?;
            let worker = ctx.direct_worker_runtime(&format!("worker-{iteration}"), &[0])?;
            Ok(RuntimePair {
                control: worker.clone(),
                worker,
                ctx: Some(ctx),
            })
        }
    }
}

fn worker_config(args: &Args) -> HostedWorkerConfig {
    HostedWorkerConfig {
        worker_id: format!("prof-worker-{}", uuid::Uuid::new_v4()),
        partition_count: args.partition_count.max(1),
        checkpoint_interval: Duration::from_secs(3600),
        checkpoint_every_events: (args.checkpoint_every_events > 0)
            .then_some(args.checkpoint_every_events),
        max_local_continuation_slices_per_flush: args.max_local_continuation_slices_per_flush
            as usize,
        projection_commit_mode: args.projection_commit_mode.into(),
        max_uncommitted_slices_per_world: args.max_uncommitted_slices_per_world.max(1) as usize,
    }
}

fn profiling_control_runtime<'a>(
    args: &Args,
    runtimes: &'a RuntimePair,
) -> &'a HostedWorkerRuntime {
    if args.unsafe_no_flush {
        &runtimes.worker
    } else {
        &runtimes.control
    }
}

fn spawn_profiled_worker(
    worker: &HostedWorker,
    runtime: HostedWorkerRuntime,
    report: &mut IterationReport,
) -> Result<WorkerSupervisorHandle> {
    let started = Instant::now();
    let supervisor = worker.with_worker_runtime(runtime).spawn_profiled()?;
    report.push(StageTiming::new("worker_spawn", started.elapsed()));
    Ok(supervisor)
}

async fn wait_stage<T>(
    name: &str,
    supervisor: &mut WorkerSupervisorHandle,
    mut probe: impl FnMut() -> Result<Option<T>>,
) -> Result<(T, StageTiming)> {
    let started = Instant::now();
    let deadline = started + WAIT_DEADLINE;
    let mut stage = StageTiming {
        name: name.to_owned(),
        ..StageTiming::default()
    };
    while Instant::now() < deadline {
        let profile = supervisor.observe_interval(WAIT_SLEEP).await?;
        stage.cycles += 1;
        stage.run.add(profile);

        let probe_started = Instant::now();
        if let Some(value) = probe()? {
            stage.probe_time += probe_started.elapsed();
            stage.elapsed = started.elapsed();
            return Ok((value, stage));
        }
        stage.probe_time += probe_started.elapsed();
        stage.sleep_time += WAIT_SLEEP;
    }
    bail!("timed out waiting for stage '{name}'")
}

fn temp_state_root(label: &str) -> PathBuf {
    ensure_shared_module_cache_env();
    std::env::temp_dir().join(format!("aos-node-hosted-{label}-{}", uuid::Uuid::new_v4()))
}

fn counter_world_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        authored_smoke_world_root(
            "00-counter",
            "aos-node-hosted-counter-prof",
            "air",
            "demo/CounterSM@1",
        )
    })
    .clone()
}

fn fetch_notify_world_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        authored_smoke_world_root(
            "03-fetch-notify",
            "aos-node-hosted-fetch-notify-prof",
            "air",
            "demo/FetchNotify@1",
        )
    })
    .clone()
}

fn smoke_fixture_root(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../aos-smoke/fixtures")
        .join(name)
        .canonicalize()
        .expect("smoke fixture path")
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn workspace_target_dir() -> PathBuf {
    repo_root().join("target")
}

fn authored_smoke_world_root(
    fixture_name: &str,
    temp_prefix: &str,
    air_dir: &str,
    workflow_module: &str,
) -> PathBuf {
    let src = smoke_fixture_root(fixture_name);
    let signature = fixture_copy_signature(&src, air_dir, workflow_module);
    let dst = std::env::temp_dir().join(format!("{temp_prefix}-{signature}"));
    if dst.exists() {
        return dst;
    }
    copy_fixture_dir(&src, &dst);

    let aos_state = dst.join(".aos");
    if aos_state.exists() {
        let _ = fs::remove_dir_all(&aos_state);
    }

    let wasm_sdk = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../aos-wasm-sdk")
        .canonicalize()
        .expect("aos-wasm-sdk path");
    let wasm_abi = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../aos-wasm-abi")
        .canonicalize()
        .expect("aos-wasm-abi path");
    let cargo_toml = dst.join("workflow/Cargo.toml");
    let cargo_text = fs::read_to_string(&cargo_toml).expect("read copied workflow Cargo.toml");
    let cargo_text = cargo_text
        .replace("../../../../aos-wasm-sdk", &wasm_sdk.display().to_string())
        .replace("../../../../aos-wasm-abi", &wasm_abi.display().to_string());
    fs::write(&cargo_toml, cargo_text).expect("patch copied workflow Cargo.toml");
    fs::write(
        dst.join("aos.sync.json"),
        serde_json::to_vec_pretty(&json!({
            "air": { "dir": air_dir },
            "build": {
                "workflow_dir": "workflow",
                "module": workflow_module,
            },
            "modules": {
                "pull": false,
            },
            "version": 1,
            "workspaces": [
                {
                    "dir": "workflow",
                    "ignore": ["target/", ".git/", ".aos/"],
                    "ref": "workflow",
                }
            ],
        }))
        .expect("encode sync config"),
    )
    .expect("write sync config");
    dst
}

fn fixture_copy_signature(src: &Path, air_dir: &str, workflow_module: &str) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(src.to_string_lossy().as_bytes());
    bytes.extend_from_slice(b"\n");
    bytes.extend_from_slice(air_dir.as_bytes());
    bytes.extend_from_slice(b"\n");
    bytes.extend_from_slice(workflow_module.as_bytes());
    bytes.extend_from_slice(b"\n");
    let mut entries = Vec::new();
    collect_fixture_files(src, src, &mut entries);
    entries.sort();
    for entry in entries {
        let rel = entry.strip_prefix(src).expect("fixture relative path");
        bytes.extend_from_slice(rel.to_string_lossy().as_bytes());
        bytes.extend_from_slice(b"\n");
        bytes.extend_from_slice(&fs::read(&entry).expect("read fixture file"));
        bytes.extend_from_slice(b"\n");
    }
    Hash::of_bytes(&bytes).to_hex()
}

fn collect_fixture_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read fixture dir") {
        let entry = entry.expect("fixture dir entry");
        let path = entry.path();
        let name = entry.file_name();
        let file_type = entry.file_type().expect("fixture file type");
        if file_type.is_dir() {
            if should_skip_fixture_path(root, &path, &name) {
                continue;
            }
            collect_fixture_files(root, &path, out);
        } else if file_type.is_file() {
            out.push(path);
        }
    }
}

fn copy_fixture_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create temp fixture root");
    for entry in fs::read_dir(src).expect("read source fixture dir") {
        let entry = entry.expect("fixture dir entry");
        let path = entry.path();
        let name = entry.file_name();
        let file_type = entry.file_type().expect("fixture file type");
        if file_type.is_dir() {
            if should_skip_fixture_path(src, &path, &name) {
                continue;
            }
            copy_fixture_dir(&path, &dst.join(entry.file_name()));
        } else if file_type.is_file() {
            fs::copy(&path, dst.join(entry.file_name())).expect("copy fixture file");
        }
    }
}

fn should_skip_fixture_path(root: &Path, path: &Path, name: &std::ffi::OsStr) -> bool {
    if matches!(name.to_str(), Some(".git" | ".aos" | "target")) {
        return true;
    }
    path.parent() == Some(root) && matches!(name.to_str(), Some(".git" | ".aos" | "target"))
}

fn prepare_authored_manifest(world_root: &Path) -> Result<PreparedAuthoredManifest> {
    let (store, bundle, _) = build_bundle_from_local_world_with_profile(
        world_root,
        false,
        WorkflowBuildProfile::Release,
    )?;
    let imported = import_genesis(&store, &bundle)?;
    let loaded = ManifestLoader::load_from_bytes(&store, &imported.manifest_bytes)?;
    let manifest: Manifest = serde_cbor::from_slice(&imported.manifest_bytes)?;

    let mut seen = BTreeSet::new();
    let mut blobs = Vec::new();
    let mut push_blob = |bytes: Vec<u8>| {
        let hash = Hash::of_bytes(&bytes);
        if seen.insert(hash) {
            blobs.push(bytes);
        }
    };

    for secret in &bundle.secrets {
        push_blob(to_canonical_cbor(&AirNode::Defsecret(secret.clone()))?);
    }
    for named in &manifest.schemas {
        if let Some(schema) = loaded.schemas.get(named.name.as_str()).cloned() {
            push_blob(to_canonical_cbor(&AirNode::Defschema(schema))?);
        } else if let Some(builtin) = builtins::find_builtin_schema(named.name.as_str()) {
            push_blob(to_canonical_cbor(&AirNode::Defschema(
                builtin.schema.clone(),
            ))?);
        } else {
            bail!("missing schema ref {}", named.name);
        }
    }
    for named in &manifest.effects {
        if let Some(effect) = loaded.effects.get(named.name.as_str()).cloned() {
            push_blob(to_canonical_cbor(&AirNode::Defeffect(effect))?);
        } else if let Some(builtin) = builtins::find_builtin_effect(named.name.as_str()) {
            push_blob(to_canonical_cbor(&AirNode::Defeffect(
                builtin.effect.clone(),
            ))?);
        } else {
            bail!("missing effect ref {}", named.name);
        }
    }
    for named in &manifest.caps {
        if let Some(cap) = loaded.caps.get(named.name.as_str()).cloned() {
            push_blob(to_canonical_cbor(&AirNode::Defcap(cap))?);
        } else if let Some(builtin) = builtins::find_builtin_cap(named.name.as_str()) {
            push_blob(to_canonical_cbor(&AirNode::Defcap(builtin.cap.clone()))?);
        } else {
            bail!("missing capability ref {}", named.name);
        }
    }
    for named in &manifest.policies {
        let policy = loaded
            .policies
            .get(named.name.as_str())
            .cloned()
            .ok_or_else(|| anyhow!("missing policy ref {}", named.name))?;
        push_blob(to_canonical_cbor(&AirNode::Defpolicy(policy))?);
    }
    for named in &manifest.modules {
        let module = if let Some(module) = loaded.modules.get(named.name.as_str()).cloned() {
            module
        } else if let Some(builtin) = builtins::find_builtin_module(named.name.as_str()) {
            builtin.module.clone()
        } else {
            bail!("missing module ref {}", named.name);
        };
        push_blob(to_canonical_cbor(&AirNode::Defmodule(module.clone()))?);
        push_module_wasm_blob(&store, &module, &mut push_blob)?;
    }

    let manifest_value: serde_cbor::Value = serde_cbor::from_slice(&imported.manifest_bytes)?;
    let mut referenced_hashes = BTreeSet::new();
    collect_hash_refs_from_cbor(&manifest_value, &mut referenced_hashes);
    for hash in referenced_hashes {
        if let Ok(bytes) = store.get_blob(hash) {
            push_blob(bytes);
            continue;
        }
        if let Ok(node) = store.get_node::<serde_cbor::Value>(hash) {
            push_blob(serde_cbor::to_vec(&node)?);
        }
    }

    Ok(PreparedAuthoredManifest {
        blobs,
        manifest_bytes: imported.manifest_bytes,
    })
}

fn push_module_wasm_blob(
    store: &impl Store,
    module: &DefModule,
    push_blob: &mut impl FnMut(Vec<u8>),
) -> Result<()> {
    let wasm_hash = Hash::from_hex_str(module.wasm_hash.as_str())
        .with_context(|| format!("parse wasm hash for module {}", module.name))?;
    if let Ok(bytes) = store.get_blob(wasm_hash) {
        push_blob(bytes);
        return Ok(());
    }
    if let Some(bytes) = builtin_module_wasm_bytes(module.name.as_str())? {
        let actual_hash = Hash::of_bytes(&bytes);
        if actual_hash != wasm_hash {
            bail!(
                "builtin module {} produced unexpected wasm hash: expected {}, got {}",
                module.name,
                wasm_hash.to_hex(),
                actual_hash.to_hex(),
            );
        }
        push_blob(bytes);
        return Ok(());
    }
    bail!("missing wasm blob for module {}", module.name);
}

fn builtin_module_wasm_bytes(name: &str) -> Result<Option<Vec<u8>>> {
    let bin = match name {
        "sys/CapEnforceHttpOut@1" => "cap_enforce_http_out",
        "sys/CapEnforceWorkspace@1" => "cap_enforce_workspace",
        "sys/Workspace@1" => "workspace",
        _ => return Ok(None),
    };
    let wasm_path = workspace_target_dir()
        .join("wasm32-unknown-unknown/debug")
        .join(format!("{bin}.wasm"));
    if !wasm_path.exists() {
        let status = Command::new("cargo")
            .current_dir(repo_root())
            .args([
                "build",
                "-p",
                "aos-sys",
                "--target",
                "wasm32-unknown-unknown",
                "--bin",
                bin,
            ])
            .status()
            .with_context(|| format!("build builtin module {bin}"))?;
        if !status.success() {
            bail!("building builtin module {bin} failed with {status}");
        }
    }
    Ok(Some(fs::read(&wasm_path).with_context(|| {
        format!("read builtin module wasm {}", wasm_path.display())
    })?))
}

fn collect_hash_refs_from_cbor(value: &serde_cbor::Value, out: &mut BTreeSet<Hash>) {
    match value {
        serde_cbor::Value::Text(text) => {
            if let Ok(hash) = Hash::from_hex_str(text) {
                out.insert(hash);
            }
        }
        serde_cbor::Value::Array(values) => {
            for value in values {
                collect_hash_refs_from_cbor(value, out);
            }
        }
        serde_cbor::Value::Map(entries) => {
            for (key, value) in entries {
                collect_hash_refs_from_cbor(key, out);
                collect_hash_refs_from_cbor(value, out);
            }
        }
        serde_cbor::Value::Tag(_, value) => collect_hash_refs_from_cbor(value, out),
        _ => {}
    }
}

fn upload_counter_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> Result<String> {
    static PREPARED: OnceLock<PreparedAuthoredManifest> = OnceLock::new();
    if PREPARED.get().is_none() {
        let prepared = prepare_authored_manifest(&counter_world_root())?;
        let _ = PREPARED.set(prepared);
    }
    let prepared = PREPARED.get().expect("counter manifest prepared");
    upload_prepared_manifest(runtime, universe_id, prepared)
}

fn upload_fetch_notify_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> Result<String> {
    static PREPARED: OnceLock<PreparedAuthoredManifest> = OnceLock::new();
    if PREPARED.get().is_none() {
        let prepared = prepare_authored_manifest(&fetch_notify_world_root())?;
        let _ = PREPARED.set(prepared);
    }
    let prepared = PREPARED.get().expect("fetch-notify manifest prepared");
    upload_prepared_manifest(runtime, universe_id, prepared)
}

fn upload_prepared_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
    prepared: &PreparedAuthoredManifest,
) -> Result<String> {
    for bytes in &prepared.blobs {
        runtime.put_blob(universe_id, bytes)?;
    }
    Ok(runtime
        .put_blob(universe_id, &prepared.manifest_bytes)?
        .to_hex())
}

async fn broker_runtime_context(label: &str, partition_count: u32) -> Result<BrokerRuntimeContext> {
    ensure_profile_env_loaded();
    let kafka_config = broker_kafka_config(label, partition_count)?
        .ok_or_else(|| anyhow!("Kafka not configured"))?;
    let mut blobstore_config =
        broker_blobstore_config(label)?.ok_or_else(|| anyhow!("blobstore not configured"))?;
    blobstore_config.pack_threshold_bytes = 0;
    ensure_kafka_topics(&kafka_config, partition_count).await?;
    Ok(BrokerRuntimeContext {
        kafka_config,
        blobstore_config,
        partition_count,
    })
}

fn broker_kafka_config(label: &str, _partition_count: u32) -> Result<Option<KafkaConfig>> {
    let mut config = KafkaConfig::default();
    let Some(bootstrap) = config.bootstrap_servers.clone() else {
        return Ok(None);
    };
    if bootstrap.trim().is_empty() {
        return Ok(None);
    }
    let unique = format!("{label}-{}", uuid::Uuid::new_v4());
    config.ingress_topic = format!("aos-ingress-{unique}");
    config.journal_topic = format!("aos-journal-{unique}");
    config.projection_topic = format!("aos-projection-{unique}");
    config.submission_group_prefix = format!("{}-{unique}", config.submission_group_prefix);
    config.transactional_id = format!("{}-{unique}", config.transactional_id);
    config.producer_message_timeout_ms = 1_000;
    config.producer_flush_timeout_ms = 1_000;
    config.transaction_timeout_ms = 2_000;
    config.metadata_timeout_ms = 500;
    config.group_session_timeout_ms = 6_000;
    config.group_heartbeat_interval_ms = 500;
    config.group_poll_wait_ms = 1;
    config.recovery_fetch_wait_ms = 10;
    config.recovery_poll_interval_ms = 10;
    config.recovery_idle_timeout_ms = 20;
    Ok(Some(config))
}

fn broker_blobstore_config(label: &str) -> Result<Option<BlobStoreConfig>> {
    let mut config = BlobStoreConfig::default();
    let Some(bucket) = config.bucket.clone() else {
        return Ok(None);
    };
    if bucket.trim().is_empty() {
        return Ok(None);
    }
    let suffix = format!("{label}-{}", uuid::Uuid::new_v4());
    config.prefix = match config.prefix.trim_matches('/') {
        "" => suffix,
        prefix => format!("{prefix}/{suffix}"),
    };
    Ok(Some(config))
}

async fn ensure_kafka_topics(config: &KafkaConfig, partition_count: u32) -> Result<()> {
    let bootstrap_servers = config
        .bootstrap_servers
        .as_ref()
        .ok_or_else(|| anyhow!("Kafka bootstrap servers are not configured for profiling"))?;
    let admin: AdminClient<_> = ClientConfig::new()
        .set("bootstrap.servers", bootstrap_servers)
        .create()
        .context("create Kafka admin client")?;

    let topics = [
        NewTopic::new(
            &config.ingress_topic,
            partition_count as i32,
            TopicReplication::Fixed(1),
        ),
        NewTopic::new(
            &config.journal_topic,
            partition_count as i32,
            TopicReplication::Fixed(1),
        ),
        NewTopic::new(
            &config.projection_topic,
            partition_count as i32,
            TopicReplication::Fixed(1),
        ),
    ];
    let results = admin
        .create_topics(&topics, &AdminOptions::new())
        .await
        .context("create Kafka topics")?;
    for result in results {
        if let Err((topic, code)) = result
            && code != RDKafkaErrorCode::TopicAlreadyExists
        {
            bail!("create Kafka topic {topic}: {code}");
        }
    }
    Ok(())
}

fn worker_kafka_config_for_label(ctx: &BrokerRuntimeContext, label: &str) -> KafkaConfig {
    let mut kafka_config = ctx.kafka_config.clone();
    kafka_config.submission_group_prefix =
        format!("{}-worker-{label}", kafka_config.submission_group_prefix);
    kafka_config.transactional_id = format!("{}-worker-{label}", kafka_config.transactional_id);
    kafka_config
}

fn control_kafka_config_for_label(ctx: &BrokerRuntimeContext, label: &str) -> KafkaConfig {
    let mut kafka_config = ctx.kafka_config.clone();
    kafka_config.submission_group_prefix =
        format!("{}-control-{label}", kafka_config.submission_group_prefix);
    kafka_config.transactional_id = format!("{}-control-{label}", kafka_config.transactional_id);
    kafka_config
}

fn ensure_profile_env_loaded() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = load_dotenv_candidates();
        let test_ns = format!("aos-node-hosted-prof-{}", uuid::Uuid::new_v4());
        unsafe {
            std::env::set_var("AOS_KAFKA_GROUP_PREFIX", &test_ns);
            std::env::set_var("AOS_KAFKA_TRANSACTIONAL_ID", &test_ns);
            std::env::set_var("AOS_BLOBSTORE_PREFIX", &test_ns);
        }
    });
    ensure_shared_module_cache_env();
}

fn ensure_shared_module_cache_env() {
    static MODULE_CACHE_INIT: OnceLock<PathBuf> = OnceLock::new();
    let cache_dir = MODULE_CACHE_INIT.get_or_init(|| {
        let dir = std::env::temp_dir().join("aos-node-hosted-prof-module-cache");
        fs::create_dir_all(&dir).expect("create hosted prof module cache dir");
        dir
    });
    if std::env::var_os("AOS_MODULE_CACHE_DIR").is_none() {
        unsafe {
            std::env::set_var("AOS_MODULE_CACHE_DIR", cache_dir);
        }
    }
}

async fn wait_for_effect_intent_hash(
    ctx: &BrokerRuntimeContext,
    world: &HostedWorldSummary,
) -> Result<[u8; 32]> {
    let mut kafka_config = ctx.kafka_config.clone();
    kafka_config.submission_group_prefix =
        format!("{}-reader", kafka_config.submission_group_prefix);
    kafka_config.transactional_id = format!("{}-reader", kafka_config.transactional_id);
    let mut reader = HostedKafkaBackend::new(ctx.partition_count, kafka_config)?;
    let deadline = Instant::now() + WAIT_DEADLINE;
    while Instant::now() < deadline {
        reader.recover_partition_from_broker(world.effective_partition)?;
        for frame in reader.world_frames(world.world_id) {
            for record in &frame.records {
                if let JournalRecord::EffectIntent(intent) = record {
                    return Ok(intent.intent_hash);
                }
            }
        }
        tokio::time::sleep(WAIT_SLEEP).await;
    }
    bail!(
        "timed out waiting for effect intent for world {} in universe {}",
        world.world_id,
        world.universe_id
    )
}

fn submit_counter_start(
    runtime: &HostedWorkerRuntime,
    world: &HostedWorldSummary,
    submission_id: String,
    target: u64,
) -> Result<()> {
    runtime.submit_event(SubmitEventRequest {
        universe_id: world.universe_id,
        world_id: world.world_id,
        schema: "demo/CounterEvent@1".into(),
        value: json!({
            "Start": {
                "target": target,
            }
        }),
        submission_id: Some(submission_id),
        expected_world_epoch: Some(world.world_epoch),
    })?;
    Ok(())
}

fn submit_counter_tick(
    runtime: &HostedWorkerRuntime,
    world: &HostedWorldSummary,
    submission_id: String,
) -> Result<()> {
    runtime.submit_event(SubmitEventRequest {
        universe_id: world.universe_id,
        world_id: world.world_id,
        schema: "demo/CounterEvent@1".into(),
        value: json!({ "Tick": null }),
        submission_id: Some(submission_id),
        expected_world_epoch: Some(world.world_epoch),
    })?;
    Ok(())
}

fn counter_ticks_processed(state: &serde_json::Value, target: u64) -> u64 {
    let remaining = state["remaining"].as_u64().unwrap_or(target);
    match state["pc"].get("$tag").and_then(|tag| tag.as_str()) {
        Some("Counting") => target.saturating_sub(remaining),
        Some("Done") if remaining == 0 => target,
        _ => 0,
    }
}

fn submit_fetch_notify_start(
    runtime: &HostedWorkerRuntime,
    world: &HostedWorldSummary,
    submission_id: String,
) -> Result<()> {
    runtime.submit_event(SubmitEventRequest {
        universe_id: world.universe_id,
        world_id: world.world_id,
        schema: "demo/FetchNotifyEvent@1".into(),
        value: json!({
            "Start": {
                "url": "https://example.com/data.json",
                "method": "GET",
            }
        }),
        submission_id: Some(submission_id),
        expected_world_epoch: Some(world.world_epoch),
    })?;
    Ok(())
}

fn fetch_notify_done(state: &serde_json::Value, next_request_id: u64) -> bool {
    state["pc"] == json!({ "$tag": "Done" })
        && state["pending_request"].is_null()
        && state["last_status"] == json!(200)
        && state["next_request_id"] == json!(next_request_id)
}

async fn wait_until_fetch_notify_done_profiled(
    runtime: &HostedWorkerRuntime,
    world: &HostedWorldSummary,
    next_request_id: u64,
    supervisor: &mut WorkerSupervisorHandle,
    stage: &mut StageTiming,
) -> Result<()> {
    let deadline = Instant::now() + WAIT_DEADLINE;
    while Instant::now() < deadline {
        let profile = supervisor.observe_interval(WAIT_SLEEP).await?;
        stage.cycles += 1;
        stage.run.add(profile);
        let probe_started = Instant::now();
        let state = runtime.state_json(
            world.universe_id,
            world.world_id,
            "demo/FetchNotify@1",
            None,
        )?;
        stage.probe_time += probe_started.elapsed();
        if state
            .as_ref()
            .is_some_and(|state| fetch_notify_done(state, next_request_id))
        {
            return Ok(());
        }
        stage.sleep_time += WAIT_SLEEP;
    }
    bail!(
        "timed out waiting for fetch-notify completion at request {} for world {}",
        next_request_id,
        world.world_id
    )
}

fn spawn_stub_http_adapter_pump(
    ctx: BrokerRuntimeContext,
    control: HostedWorkerRuntime,
    world: HostedWorldSummary,
) -> JoinHandle<Result<()>> {
    tokio::spawn(async move {
        let mut kafka_config = ctx.kafka_config.clone();
        kafka_config.submission_group_prefix = format!(
            "{}-adapter-{}",
            kafka_config.submission_group_prefix,
            uuid::Uuid::new_v4()
        );
        kafka_config.transactional_id = format!(
            "{}-adapter-{}",
            kafka_config.transactional_id,
            uuid::Uuid::new_v4()
        );
        let mut reader = HostedKafkaBackend::new(ctx.partition_count, kafka_config)?;
        let mut seen = HashSet::new();
        let adapter = StubHttpAdapter;

        loop {
            reader.recover_partition_from_broker(world.effective_partition)?;
            let mut dispatched = 0usize;
            for frame in reader.world_frames(world.world_id) {
                for record in &frame.records {
                    let JournalRecord::EffectIntent(intent_record) = record else {
                        continue;
                    };
                    if !seen.insert(intent_record.intent_hash) {
                        continue;
                    }
                    let intent = EffectIntent::from_raw_params(
                        EffectKind::new(intent_record.kind.clone()),
                        intent_record.cap_name.clone(),
                        intent_record.params_cbor.clone(),
                        intent_record.idempotency_key,
                    )?;
                    let receipt = adapter.run_terminal(&intent).await?;
                    control.submit_receipt(
                        world.universe_id,
                        world.world_id,
                        ReceiptIngress {
                            intent_hash: receipt.intent_hash.to_vec(),
                            effect_kind: intent.kind.as_str().to_owned(),
                            adapter_id: receipt.adapter_id,
                            status: receipt.status,
                            payload: CborPayload::inline(receipt.payload_cbor),
                            cost_cents: receipt.cost_cents,
                            signature: receipt.signature,
                            correlation_id: Some(format!(
                                "adapter-pump-{}",
                                Hash::of_bytes(&intent.intent_hash).to_hex()
                            )),
                        },
                    )?;
                    dispatched += 1;
                }
            }
            if dispatched == 0 {
                tokio::time::sleep(WAIT_SLEEP).await;
            }
        }
    })
}

fn latency_stats(latencies: &[Duration]) -> LatencyStats {
    if latencies.is_empty() {
        return LatencyStats {
            min: Duration::ZERO,
            avg: Duration::ZERO,
            p50: Duration::ZERO,
            p95: Duration::ZERO,
            p99: Duration::ZERO,
            max: Duration::ZERO,
        };
    }
    let mut sorted = latencies.to_vec();
    sorted.sort_unstable();
    LatencyStats {
        min: *sorted.first().expect("non-empty latencies"),
        avg: average_duration(&sorted),
        p50: percentile_duration(&sorted, 0.50),
        p95: percentile_duration(&sorted, 0.95),
        p99: percentile_duration(&sorted, 0.99),
        max: *sorted.last().expect("non-empty latencies"),
    }
}

fn percentile_duration(sorted: &[Duration], percentile: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = ((sorted.len() - 1) as f64 * percentile).ceil() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn print_iteration(iteration: usize, report: &IterationReport) {
    println!();
    println!("iteration {}", iteration + 1);
    for stage in &report.stages {
        let mut extras = String::new();
        if stage.cycles > 0 {
            let _ = write!(
                extras,
                " cycles={} run={}ms probe={}ms sleep={}ms",
                stage.cycles,
                stage.run.total.as_millis(),
                stage.probe_time.as_millis(),
                stage.sleep_time.as_millis()
            );
            let _ = write!(
                extras,
                " parts(sync={} active={} run={} ckpt={})",
                stage.run.sync_assignments.as_millis(),
                stage.run.sync_active_worlds.as_millis(),
                stage.run.run_partitions.as_millis(),
                stage.run.publish_checkpoints.as_millis()
            );
            let _ = write!(
                extras,
                " partition(drain={} create={} existing={} activate={} apply={} build={} host_drain={} post={} commit={} cmd={} promote={} inline_ckpt={})",
                stage.run.partition_drain_submissions.as_millis(),
                stage.run.partition_process_create.as_millis(),
                stage.run.partition_process_existing.as_millis(),
                stage.run.partition_activate_world.as_millis(),
                stage.run.partition_apply_submission.as_millis(),
                stage.run.partition_build_external_event.as_millis(),
                stage.run.partition_host_drain.as_millis(),
                stage.run.partition_post_apply.as_millis(),
                stage.run.partition_commit_batch.as_millis(),
                stage.run.partition_commit_command_records.as_millis(),
                stage.run.partition_promote_worlds.as_millis(),
                stage.run.partition_inline_checkpoint.as_millis(),
            );
        }
        if let Some(note) = &stage.note {
            let _ = write!(extras, " {note}");
        }
        if let Some(message_count) = stage.message_count {
            let _ = write!(extras, " messages={message_count}");
        }
        if let Some(throughput) = stage.throughput_msgs_per_sec {
            let _ = write!(extras, " throughput={throughput:.2} msg/s");
        }
        println!(
            "  {:32} {:>6} ms{}",
            stage.name,
            stage.elapsed.as_millis(),
            extras
        );
    }
    println!("  {:32} {:>6} ms", "total", report.total.as_millis());
}

fn print_summary(reports: &[IterationReport]) {
    if reports.is_empty() {
        return;
    }
    let mut by_stage = std::collections::BTreeMap::<&str, Vec<Duration>>::new();
    let mut throughput_by_stage = std::collections::BTreeMap::<&str, Vec<f64>>::new();
    let mut messages_by_stage = std::collections::BTreeMap::<&str, Vec<usize>>::new();
    let mut totals = Vec::with_capacity(reports.len());
    for report in reports {
        totals.push(report.total);
        for stage in &report.stages {
            by_stage
                .entry(stage.name.as_str())
                .or_default()
                .push(stage.elapsed);
            if let Some(throughput) = stage.throughput_msgs_per_sec {
                throughput_by_stage
                    .entry(stage.name.as_str())
                    .or_default()
                    .push(throughput);
            }
            if let Some(message_count) = stage.message_count {
                messages_by_stage
                    .entry(stage.name.as_str())
                    .or_default()
                    .push(message_count);
            }
        }
    }
    println!();
    println!("summary");
    for (name, values) in by_stage {
        let mut extras = String::new();
        if let Some(message_counts) = messages_by_stage.get(name) {
            let min = message_counts.iter().min().copied().unwrap_or_default();
            let avg = message_counts.iter().sum::<usize>() as f64 / message_counts.len() as f64;
            let max = message_counts.iter().max().copied().unwrap_or_default();
            let _ = write!(extras, " messages(min={} avg={avg:.1} max={})", min, max);
        }
        if let Some(throughputs) = throughput_by_stage.get(name) {
            let min = throughputs.iter().copied().fold(f64::INFINITY, f64::min);
            let max = throughputs
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max);
            let avg = throughputs.iter().copied().sum::<f64>() / throughputs.len() as f64;
            let _ = write!(
                extras,
                " throughput(min={min:.2} avg={avg:.2} max={max:.2} msg/s)"
            );
        }
        println!(
            "  {:32} min={:>6} ms avg={:>6} ms max={:>6} ms{}",
            name,
            values.iter().min().copied().unwrap_or_default().as_millis(),
            average_duration(&values).as_millis(),
            values.iter().max().copied().unwrap_or_default().as_millis(),
            extras,
        );
    }
    println!(
        "  {:32} min={:>6} ms avg={:>6} ms max={:>6} ms",
        "total",
        totals.iter().min().copied().unwrap_or_default().as_millis(),
        average_duration(&totals).as_millis(),
        totals.iter().max().copied().unwrap_or_default().as_millis(),
    );
}

fn average_duration(values: &[Duration]) -> Duration {
    if values.is_empty() {
        return Duration::ZERO;
    }
    let nanos = values.iter().map(Duration::as_nanos).sum::<u128>() / values.len() as u128;
    Duration::from_nanos(nanos as u64)
}
