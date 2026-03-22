use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Once, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use aos_air_types::{AirNode, EffectKind, Manifest, builtins};
use aos_authoring::bundle::import_genesis;
use aos_authoring::sync::load_available_secret_value_map;
use aos_authoring::{WorkflowBuildProfile, build_bundle_from_local_world_with_profile};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effect_adapters::adapters::registry::{AdapterRegistry, AdapterRegistryConfig};
use aos_effect_adapters::adapters::stub::StubHttpAdapter;
use aos_effect_types::{HashRef, HttpRequestReceipt, RequestTimings};
use aos_effects::EffectIntent;
use aos_effects::ReceiptStatus;
use aos_kernel::journal::{Journal, JournalRecord, OwnedJournalEntry};
use aos_kernel::{ManifestLoader, Store};
use aos_node::api::{
    CreateWorldBody, PutSecretVersionBody, SubmitEventBody, UpsertSecretBindingBody,
};
use aos_node::{
    BlobPlane, CborPayload, CreateWorldRequest, CreateWorldSource, ReceiptIngress,
    SecretBindingSourceKind, SecretBindingStatus, UniverseId, journal_entries_from_world_frames,
    open_plane_world_from_checkpoint, open_plane_world_from_frames, partition_for_world,
};
use aos_node_hosted::blobstore::{BlobStoreConfig, RemoteCasStore};
use aos_node_hosted::bootstrap::{build_control_deps_broker, build_worker_runtime_broker};
use aos_node_hosted::config::HostedWorkerConfig;
use aos_node_hosted::control::ControlFacade;
use aos_node_hosted::infra::vault::UpsertSecretBinding;
use aos_node_hosted::kafka::{HostedKafkaBackend, KafkaConfig};
use aos_node_hosted::worker::HostedWorkerRuntime;
use aos_node_hosted::{
    HostedWorker, HostedWorldSummary, SubmitEventRequest, SupervisorRunProfile, WorkerSupervisor,
    load_dotenv_candidates,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use clap::{Parser, ValueEnum};
use serde_json::json;
use tokio::task::JoinHandle;

const WAIT_DEADLINE: Duration = Duration::from_secs(120);
const WAIT_SLEEP: Duration = Duration::from_millis(1);
const DEMIURGE_DEFAULT_TASK: &str =
    "Read README.md and summarize the project name in one sentence.";
const DEMIURGE_DEFAULT_MODEL_OPENAI: &str = "gpt-5.3-codex";
const DEMIURGE_DEFAULT_MODEL_ANTHROPIC: &str = "claude-sonnet-4-5";

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

    #[arg(long, default_value = DEMIURGE_DEFAULT_TASK)]
    demiurge_task: String,

    #[arg(long, default_value_t = 2)]
    demiurge_task_count: usize,

    #[arg(long)]
    demiurge_provider: Option<String>,

    #[arg(long)]
    demiurge_model: Option<String>,

    #[arg(long)]
    demiurge_workdir: Option<PathBuf>,

    #[arg(long, default_value_t = 256)]
    demiurge_max_tokens: u64,

    #[arg(long, default_value = "openai")]
    demiurge_tool_profile: String,

    #[arg(long, default_value = "host.fs.read_file")]
    demiurge_allowed_tools: String,

    #[arg(long, default_value = "host.fs.read_file")]
    demiurge_tool_enable: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Scenario {
    CounterStart,
    FetchNotifyReceipt,
    FetchNotifyThroughput,
    CounterThroughput,
    DemiurgeTask,
    DemiurgeRestartRepro,
    DemiurgeRestartInproc,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum RuntimeKind {
    Embedded,
    Broker,
    Direct,
}

#[derive(Clone)]
struct PreparedAuthoredManifest {
    blobs: Vec<Vec<u8>>,
    manifest_bytes: Vec<u8>,
}

#[derive(Clone)]
struct DemiurgeRunConfig {
    provider: String,
    model: String,
    task: String,
    workdir: PathBuf,
    max_tokens: u64,
    tool_profile: String,
    allowed_tools: Option<Vec<String>>,
    tool_enable: Option<Vec<String>>,
    live_provider: bool,
    synced_bindings: Vec<String>,
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

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    load_dotenv_candidates()?;
    let args = Args::parse();

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

    if matches!(args.scenario, Scenario::DemiurgeRestartInproc) {
        return run_demiurge_restart_inproc_iteration(args, iteration, universe_id).await;
    }

    let runtime_started = Instant::now();
    let runtimes = create_runtimes(args.runtime, args.partition_count, iteration)?;
    report.push(StageTiming::new(
        "runtime_create",
        runtime_started.elapsed(),
    ));

    let worker = HostedWorker::new(worker_config(args));

    match args.scenario {
        Scenario::CounterStart => {
            let mut supervisor = worker.with_worker_runtime(runtimes.worker.clone());
            let manifest_started = Instant::now();
            let manifest_hash = upload_counter_manifest(&runtimes.control, universe_id)?;
            report.push(StageTiming::new(
                "manifest_upload.counter",
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

            let (world, stage) = wait_stage("world_register_wait", &mut supervisor, || {
                match runtimes.control.get_world(universe_id, accepted.world_id) {
                    Ok(world) => Ok(Some(world)),
                    Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
                    Err(err) => Err(err.into()),
                }
            })
            .await?;
            report.push(stage);

            let submit_started = Instant::now();
            runtimes.worker.submit_event(SubmitEventRequest {
                universe_id: world.universe_id,
                world_id: world.world_id,
                schema: "demo/CounterEvent@1".into(),
                value: json!({ "Start": { "target": 1 } }),
                submission_id: Some(format!("prof-counter-{iteration}")),
                expected_world_epoch: Some(world.world_epoch),
            })?;
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
        }
        Scenario::FetchNotifyReceipt => {
            let mut supervisor = worker.with_worker_runtime(runtimes.worker.clone());
            let manifest_started = Instant::now();
            let manifest_hash = upload_fetch_notify_manifest(&runtimes.worker, universe_id)?;
            if let Some(ctx) = &runtimes.ctx {
                seed_http_builtins(ctx, universe_id)?;
            }
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

            let (world, stage) = wait_stage("world_register_wait", &mut supervisor, || {
                match runtimes.control.get_world(universe_id, accepted.world_id) {
                    Ok(world) => Ok(Some(world)),
                    Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
                    Err(err) => Err(err.into()),
                }
            })
            .await?;
            report.push(stage);

            let submit_started = Instant::now();
            runtimes.control.submit_event(SubmitEventRequest {
                universe_id: world.universe_id,
                world_id: world.world_id,
                schema: "demo/FetchNotifyEvent@1".into(),
                value: json!({
                    "Start": {
                        "url": "https://example.com/data.json",
                        "method": "GET",
                    }
                }),
                submission_id: Some(format!("prof-fetch-start-{iteration}")),
                expected_world_epoch: Some(world.world_epoch),
            })?;
            report.push(StageTiming::new("event_submit", submit_started.elapsed()));

            let (_trace, stage) =
                wait_stage("event_to_pending_receipt_wait", &mut supervisor, || {
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
            let intent_hash = wait_for_effect_intent_hash(
                runtimes
                    .ctx
                    .as_ref()
                    .ok_or_else(|| anyhow!("fetch-notify profiling requires broker runtime"))?,
                &world,
            )
            .await?;
            report.push(StageTiming::new(
                "intent_hash_discovery",
                intent_started.elapsed(),
            ));

            let receipt_started = Instant::now();
            let receipt_payload = HttpRequestReceipt {
                status: 200,
                headers: Default::default(),
                body_ref: Some(
                    HashRef::new(
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
        }
        Scenario::FetchNotifyThroughput => {
            if args.runtime != RuntimeKind::Broker {
                bail!("fetch-notify throughput scenario requires --runtime broker");
            }
            if args.messages == 0 {
                bail!("fetch-notify throughput scenario requires --messages >= 1");
            }
            let ctx = runtimes.ctx.as_ref().ok_or_else(|| {
                anyhow!("fetch-notify throughput scenario requires broker runtime context")
            })?;
            let worker_started = Instant::now();
            let mut background_supervisor = worker.with_worker_runtime(runtimes.worker.clone());
            let worker_handle =
                tokio::spawn(async move { background_supervisor.serve_forever().await });
            report.push(StageTiming::new(
                "worker_task_spawn",
                worker_started.elapsed(),
            ));

            let scenario_result = async {
                let driver = runtimes.worker.clone();
                let manifest_started = Instant::now();
                let manifest_hash = upload_fetch_notify_manifest(&driver, universe_id)?;
                seed_http_builtins(ctx, universe_id)?;
                report.push(StageTiming::new(
                    "manifest_upload.fetch_notify",
                    manifest_started.elapsed(),
                ));

                let create_started = Instant::now();
                let accepted = driver.create_world(
                    universe_id,
                    CreateWorldRequest {
                        world_id: None,
                        universe_id,
                        created_at_ns: 1,
                        source: CreateWorldSource::Manifest { manifest_hash },
                    },
                )?;
                report.push(StageTiming::new("world_create_submit", create_started.elapsed()));

                let (world, stage) = wait_probe_stage(
                    "world_register_wait",
                    || match driver.get_world(universe_id, accepted.world_id) {
                        Ok(world) => Ok(Some(world)),
                        Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
                        Err(err) => Err(err.into()),
                    },
                )
                .await?;
                report.push(stage);

                let adapter_started = Instant::now();
                let adapter_handle = spawn_stub_http_adapter_pump(
                    ctx.clone(),
                    driver.clone(),
                    world.clone(),
                );
                report.push(StageTiming::new(
                    "adapter_task_spawn",
                    adapter_started.elapsed(),
                ));

                let benchmark_result = async {
                    let warm_submit_started = Instant::now();
                    submit_fetch_notify_start(
                        &driver,
                        &world,
                        format!("prof-fetch-throughput-warm-{iteration}"),
                    )?;
                    report.push(StageTiming::new(
                        "warm_event_submit",
                        warm_submit_started.elapsed(),
                    ));

                    let (_, stage) = wait_probe_stage("warm_event_end_to_end", || {
                        let state = driver.state_json(
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
                    let mut latencies = Vec::with_capacity(args.messages);
                    for index in 0..args.messages {
                        let event_started = Instant::now();
                        submit_fetch_notify_start(
                            &driver,
                            &world,
                            format!("prof-fetch-throughput-{iteration}-{index}"),
                        )?;
                        let target_request_id = (index + 2) as u64;
                        wait_until_fetch_notify_done(&driver, &world, target_request_id)
                            .await?;
                        latencies.push(event_started.elapsed());
                    }
                    let total = stream_started.elapsed();
                    let mut stage = StageTiming::new("steady_state_stream", total);
                    let stats = latency_stats(&latencies);
                    let throughput = if total.is_zero() {
                        0.0
                    } else {
                        args.messages as f64 / total.as_secs_f64()
                    };
                    stage.note = Some(format!(
                        "events={} throughput={throughput:.2}/s latency_ms(min={} avg={} p50={} p95={} p99={} max={})",
                        args.messages,
                        stats.min.as_millis(),
                        stats.avg.as_millis(),
                        stats.p50.as_millis(),
                        stats.p95.as_millis(),
                        stats.p99.as_millis(),
                        stats.max.as_millis(),
                    ));
                    report.push(stage);
                    Result::<(), anyhow::Error>::Ok(())
                }
                .await;

                adapter_handle.abort();
                let _ = adapter_handle.await;
                benchmark_result
            }
            .await;

            worker_handle.abort();
            let _ = worker_handle.await;
            scenario_result?;
        }
        Scenario::CounterThroughput => {
            if args.runtime != RuntimeKind::Broker {
                bail!("counter throughput scenario requires --runtime broker");
            }
            if args.messages == 0 {
                bail!("counter throughput scenario requires --messages >= 1");
            }

            let driver = runtimes.worker.clone();
            let sender = runtimes.control.clone();
            let mut supervisor = worker.with_worker_runtime(driver.clone());

            let scenario_result = async {
                let manifest_started = Instant::now();
                let manifest_hash = upload_counter_manifest(&driver, universe_id)?;
                report.push(StageTiming::new(
                    "manifest_upload.counter",
                    manifest_started.elapsed(),
                ));

                let create_started = Instant::now();
                let accepted = driver.create_world(
                    universe_id,
                    CreateWorldRequest {
                        world_id: None,
                        universe_id,
                        created_at_ns: 1,
                        source: CreateWorldSource::Manifest { manifest_hash },
                    },
                )?;
                report.push(StageTiming::new("world_create_submit", create_started.elapsed()));

                let (_, stage) = wait_stage(
                    "world_register_wait",
                    &mut supervisor,
                    || Ok(driver.is_world_active(universe_id, accepted.world_id)?.then_some(())),
                )
                .await?;
                report.push(stage);

                let (send_world, stage) = wait_stage("sender_route_sync", &mut supervisor, || {
                    match sender.get_world(universe_id, accepted.world_id) {
                        Ok(summary) => Ok(Some(summary)),
                        Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
                        Err(err) => Err(err.into()),
                    }
                })
                .await?;
                report.push(stage);

                let warm_submit_started = Instant::now();
                submit_counter_start(
                    &sender,
                    &send_world,
                    format!("prof-counter-warm-{iteration}"),
                    args.messages as u64,
                )?;
                report.push(StageTiming::new(
                    "warm_event_submit",
                    warm_submit_started.elapsed(),
                ));

                let (_, stage) = wait_stage("warm_event_end_to_end", &mut supervisor, || {
                    let state = driver.active_state_json(
                        universe_id,
                        accepted.world_id,
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
                let sender_runtime = sender.clone();
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
                    Result::<_, anyhow::Error>::Ok((
                        send_started,
                        send_started.elapsed(),
                        submit_times,
                    ))
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
                        sender_result = Some(match sender_handle.take().expect("sender handle").await
                        {
                            Ok(Ok(result)) => result,
                            Ok(Err(err)) => return Err(err),
                            Err(err) => return Err(err.into()),
                        });
                    }
                    let (_, profile) = supervisor.run_once_profiled().await?;
                    observe_stage.cycles += 1;
                    observe_stage.run.add(profile);
                    let probe_started = Instant::now();
                    let state = driver.active_state_json(
                        universe_id,
                        accepted.world_id,
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
                    tokio::time::sleep(WAIT_SLEEP).await;
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
                let mut stage = StageTiming::new("steady_state_stream", total);
                let stats = latency_stats(&latencies);
                let throughput = if total.is_zero() {
                    0.0
                } else {
                    args.messages as f64 / total.as_secs_f64()
                };
                stage.note = Some(format!(
                    "events={} throughput={throughput:.2}/s latency_ms(min={} avg={} p50={} p95={} p99={} max={})",
                    args.messages,
                    stats.min.as_millis(),
                    stats.avg.as_millis(),
                    stats.p50.as_millis(),
                    stats.p95.as_millis(),
                    stats.p99.as_millis(),
                    stats.max.as_millis(),
                ));
                report.push(stage);
                Result::<(), anyhow::Error>::Ok(())
            }
            .await;
            scenario_result?;
        }
        Scenario::DemiurgeTask => {
            let mut supervisor = worker.with_worker_runtime(runtimes.worker.clone());

            let manifest_started = Instant::now();
            let manifest_hash = upload_demiurge_manifest(&runtimes.control, universe_id)?;
            report.push(StageTiming::new(
                "manifest_upload.demiurge",
                manifest_started.elapsed(),
            ));

            let secret_started = Instant::now();
            let demiurge =
                sync_demiurge_secrets_and_resolve_config(&args, &runtimes.control, universe_id)?;
            let mut secret_stage =
                StageTiming::new("secret_sync.demiurge", secret_started.elapsed());
            secret_stage.note = Some(format!(
                "provider={} model={} live_provider={} synced_bindings={}",
                demiurge.provider,
                demiurge.model,
                demiurge.live_provider,
                if demiurge.synced_bindings.is_empty() {
                    "<none>".to_string()
                } else {
                    demiurge.synced_bindings.join(",")
                }
            ));
            report.push(secret_stage);

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

            let (world, stage) = wait_stage("world_register_wait", &mut supervisor, || {
                match runtimes.control.get_world(universe_id, accepted.world_id) {
                    Ok(world) => Ok(Some(world)),
                    Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
                    Err(err) => Err(err.into()),
                }
            })
            .await?;
            report.push(stage);

            for stage in run_demiurge_task_profiled(
                "task",
                format!("prof-demiurge-{iteration}"),
                &runtimes.control,
                &runtimes.worker,
                &mut supervisor,
                &world,
                &demiurge,
            )
            .await?
            {
                report.push(stage);
            }
        }
        Scenario::DemiurgeRestartRepro => {
            if args.runtime != RuntimeKind::Broker {
                bail!("demiurge-restart-repro requires --runtime broker");
            }
            let ctx = runtimes
                .ctx
                .clone()
                .ok_or_else(|| anyhow!("demiurge-restart-repro requires broker runtime context"))?;
            let shared_state_root = temp_state_root(&format!("prof-demiurge-restart-{iteration}"));

            let manifest_started = Instant::now();
            let manifest_hash = upload_demiurge_manifest(&runtimes.control, universe_id)?;
            report.push(StageTiming::new(
                "manifest_upload.demiurge",
                manifest_started.elapsed(),
            ));

            let secret_started = Instant::now();
            let demiurge =
                sync_demiurge_secrets_and_resolve_config(&args, &runtimes.control, universe_id)?;
            if !demiurge.live_provider {
                bail!(
                    "demiurge-restart-repro requires a live provider secret in worlds/demiurge/.env"
                );
            }
            let mut secret_stage =
                StageTiming::new("secret_sync.demiurge", secret_started.elapsed());
            secret_stage.note = Some(format!(
                "provider={} model={} live_provider={} synced_bindings={}",
                demiurge.provider,
                demiurge.model,
                demiurge.live_provider,
                demiurge.synced_bindings.join(",")
            ));
            report.push(secret_stage);

            let mut control =
                broker_control_runtime_at(&ctx, "control-initial", &shared_state_root)?;
            let mut worker_runtime =
                broker_worker_runtime_at(&ctx, "worker-initial", &shared_state_root)?;
            let mut supervisor = worker.with_worker_runtime(worker_runtime.clone());

            let create_started = Instant::now();
            let accepted = control.create_world(
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

            let (world, stage) = wait_for_clean_world_summary(
                "world_register_wait",
                &control,
                &mut supervisor,
                universe_id,
                accepted.world_id,
            )
            .await?;
            report.push(stage);

            drop(supervisor);
            drop(worker_runtime);
            drop(control);

            control = broker_control_runtime_at(&ctx, "control-restart-1", &shared_state_root)?;
            worker_runtime =
                broker_worker_runtime_at(&ctx, "worker-restart-1", &shared_state_root)?;
            supervisor = worker.with_worker_runtime(worker_runtime.clone());
            let (world, stage) = wait_for_clean_world_summary(
                "restart_1_clean_recover_wait",
                &control,
                &mut supervisor,
                world.universe_id,
                world.world_id,
            )
            .await?;
            report.push(stage);

            for task_index in 1..=2 {
                for stage in run_demiurge_task_profiled(
                    &format!("task{task_index}"),
                    format!("prof-demiurge-restart-{iteration}-{task_index}"),
                    &control,
                    &worker_runtime,
                    &mut supervisor,
                    &world,
                    &demiurge,
                )
                .await?
                {
                    report.push(stage);
                }
            }

            drop(supervisor);
            drop(worker_runtime);
            drop(control);

            control = broker_control_runtime_at(&ctx, "control-restart-2", &shared_state_root)?;
            worker_runtime =
                broker_worker_runtime_at(&ctx, "worker-restart-2", &shared_state_root)?;
            supervisor = worker.with_worker_runtime(worker_runtime.clone());
            let ((summary, warning), mut stage) = wait_for_disabled_world_summary(
                "restart_2_corruption_wait",
                &control,
                &mut supervisor,
                world.universe_id,
                world.world_id,
            )
            .await?;
            stage.note = Some(format!(
                "reproduced=true world_id={} warning={}",
                summary.world_id, warning
            ));
            report.push(stage);

            let mut note = StageTiming::new("repro_summary", Duration::ZERO);
            note.note = Some(format!(
                "world_epoch={} next_world_seq={} warnings={}",
                summary.world_epoch,
                summary.next_world_seq,
                summary.warnings.join(" | ")
            ));
            report.push(note);
        }
        Scenario::DemiurgeRestartInproc => unreachable!("handled before runtime creation"),
    }

    Ok(report)
}

async fn run_demiurge_restart_inproc_iteration(
    args: &Args,
    iteration: usize,
    _universe_id: UniverseId,
) -> Result<IterationReport> {
    if args.runtime != RuntimeKind::Broker {
        bail!("demiurge-restart-inproc requires --runtime broker");
    }

    let mut report = IterationReport::default();
    let mut task_ids: Vec<String> = Vec::new();
    let partition_count = args.partition_count.max(1);
    let universe_id = aos_node::local_universe_id();
    let state_root = managed_repro_state_root();

    let reset_started = Instant::now();
    reset_managed_hosted_stack(&state_root)?;
    let mut reset_stage = StageTiming::new("managed_stack_reset", reset_started.elapsed());
    reset_stage.note = Some(format!(
        "state_root={} universe_id={}",
        state_root.display(),
        universe_id
    ));
    report.push(reset_stage);

    let config_started = Instant::now();
    let kafka_config = managed_broker_kafka_config(partition_count)?;
    let blobstore_config = managed_blobstore_config()?;
    report.push(StageTiming::new(
        "managed_config_load",
        config_started.elapsed(),
    ));

    let control_started = Instant::now();
    let control = managed_control_facade(
        partition_count,
        &state_root,
        universe_id,
        kafka_config.clone(),
        blobstore_config.clone(),
    )?;
    report.push(StageTiming::new(
        "control_bootstrap",
        control_started.elapsed(),
    ));

    let manifest_started = Instant::now();
    let manifest_hash = upload_demiurge_manifest_via_control(&control, universe_id)?;
    report.push(StageTiming::new(
        "manifest_upload.demiurge",
        manifest_started.elapsed(),
    ));

    let secret_started = Instant::now();
    let demiurge =
        sync_demiurge_secrets_and_resolve_config_via_control(args, &control, universe_id)?;
    if !demiurge.live_provider {
        bail!("demiurge-restart-inproc requires a live provider secret in worlds/demiurge/.env");
    }
    let mut secret_stage = StageTiming::new("secret_sync.demiurge", secret_started.elapsed());
    secret_stage.note = Some(format!(
        "provider={} model={} live_provider={} synced_bindings={}",
        demiurge.provider,
        demiurge.model,
        demiurge.live_provider,
        demiurge.synced_bindings.join(",")
    ));
    report.push(secret_stage);

    let worker = HostedWorker::new(node_equivalent_worker_config(args));

    let worker_started = Instant::now();
    let mut worker_runtime = build_worker_runtime_broker(
        partition_count,
        &state_root,
        universe_id,
        kafka_config.clone(),
        blobstore_config.clone(),
    )?;
    report.push(StageTiming::new(
        "worker_boot.initial",
        worker_started.elapsed(),
    ));
    let mut supervisor = worker.with_worker_runtime(worker_runtime.clone());

    let create_started = Instant::now();
    let accepted = control.create_world(CreateWorldBody {
        world_id: None,
        universe_id,
        created_at_ns: 1,
        source: CreateWorldSource::Manifest { manifest_hash },
    })?;
    report.push(StageTiming::new(
        "world_create_submit",
        create_started.elapsed(),
    ));

    let (world, stage) = wait_for_clean_world_summary(
        "world_register_wait",
        &worker_runtime,
        &mut supervisor,
        universe_id,
        accepted.world_id,
    )
    .await?;
    report.push(stage);

    drop(supervisor);
    drop(worker_runtime);

    let restart_started = Instant::now();
    worker_runtime = build_worker_runtime_broker(
        partition_count,
        &state_root,
        universe_id,
        kafka_config.clone(),
        blobstore_config.clone(),
    )?;
    report.push(StageTiming::new(
        "worker_boot.restart_1",
        restart_started.elapsed(),
    ));
    supervisor = worker.with_worker_runtime(worker_runtime.clone());
    let (world, stage) = wait_for_clean_world_summary(
        "restart_1_clean_recover_wait",
        &worker_runtime,
        &mut supervisor,
        world.universe_id,
        world.world_id,
    )
    .await?;
    report.push(stage);

    for task_index in 1..=args.demiurge_task_count {
        let stages = run_demiurge_task_via_control_profiled(
            &format!("task{task_index}"),
            format!("prof-demiurge-inproc-{iteration}-{task_index}"),
            &control,
            &worker_runtime,
            &mut supervisor,
            &world,
            &demiurge,
        )
        .await?;
        if let Some(task_id) = extract_task_id_from_stages(&stages) {
            task_ids.push(task_id);
        }
        for stage in stages {
            report.push(stage);
        }
    }

    drop(supervisor);
    drop(worker_runtime);

    let restart_started = Instant::now();
    worker_runtime = build_worker_runtime_broker(
        partition_count,
        &state_root,
        universe_id,
        kafka_config,
        blobstore_config,
    )?;
    report.push(StageTiming::new(
        "worker_boot.restart_2",
        restart_started.elapsed(),
    ));
    supervisor = worker.with_worker_runtime(worker_runtime.clone());
    let ((summary, warning), mut stage) = wait_for_disabled_world_summary(
        "restart_2_corruption_wait",
        &worker_runtime,
        &mut supervisor,
        world.universe_id,
        world.world_id,
    )
    .await?;
    stage.note = Some(format!(
        "reproduced=true world_id={} warning={}",
        summary.world_id, warning
    ));
    report.push(stage);

    let trace_started = Instant::now();
    let mut trace_stage = StageTiming::new("restart_2_trace_summary", trace_started.elapsed());
    trace_stage.note = Some(
        match worker_runtime.trace_summary(summary.universe_id, summary.world_id) {
            Ok(trace) => truncate_note(&trace.to_string(), 240),
            Err(err) => format!("error={}", truncate_note(&err.to_string(), 200)),
        },
    );
    report.push(trace_stage);

    let mut note = StageTiming::new("repro_summary", Duration::ZERO);
    note.note = Some(format!(
        "world_id={} world_epoch={} next_world_seq={} warnings={}",
        summary.world_id,
        summary.world_epoch,
        summary.next_world_seq,
        summary.warnings.join(" | ")
    ));
    report.push(note);

    let diagnostics_started = Instant::now();
    let mut diagnostics = StageTiming::new("reopen_diagnostics", diagnostics_started.elapsed());
    diagnostics.note = Some(manual_reopen_diagnostics(
        &worker_runtime,
        &summary,
        &task_ids,
    )?);
    report.push(diagnostics);

    Ok(report)
}

fn create_runtimes(
    kind: RuntimeKind,
    partition_count: u32,
    iteration: usize,
) -> Result<RuntimePair> {
    match kind {
        RuntimeKind::Embedded => {
            let runtime = HostedWorkerRuntime::new_embedded_with_state_root(
                partition_count,
                temp_state_root(&format!("prof-embedded-{iteration}")),
            )?;
            Ok(RuntimePair {
                control: runtime.clone(),
                worker: runtime,
                ctx: None,
            })
        }
        RuntimeKind::Broker => {
            let ctx = broker_runtime_context("prof-broker", partition_count)?;
            Ok(RuntimePair {
                control: ctx.control_runtime(&format!("control-{iteration}"))?,
                worker: ctx.worker_runtime(&format!("worker-{iteration}"))?,
                ctx: Some(ctx),
            })
        }
        RuntimeKind::Direct => {
            let ctx = broker_runtime_context("prof-direct", partition_count)?;
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
    let hosted_up_checkpoint_every_events = (args.checkpoint_every_events > 0)
        .then_some(args.checkpoint_every_events)
        .or_else(|| {
            matches!(
                args.scenario,
                Scenario::DemiurgeRestartRepro | Scenario::DemiurgeRestartInproc
            )
            .then_some(100)
        });
    HostedWorkerConfig {
        worker_id: format!("prof-worker-{}", uuid::Uuid::new_v4()),
        partition_count: args.partition_count,
        supervisor_poll_interval: Duration::from_millis(1),
        checkpoint_interval: if matches!(
            args.scenario,
            Scenario::DemiurgeRestartRepro | Scenario::DemiurgeRestartInproc
        ) {
            Duration::from_millis(30_000)
        } else {
            Duration::from_secs(3600)
        },
        checkpoint_every_events: hosted_up_checkpoint_every_events,
        checkpoint_on_create: matches!(
            args.scenario,
            Scenario::DemiurgeRestartRepro | Scenario::DemiurgeRestartInproc
        ),
    }
}

fn node_equivalent_worker_config(args: &Args) -> HostedWorkerConfig {
    HostedWorkerConfig {
        worker_id: format!("prof-worker-{}", uuid::Uuid::new_v4()),
        partition_count: args.partition_count.max(1),
        supervisor_poll_interval: Duration::from_millis(500),
        checkpoint_interval: Duration::from_millis(30_000),
        checkpoint_every_events: (args.checkpoint_every_events > 0)
            .then_some(args.checkpoint_every_events)
            .or(Some(100)),
        checkpoint_on_create: true,
    }
}

fn managed_repro_state_root() -> PathBuf {
    repo_root().join(".aos-hosted")
}

fn reset_managed_hosted_stack(state_root: &Path) -> Result<()> {
    let reset_script = repo_root().join("dev/scripts/hosted-topics-reset.sh");
    let output = Command::new(&reset_script)
        .current_dir(repo_root())
        .output()
        .with_context(|| format!("run {}", reset_script.display()))?;
    if !output.status.success() {
        bail!(
            "{} failed: {}",
            reset_script.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if state_root.exists() {
        fs::remove_dir_all(state_root)
            .with_context(|| format!("remove {}", state_root.display()))?;
    }
    Ok(())
}

fn managed_broker_kafka_config(partition_count: u32) -> Result<KafkaConfig> {
    let mut config = KafkaConfig::default();
    let bootstrap_servers = config
        .bootstrap_servers
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if bootstrap_servers.is_none() {
        bail!("AOS_KAFKA_BOOTSTRAP_SERVERS must be set for demiurge-restart-inproc broker runtime");
    }
    config.direct_assigned_partitions = (0..partition_count.max(1)).collect();
    Ok(config)
}

fn managed_blobstore_config() -> Result<BlobStoreConfig> {
    let config = BlobStoreConfig::default();
    let bucket = config
        .bucket
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if bucket.is_none() {
        bail!("AOS_BLOBSTORE_BUCKET or AOS_S3_BUCKET must be set for demiurge-restart-inproc");
    }
    if config.prefix.trim().is_empty() {
        bail!("AOS_BLOBSTORE_PREFIX must not be empty");
    }
    if config.pack_threshold_bytes == 0 || config.pack_target_bytes == 0 {
        bail!(
            "blobstore pack thresholds must be positive; check AOS_BLOBSTORE_PACK_THRESHOLD_BYTES and AOS_BLOBSTORE_PACK_TARGET_BYTES"
        );
    }
    if config.pack_threshold_bytes > config.pack_target_bytes {
        bail!("AOS_BLOBSTORE_PACK_THRESHOLD_BYTES must be <= AOS_BLOBSTORE_PACK_TARGET_BYTES");
    }
    Ok(config)
}

fn managed_control_facade(
    partition_count: u32,
    state_root: &Path,
    default_universe_id: UniverseId,
    kafka_config: KafkaConfig,
    blobstore_config: BlobStoreConfig,
) -> Result<ControlFacade> {
    Ok(ControlFacade::new(build_control_deps_broker(
        partition_count,
        state_root,
        default_universe_id,
        kafka_config,
        blobstore_config,
    )?)?)
}

fn manual_reopen_diagnostics(
    runtime: &HostedWorkerRuntime,
    world: &HostedWorldSummary,
    task_ids: &[String],
) -> Result<String> {
    let partition = partition_for_world(world.world_id, 1);
    let entries = runtime.partition_entries(partition)?;
    let partition_worlds = entries.iter().fold(BTreeMap::new(), |mut acc, entry| {
        *acc.entry(entry.frame.world_id).or_insert(0usize) += 1;
        acc
    });
    let frames = entries
        .iter()
        .filter(|entry| {
            entry.frame.universe_id == world.universe_id && entry.frame.world_id == world.world_id
        })
        .map(|entry| entry.frame.clone())
        .collect::<Vec<_>>();
    let all_entries = journal_entries_from_world_frames(&frames)?;
    let loaded = runtime.load_manifest(world.universe_id, &world.manifest_hash)?;
    let store = runtime.cas_store_for_domain(world.universe_id)?;
    let world_config = aos_runtime::WorldConfig::from_env_with_fallback_module_cache_dir(None);
    let kernel_config = aos_kernel::KernelConfig {
        universe_id: world.universe_id.as_uuid(),
        secret_resolver: Some(Arc::new(
            runtime.vault()?.resolver_for_universe(world.universe_id),
        )),
        ..aos_kernel::KernelConfig::default()
    };
    let frames_result = open_plane_world_from_frames(
        Arc::clone(&store),
        loaded.clone(),
        &frames,
        world_config.clone(),
        aos_effect_adapters::config::EffectAdapterConfig::default(),
        kernel_config.clone(),
    );
    let checkpoint = runtime.latest_checkpoint(world.universe_id, partition)?;
    let snapshot_records = snapshot_records_from_entries(&all_entries)?;

    let checkpoint_note = if let Some(checkpoint) = checkpoint.as_ref() {
        if let Some(world_checkpoint) = checkpoint
            .worlds
            .iter()
            .find(|item| item.universe_id == world.universe_id && item.world_id == world.world_id)
        {
            let tail_frames = entries
                .iter()
                .filter(|entry| entry.offset > checkpoint.journal_offset)
                .filter(|entry| {
                    entry.frame.universe_id == world.universe_id
                        && entry.frame.world_id == world.world_id
                })
                .map(|entry| entry.frame.clone())
                .collect::<Vec<_>>();
            let checkpoint_tail_entries = journal_entries_from_world_frames(&tail_frames)?;
            let expected_tail_entries = all_entries
                .iter()
                .filter(|entry| entry.seq > world_checkpoint.baseline.height)
                .cloned()
                .collect::<Vec<_>>();
            let tail_match = seqs_of(&checkpoint_tail_entries) == seqs_of(&expected_tail_entries);
            let tail_expected = seq_window_note(&expected_tail_entries);
            let tail_checkpoint = seq_window_note(&checkpoint_tail_entries);
            let baseline_entries = all_entries
                .iter()
                .filter(|entry| entry.seq <= world_checkpoint.baseline.height)
                .cloned()
                .collect::<Vec<_>>();
            let baseline_log = open_world_from_entries(
                Arc::clone(&store),
                loaded.clone(),
                &baseline_entries,
                world_config.clone(),
                kernel_config.clone(),
            )?;
            let checkpoint_only = open_plane_world_from_checkpoint(
                Arc::clone(&store),
                loaded.clone(),
                &world_checkpoint.baseline,
                &[],
                world_config.clone(),
                aos_effect_adapters::config::EffectAdapterConfig::default(),
                kernel_config.clone(),
            )?;
            let baseline_compare = compare_runtime_snapshots(&baseline_log, &checkpoint_only)?;
            let checkpoint_result = open_plane_world_from_checkpoint(
                Arc::clone(&store),
                loaded.clone(),
                &world_checkpoint.baseline,
                &tail_frames,
                world_config.clone(),
                aos_effect_adapters::config::EffectAdapterConfig::default(),
                kernel_config.clone(),
            );
            let journal_snapshot_note = snapshot_records
                .iter()
                .rev()
                .take(3)
                .map(|snapshot| {
                    format!(
                        "{}@{}",
                        snapshot.height,
                        truncate_note(&snapshot.snapshot_ref, 16)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            let session_note = task_ids
                .iter()
                .take(2)
                .map(|task_id| -> Result<String> {
                    Ok(format!(
                        "{}:{}|{}",
                        &task_id[..8.min(task_id.len())],
                        session_state_brief(&baseline_log, task_id)?,
                        session_state_brief(&checkpoint_only, task_id)?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?
                .join(";");
            let tail_head = expected_tail_entries
                .iter()
                .take(3)
                .map(entry_brief)
                .collect::<Result<Vec<_>>>()?
                .join(",");
            let checkpoint_result_note = match checkpoint_result {
                Ok(_) => "checkpoint=ok".to_string(),
                Err(err) => format!(
                    "checkpoint=err err={}",
                    truncate_note(&err.to_string(), 120)
                ),
            };
            format!(
                "partition_worlds={} checkpoint_worlds={} checkpoint created_at_ns={} journal_offset={} baseline_height={} baseline_ref={} latest_snapshots=[{}] baseline_match={} tail_match={} expected_tail={} checkpoint_tail={} tail_head=[{}] session_baseline_vs_checkpoint=[{}] {}",
                partition_worlds
                    .iter()
                    .map(|(world_id, count)| format!("{}:{}", world_id, count))
                    .collect::<Vec<_>>()
                    .join(","),
                checkpoint.worlds.len(),
                checkpoint.created_at_ns,
                checkpoint.journal_offset,
                world_checkpoint.baseline.height,
                truncate_note(&world_checkpoint.baseline.snapshot_ref, 20),
                journal_snapshot_note,
                baseline_compare,
                tail_match,
                tail_expected,
                tail_checkpoint,
                tail_head,
                session_note,
                checkpoint_result_note,
            )
        } else {
            format!(
                "checkpoint=missing_world created_at_ns={} journal_offset={}",
                checkpoint.created_at_ns, checkpoint.journal_offset
            )
        }
    } else {
        "checkpoint=none".to_string()
    };

    let frames_note = match frames_result {
        Ok(_) => format!("frames=ok frame_count={}", frames.len()),
        Err(err) => format!(
            "frames=err frame_count={} err={}",
            frames.len(),
            truncate_note(&err.to_string(), 180)
        ),
    };
    Ok(format!("{frames_note} {checkpoint_note}"))
}

fn open_world_from_entries(
    store: Arc<aos_node_hosted::blobstore::HostedCas>,
    loaded: aos_kernel::LoadedManifest,
    entries: &[OwnedJournalEntry],
    world_config: aos_runtime::WorldConfig,
    kernel_config: aos_kernel::KernelConfig,
) -> Result<aos_runtime::WorldHost<aos_node_hosted::blobstore::HostedCas>> {
    Ok(
        aos_runtime::WorldHost::from_loaded_manifest_with_journal_replay(
            store,
            loaded,
            Journal::from_entries(entries).map_err(|err| anyhow!(err.to_string()))?,
            world_config,
            aos_effect_adapters::config::EffectAdapterConfig::default(),
            kernel_config,
            None,
        )?,
    )
}

fn snapshot_records_from_entries(
    entries: &[OwnedJournalEntry],
) -> Result<Vec<aos_kernel::journal::SnapshotRecord>> {
    entries
        .iter()
        .filter_map(
            |entry| match serde_cbor::from_slice::<JournalRecord>(&entry.payload) {
                Ok(JournalRecord::Snapshot(snapshot)) => Some(Ok(snapshot)),
                Ok(_) => None,
                Err(err) => Some(Err(err.into())),
            },
        )
        .collect()
}

fn compare_runtime_snapshots(
    lhs: &aos_runtime::WorldHost<aos_node_hosted::blobstore::HostedCas>,
    rhs: &aos_runtime::WorldHost<aos_node_hosted::blobstore::HostedCas>,
) -> Result<String> {
    let workflow_instances_match = to_canonical_cbor(&lhs.kernel().workflow_instances_snapshot())?
        == to_canonical_cbor(&rhs.kernel().workflow_instances_snapshot())?;
    let pending_match = to_canonical_cbor(&lhs.kernel().pending_workflow_receipts_snapshot())?
        == to_canonical_cbor(&rhs.kernel().pending_workflow_receipts_snapshot())?;
    let queued_match = to_canonical_cbor(&lhs.kernel().queued_effects_snapshot())?
        == to_canonical_cbor(&rhs.kernel().queued_effects_snapshot())?;
    Ok(format!(
        "workflow_instances={} pending_receipts={} queued_effects={}",
        workflow_instances_match, pending_match, queued_match
    ))
}

fn seqs_of(entries: &[OwnedJournalEntry]) -> Vec<u64> {
    entries.iter().map(|entry| entry.seq).collect()
}

fn seq_window_note(entries: &[OwnedJournalEntry]) -> String {
    match (entries.first(), entries.last()) {
        (Some(first), Some(last)) => format!("{}..{}({})", first.seq, last.seq, entries.len()),
        _ => "empty".into(),
    }
}

fn entry_brief(entry: &OwnedJournalEntry) -> Result<String> {
    let record: JournalRecord = serde_cbor::from_slice(&entry.payload)?;
    let detail = match record {
        JournalRecord::DomainEvent(event) => format!(
            "domain:{}:{}",
            event.schema,
            event
                .key
                .as_ref()
                .map(|key| truncate_note(&String::from_utf8_lossy(key), 24))
                .unwrap_or_else(|| "-".into())
        ),
        JournalRecord::EffectIntent(intent) => {
            format!(
                "intent:{}:{}",
                intent.kind,
                hex::encode(&intent.intent_hash[..4])
            )
        }
        JournalRecord::EffectReceipt(receipt) => {
            format!(
                "receipt:{}:{}",
                receipt.adapter_id,
                hex::encode(&receipt.intent_hash[..4])
            )
        }
        JournalRecord::StreamFrame(frame) => format!(
            "stream:{}:{}:{}",
            frame.effect_kind,
            hex::encode(&frame.intent_hash[..4]),
            frame.seq
        ),
        JournalRecord::Snapshot(snapshot) => format!("snapshot:{}", snapshot.height),
        JournalRecord::Custom(custom) => format!("custom:{}", custom.tag),
        other => format!("{:?}", other.kind()).to_lowercase(),
    };
    Ok(format!("{}={detail}", entry.seq))
}

fn session_state_brief(
    host: &aos_runtime::WorldHost<aos_node_hosted::blobstore::HostedCas>,
    task_id: &str,
) -> Result<String> {
    let key = to_canonical_cbor(&task_id)?;
    let Some(bytes) = host.state("aos.agent/SessionWorkflow@1", Some(&key)) else {
        return Ok("missing".into());
    };
    let state: serde_json::Value =
        serde_json::to_value(serde_cbor::from_slice::<serde_cbor::Value>(&bytes)?)?;
    let lifecycle = state
        .get("lifecycle")
        .and_then(|value| value.get("$tag"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    let host_session_id = state
        .get("tool_runtime_context")
        .and_then(|value| value.get("host_session_id"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("-");
    let active_run = state
        .get("active_run_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("-");
    let inflight = state
        .get("in_flight_effects")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let updated_at = state
        .get("updated_at")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    Ok(format!(
        "lifecycle={lifecycle} host_session_id={} active_run_id={} in_flight_effects={} updated_at={updated_at}",
        truncate_note(host_session_id, 12),
        truncate_note(active_run, 12),
        inflight,
    ))
}

fn extract_task_id_from_stages(stages: &[StageTiming]) -> Option<String> {
    let note = stages.first()?.note.as_deref()?;
    let start = note.find("task_id=")? + "task_id=".len();
    let rest = &note[start..];
    let end = rest.find(' ').unwrap_or(rest.len());
    Some(rest[..end].to_owned())
}

fn broker_worker_runtime_at(
    ctx: &BrokerRuntimeContext,
    label: &str,
    state_root: &Path,
) -> Result<HostedWorkerRuntime> {
    HostedWorkerRuntime::new_broker_with_state_root(
        ctx.partition_count,
        state_root,
        worker_kafka_config_for_label(ctx, label),
        ctx.blobstore_config.clone(),
    )
    .map_err(Into::into)
}

fn broker_control_runtime_at(
    ctx: &BrokerRuntimeContext,
    label: &str,
    state_root: &Path,
) -> Result<HostedWorkerRuntime> {
    HostedWorkerRuntime::new_broker_with_state_root(
        ctx.partition_count,
        state_root,
        control_kafka_config_for_label(ctx, label),
        ctx.blobstore_config.clone(),
    )
    .map_err(Into::into)
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

async fn wait_stage<T>(
    name: &str,
    supervisor: &mut WorkerSupervisor,
    mut probe: impl FnMut() -> Result<Option<T>>,
) -> Result<(T, StageTiming)> {
    let started = Instant::now();
    let deadline = started + WAIT_DEADLINE;
    let mut stage = StageTiming {
        name: name.to_owned(),
        ..StageTiming::default()
    };
    while Instant::now() < deadline {
        let (_, profile) = supervisor.run_once_profiled().await?;
        stage.cycles += 1;
        stage.run.add(profile);

        let probe_started = Instant::now();
        if let Some(value) = probe()? {
            stage.probe_time += probe_started.elapsed();
            stage.elapsed = started.elapsed();
            return Ok((value, stage));
        }
        stage.probe_time += probe_started.elapsed();

        tokio::time::sleep(WAIT_SLEEP).await;
        stage.sleep_time += WAIT_SLEEP;
    }
    bail!("timed out waiting for stage '{name}'")
}

async fn wait_probe_stage<T>(
    name: &str,
    mut probe: impl FnMut() -> Result<Option<T>>,
) -> Result<(T, StageTiming)> {
    let started = Instant::now();
    let deadline = started + WAIT_DEADLINE;
    let mut stage = StageTiming {
        name: name.to_owned(),
        ..StageTiming::default()
    };
    while Instant::now() < deadline {
        stage.cycles += 1;
        let probe_started = Instant::now();
        if let Some(value) = probe()? {
            stage.probe_time += probe_started.elapsed();
            stage.elapsed = started.elapsed();
            return Ok((value, stage));
        }
        stage.probe_time += probe_started.elapsed();
        tokio::time::sleep(WAIT_SLEEP).await;
        stage.sleep_time += WAIT_SLEEP;
    }
    bail!("timed out waiting for stage '{name}'")
}

fn disabled_world_warning(world: &HostedWorldSummary) -> Option<&str> {
    world.warnings.iter().find_map(|warning| {
        warning
            .strip_prefix("disabled: ")
            .or_else(|| warning.strip_prefix("disabled:"))
    })
}

async fn wait_for_clean_world_summary(
    name: &str,
    runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    universe_id: UniverseId,
    world_id: aos_node::WorldId,
) -> Result<(HostedWorldSummary, StageTiming)> {
    wait_stage(name, supervisor, || {
        match runtime.get_world(universe_id, world_id) {
            Ok(world) => {
                if let Some(reason) = disabled_world_warning(&world) {
                    bail!(
                        "world {} in universe {} disabled during {}: {}",
                        world.world_id,
                        world.universe_id,
                        name,
                        reason
                    );
                }
                Ok(Some(world))
            }
            Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
            Err(err) => Err(err.into()),
        }
    })
    .await
}

async fn wait_for_disabled_world_summary(
    name: &str,
    runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    universe_id: UniverseId,
    world_id: aos_node::WorldId,
) -> Result<((HostedWorldSummary, String), StageTiming)> {
    wait_stage(name, supervisor, || {
        match runtime.get_world(universe_id, world_id) {
            Ok(world) => {
                let warning = disabled_world_warning(&world).map(str::to_owned);
                Ok(warning.map(|warning| (world, warning)))
            }
            Err(aos_node_hosted::WorkerError::UnknownWorld { .. }) => Ok(None),
            Err(err) => Err(err.into()),
        }
    })
    .await
}

async fn run_demiurge_task_profiled(
    stage_prefix: &str,
    submission_id: String,
    control: &HostedWorkerRuntime,
    worker: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    world: &HostedWorldSummary,
    demiurge: &DemiurgeRunConfig,
) -> Result<Vec<StageTiming>> {
    run_demiurge_task_profiled_with_submit(
        stage_prefix,
        submission_id,
        |submission_id, value| {
            control.submit_event(SubmitEventRequest {
                universe_id: world.universe_id,
                world_id: world.world_id,
                schema: "demiurge/TaskSubmitted@1".into(),
                value,
                submission_id: Some(submission_id),
                expected_world_epoch: Some(world.world_epoch),
            })?;
            Ok(())
        },
        worker,
        supervisor,
        world,
        demiurge,
    )
    .await
}

async fn run_demiurge_task_via_control_profiled(
    stage_prefix: &str,
    submission_id: String,
    control: &ControlFacade,
    worker: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    world: &HostedWorldSummary,
    demiurge: &DemiurgeRunConfig,
) -> Result<Vec<StageTiming>> {
    run_demiurge_task_profiled_with_submit(
        stage_prefix,
        submission_id,
        |submission_id, value| {
            control.submit_event(
                world.world_id,
                SubmitEventBody {
                    schema: "demiurge/TaskSubmitted@1".into(),
                    value: Some(value),
                    value_json: None,
                    value_b64: None,
                    key_b64: None,
                    correlation_id: None,
                    submission_id: Some(submission_id),
                    expected_world_epoch: Some(world.world_epoch),
                },
            )?;
            Ok(())
        },
        worker,
        supervisor,
        world,
        demiurge,
    )
    .await
}

async fn run_demiurge_task_profiled_with_submit(
    stage_prefix: &str,
    submission_id: String,
    submit: impl FnOnce(String, serde_json::Value) -> Result<()>,
    worker: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    world: &HostedWorldSummary,
    demiurge: &DemiurgeRunConfig,
) -> Result<Vec<StageTiming>> {
    let task_id = uuid::Uuid::new_v4().to_string();
    let submit_started = Instant::now();
    submit(submission_id, demiurge_task_event(&task_id, demiurge))?;
    let mut submit_stage = StageTiming::new(
        format!("{stage_prefix}.task_submit"),
        submit_started.elapsed(),
    );
    submit_stage.note = Some(format!(
        "task_id={} workdir={} task={}",
        task_id,
        demiurge.workdir.display(),
        demiurge.task
    ));

    let (_state, bootstrap_stage) = wait_stage(
        &format!("{stage_prefix}.task_bootstrap_wait"),
        supervisor,
        || {
            if let Ok(world_summary) = worker.get_world(world.universe_id, world.world_id)
                && let Some(reason) = disabled_world_warning(&world_summary)
            {
                bail!(
                    "world {} disabled during {} bootstrap: {}",
                    world.world_id,
                    stage_prefix,
                    reason
                );
            }
            let state = worker.state_json(
                world.universe_id,
                world.world_id,
                "demiurge/Demiurge@1",
                Some(task_id.as_str()),
            )?;
            Ok(state.filter(|state| {
                state
                    .get("host_session_id")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
            }))
        },
    )
    .await?;

    let mut stages = vec![submit_stage, bootstrap_stage];
    if demiurge.live_provider {
        let (state, complete_stage) = wait_stage(
            &format!("{stage_prefix}.task_complete_wait"),
            supervisor,
            || {
                if let Ok(world_summary) = worker.get_world(world.universe_id, world.world_id)
                    && let Some(reason) = disabled_world_warning(&world_summary)
                {
                    bail!(
                        "world {} disabled during {} completion: {}",
                        world.world_id,
                        stage_prefix,
                        reason
                    );
                }
                let state = worker.state_json(
                    world.universe_id,
                    world.world_id,
                    "demiurge/Demiurge@1",
                    Some(task_id.as_str()),
                )?;
                if let Some(state) = state.as_ref()
                    && let Some(failure) = state.get("failure")
                    && !failure.is_null()
                {
                    bail!("demiurge task failed: {}", failure);
                }
                Ok(state.filter(demiurge_task_finished))
            },
        )
        .await?;
        stages.push(complete_stage);

        let output_started = Instant::now();
        let assistant_text = extract_demiurge_assistant_text(worker, world.universe_id, &state)?;
        let mut output_stage = StageTiming::new(
            format!("{stage_prefix}.assistant_output_fetch"),
            output_started.elapsed(),
        );
        output_stage.note = Some(format!(
            "assistant_text={}",
            assistant_text
                .map(|value| truncate_note(&value, 160))
                .unwrap_or_else(|| "<missing>".into())
        ));
        stages.push(output_stage);
    } else {
        let mut note =
            StageTiming::new(format!("{stage_prefix}.task_complete_wait"), Duration::ZERO);
        note.note = Some(
            "provider resolved to mock; bootstrap path verified but no live LLM call was attempted"
                .into(),
        );
        stages.push(note);
    }

    Ok(stages)
}

fn temp_state_root(label: &str) -> PathBuf {
    ensure_shared_module_cache_env();
    std::env::temp_dir().join(format!("aos-node-hosted-{label}-{}", uuid::Uuid::new_v4()))
}

fn smoke_fixture_root(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../aos-smoke/fixtures")
        .join(name)
        .canonicalize()
        .expect("smoke fixture path")
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

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn demiurge_world_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        repo_root()
            .join("worlds/demiurge")
            .canonicalize()
            .expect("demiurge world root")
    })
    .clone()
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
    copy_dir_recursive(&src, &dst);
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
    aos_cbor::Hash::of_bytes(&bytes).to_hex()
}

fn collect_fixture_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read fixture dir") {
        let entry = entry.expect("fixture dir entry");
        let path = entry.path();
        let name = entry.file_name();
        if dir == root && matches!(name.to_str(), Some(".aos" | "target" | ".git")) {
            continue;
        }
        let file_type = entry.file_type().expect("fixture file type");
        if file_type.is_dir() {
            if matches!(name.to_str(), Some(".aos" | "target" | ".git")) {
                continue;
            }
            collect_fixture_files(root, &path, out);
        } else if file_type.is_file() {
            out.push(path);
        }
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create temp world root");
    for entry in fs::read_dir(src).expect("read source dir") {
        let entry = entry.expect("dir entry");
        let file_type = entry.file_type().expect("file type");
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), to).expect("copy fixture file");
        }
    }
}

fn prepare_authored_manifest(world_root: &Path) -> PreparedAuthoredManifest {
    let (store, bundle, _) = build_bundle_from_local_world_with_profile(
        world_root,
        false,
        WorkflowBuildProfile::Release,
    )
    .unwrap();
    let imported = import_genesis(&store, &bundle).unwrap();
    let loaded = ManifestLoader::load_from_bytes(&store, &imported.manifest_bytes).unwrap();
    let manifest: Manifest = serde_cbor::from_slice(&imported.manifest_bytes).unwrap();
    let mut seen = BTreeSet::new();
    let mut blobs = Vec::new();

    let mut push_blob = |bytes: Vec<u8>| {
        let hash = Hash::of_bytes(&bytes);
        if seen.insert(hash) {
            blobs.push(bytes);
        }
    };

    for secret in &bundle.secrets {
        push_blob(to_canonical_cbor(&AirNode::Defsecret(secret.clone())).unwrap());
    }
    for named in &manifest.schemas {
        if let Some(schema) = loaded.schemas.get(named.name.as_str()).cloned() {
            push_blob(to_canonical_cbor(&AirNode::Defschema(schema)).unwrap());
        } else if let Some(builtin) = builtins::find_builtin_schema(named.name.as_str()) {
            push_blob(to_canonical_cbor(&builtin.schema).unwrap());
        } else {
            panic!("missing schema ref {}", named.name);
        }
    }
    for named in &manifest.effects {
        if let Some(effect) = loaded.effects.get(named.name.as_str()).cloned() {
            push_blob(to_canonical_cbor(&AirNode::Defeffect(effect)).unwrap());
        } else if let Some(builtin) = builtins::find_builtin_effect(named.name.as_str()) {
            push_blob(to_canonical_cbor(&builtin.effect).unwrap());
        } else {
            panic!("missing effect ref {}", named.name);
        }
    }
    for named in &manifest.caps {
        if let Some(cap) = loaded.caps.get(named.name.as_str()).cloned() {
            push_blob(to_canonical_cbor(&AirNode::Defcap(cap)).unwrap());
        } else if let Some(builtin) = builtins::find_builtin_cap(named.name.as_str()) {
            push_blob(to_canonical_cbor(&builtin.cap).unwrap());
        } else {
            panic!("missing cap ref {}", named.name);
        }
    }
    for named in &manifest.policies {
        let policy = loaded.policies.get(named.name.as_str()).cloned().unwrap();
        push_blob(to_canonical_cbor(&AirNode::Defpolicy(policy)).unwrap());
    }
    for named in &manifest.modules {
        if let Some(module) = loaded.modules.get(named.name.as_str()).cloned() {
            push_blob(to_canonical_cbor(&AirNode::Defmodule(module.clone())).unwrap());
            let hash = Hash::from_hex_str(module.wasm_hash.as_str()).unwrap();
            let bytes = store.get_blob(hash).unwrap();
            push_blob(bytes);
        } else if let Some(builtin) = builtins::find_builtin_module(named.name.as_str()) {
            push_blob(to_canonical_cbor(&builtin.module).unwrap());
        } else {
            panic!("missing module ref {}", named.name);
        }
    }
    let manifest_value: serde_cbor::Value =
        serde_cbor::from_slice(&imported.manifest_bytes).expect("decode manifest value");
    let mut referenced_hashes = BTreeSet::new();
    collect_hash_refs_from_cbor(&manifest_value, &mut referenced_hashes);
    for hash in referenced_hashes {
        if let Ok(bytes) = store.get_blob(hash) {
            push_blob(bytes);
            continue;
        }
        if let Ok(node) = store.get_node::<serde_cbor::Value>(hash) {
            push_blob(serde_cbor::to_vec(&node).unwrap());
        }
    }
    PreparedAuthoredManifest {
        blobs,
        manifest_bytes: imported.manifest_bytes,
    }
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
    let prepared = PREPARED.get_or_init(|| prepare_authored_manifest(&counter_world_root()));
    upload_prepared_manifest(runtime, universe_id, prepared)
}

fn upload_fetch_notify_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> Result<String> {
    static PREPARED: OnceLock<PreparedAuthoredManifest> = OnceLock::new();
    let prepared = PREPARED.get_or_init(|| prepare_authored_manifest(&fetch_notify_world_root()));
    upload_prepared_manifest(runtime, universe_id, prepared)
}

fn upload_demiurge_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> Result<String> {
    static PREPARED: OnceLock<PreparedAuthoredManifest> = OnceLock::new();
    let prepared = PREPARED.get_or_init(|| prepare_authored_manifest(&demiurge_world_root()));
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

fn upload_demiurge_manifest_via_control(
    control: &ControlFacade,
    universe_id: UniverseId,
) -> Result<String> {
    static PREPARED: OnceLock<PreparedAuthoredManifest> = OnceLock::new();
    let prepared = PREPARED.get_or_init(|| prepare_authored_manifest(&demiurge_world_root()));
    upload_prepared_manifest_via_control(control, universe_id, prepared)
}

fn upload_prepared_manifest_via_control(
    control: &ControlFacade,
    universe_id: UniverseId,
    prepared: &PreparedAuthoredManifest,
) -> Result<String> {
    for bytes in &prepared.blobs {
        let _ = control.put_blob(bytes, Some(universe_id), None)?;
    }
    Ok(control
        .put_blob(&prepared.manifest_bytes, Some(universe_id), None)?
        .hash)
}

fn sync_demiurge_secrets_and_resolve_config(
    args: &Args,
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> Result<DemiurgeRunConfig> {
    let world_root = demiurge_world_root();
    let required_bindings = BTreeSet::from([
        "llm/openai_api".to_string(),
        "llm/anthropic_api".to_string(),
    ]);
    let available = load_available_secret_value_map(&world_root, None, &required_bindings)?;
    let vault = runtime.vault()?;
    let mut synced_bindings = Vec::new();
    for (binding_id, plaintext) in &available {
        let _ = vault.upsert_binding(
            universe_id,
            binding_id,
            UpsertSecretBinding {
                source_kind: SecretBindingSourceKind::NodeSecretStore,
                env_var: None,
                required_placement_pin: None,
                status: SecretBindingStatus::Active,
            },
        )?;
        let _ = vault.put_secret_value(
            universe_id,
            binding_id,
            plaintext,
            Some(Hash::of_bytes(plaintext).to_hex().as_str()),
            Some("hosted-prof".into()),
        )?;
        synced_bindings.push(binding_id.to_string());
    }
    synced_bindings.sort();

    let provider = args.demiurge_provider.clone().unwrap_or_else(|| {
        if available.contains_key("llm/openai_api") {
            "openai-responses".into()
        } else if available.contains_key("llm/anthropic_api") {
            "anthropic".into()
        } else {
            "mock".into()
        }
    });
    let model = args.demiurge_model.clone().unwrap_or_else(|| {
        if provider.contains("anthropic") {
            DEMIURGE_DEFAULT_MODEL_ANTHROPIC.into()
        } else {
            DEMIURGE_DEFAULT_MODEL_OPENAI.into()
        }
    });
    let live_provider = provider != "mock";
    if live_provider {
        let required_binding = if provider.contains("anthropic") {
            "llm/anthropic_api"
        } else {
            "llm/openai_api"
        };
        if !available.contains_key(required_binding) {
            bail!(
                "demiurge scenario provider '{}' requires secret binding '{}' in {}",
                provider,
                required_binding,
                world_root.join(".env").display()
            );
        }
    }

    Ok(DemiurgeRunConfig {
        provider,
        model,
        task: args.demiurge_task.clone(),
        workdir: args
            .demiurge_workdir
            .clone()
            .unwrap_or_else(repo_root)
            .canonicalize()
            .context("canonicalize demiurge workdir")?,
        max_tokens: args.demiurge_max_tokens,
        tool_profile: args.demiurge_tool_profile.clone(),
        allowed_tools: csv_arg_to_list(&args.demiurge_allowed_tools),
        tool_enable: csv_arg_to_list(&args.demiurge_tool_enable),
        live_provider,
        synced_bindings,
    })
}

fn sync_demiurge_secrets_and_resolve_config_via_control(
    args: &Args,
    control: &ControlFacade,
    universe_id: UniverseId,
) -> Result<DemiurgeRunConfig> {
    let world_root = demiurge_world_root();
    let required_bindings = BTreeSet::from([
        "llm/openai_api".to_string(),
        "llm/anthropic_api".to_string(),
    ]);
    let available = load_available_secret_value_map(&world_root, None, &required_bindings)?;
    let mut synced_bindings = Vec::new();
    for (binding_id, plaintext) in &available {
        let _ = control.upsert_secret_binding(
            universe_id,
            binding_id,
            UpsertSecretBindingBody {
                source_kind: SecretBindingSourceKind::NodeSecretStore,
                env_var: None,
                required_placement_pin: None,
                status: SecretBindingStatus::Active,
                actor: Some("hosted-prof".into()),
            },
        )?;
        let _ = control.put_secret_version(
            universe_id,
            binding_id,
            PutSecretVersionBody {
                plaintext_b64: BASE64_STANDARD.encode(plaintext),
                expected_digest: Some(Hash::of_bytes(plaintext).to_hex()),
                actor: Some("hosted-prof".into()),
            },
        )?;
        synced_bindings.push(binding_id.to_string());
    }
    synced_bindings.sort();

    let provider = args.demiurge_provider.clone().unwrap_or_else(|| {
        if available.contains_key("llm/openai_api") {
            "openai-responses".into()
        } else if available.contains_key("llm/anthropic_api") {
            "anthropic".into()
        } else {
            "mock".into()
        }
    });
    let model = args.demiurge_model.clone().unwrap_or_else(|| {
        if provider.contains("anthropic") {
            DEMIURGE_DEFAULT_MODEL_ANTHROPIC.into()
        } else {
            DEMIURGE_DEFAULT_MODEL_OPENAI.into()
        }
    });
    let live_provider = provider != "mock";
    if live_provider {
        let required_binding = if provider.contains("anthropic") {
            "llm/anthropic_api"
        } else {
            "llm/openai_api"
        };
        if !available.contains_key(required_binding) {
            bail!(
                "demiurge scenario provider '{}' requires secret binding '{}' in {}",
                provider,
                required_binding,
                world_root.join(".env").display()
            );
        }
    }

    Ok(DemiurgeRunConfig {
        provider,
        model,
        task: args.demiurge_task.clone(),
        workdir: args
            .demiurge_workdir
            .clone()
            .unwrap_or_else(repo_root)
            .canonicalize()
            .context("canonicalize demiurge workdir")?,
        max_tokens: args.demiurge_max_tokens,
        tool_profile: args.demiurge_tool_profile.clone(),
        allowed_tools: csv_arg_to_list(&args.demiurge_allowed_tools),
        tool_enable: csv_arg_to_list(&args.demiurge_tool_enable),
        live_provider,
        synced_bindings,
    })
}

fn csv_arg_to_list(value: &str) -> Option<Vec<String>> {
    let values = value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

fn demiurge_task_event(task_id: &str, config: &DemiurgeRunConfig) -> serde_json::Value {
    json!({
        "task_id": task_id,
        "observed_at_ns": 1,
        "workdir": config.workdir,
        "task": config.task,
        "config": {
            "provider": config.provider,
            "model": config.model,
            "reasoning_effort": serde_json::Value::Null,
            "max_tokens": config.max_tokens,
            "tool_profile": config.tool_profile,
            "allowed_tools": config.allowed_tools,
            "tool_enable": config.tool_enable,
            "tool_disable": serde_json::Value::Null,
            "tool_force": serde_json::Value::Null,
            "session_ttl_ns": serde_json::Value::Null,
        }
    })
}

fn demiurge_task_finished(state: &serde_json::Value) -> bool {
    state.get("finished").and_then(serde_json::Value::as_bool) == Some(true)
        && state
            .get("failure")
            .map(serde_json::Value::is_null)
            .unwrap_or(true)
        && state
            .get("host_session_id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
}

fn extract_demiurge_assistant_text(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
    state: &serde_json::Value,
) -> Result<Option<String>> {
    let Some(output_ref) = state.get("output_ref").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let hash = Hash::from_hex_str(output_ref)
        .with_context(|| format!("parse demiurge output_ref '{output_ref}'"))?;
    let bytes = runtime.get_blob(universe_id, hash)?;
    let payload: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("decode demiurge output blob '{output_ref}'"))?;
    Ok(payload
        .get("assistant_text")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned))
}

fn truncate_note(value: &str, max_chars: usize) -> String {
    let truncated = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn seed_http_builtins(ctx: &BrokerRuntimeContext, universe_id: UniverseId) -> Result<()> {
    let blobstore = ctx.blobstore()?;
    let upload_builtin = |bytes: Vec<u8>, expected: Hash| -> Result<()> {
        let uploaded = blobstore.put_blob(universe_id, &bytes)?;
        if uploaded != expected {
            bail!("unexpected blob hash while seeding HTTP builtins");
        }
        Ok(())
    };

    let schema = builtins::find_builtin_schema("sys/HttpRequestParams@1").unwrap();
    upload_builtin(to_canonical_cbor(&schema.schema)?, schema.hash)?;
    let schema = builtins::find_builtin_schema("sys/HttpRequestReceipt@1").unwrap();
    upload_builtin(to_canonical_cbor(&schema.schema)?, schema.hash)?;
    let effect = builtins::find_builtin_effect("sys/http.request@1").unwrap();
    upload_builtin(to_canonical_cbor(&effect.effect)?, effect.hash)?;
    let cap = builtins::find_builtin_cap("sys/http.out@1").unwrap();
    upload_builtin(to_canonical_cbor(&cap.cap)?, cap.hash)?;
    let module = builtins::find_builtin_module("sys/CapEnforceHttpOut@1").unwrap();
    upload_builtin(to_canonical_cbor(&module.module)?, module.hash)?;
    Ok(())
}

#[derive(Clone)]
struct BrokerRuntimeContext {
    kafka_config: KafkaConfig,
    blobstore_config: BlobStoreConfig,
    partition_count: u32,
}

impl BrokerRuntimeContext {
    fn worker_runtime(&self, label: &str) -> Result<HostedWorkerRuntime> {
        self.runtime_with_kafka(label, self.kafka_config.clone())
    }

    fn control_runtime(&self, label: &str) -> Result<HostedWorkerRuntime> {
        let mut kafka_config = self.kafka_config.clone();
        kafka_config.submission_group_prefix =
            format!("{}-control-{label}", kafka_config.submission_group_prefix);
        kafka_config.transactional_id =
            format!("{}-control-{label}", kafka_config.transactional_id);
        self.runtime_with_kafka(label, kafka_config)
    }

    fn direct_worker_runtime(
        &self,
        label: &str,
        partitions: &[u32],
    ) -> Result<HostedWorkerRuntime> {
        let mut kafka_config = self.kafka_config.clone();
        kafka_config.direct_assigned_partitions = partitions.iter().copied().collect();
        kafka_config.submission_group_prefix =
            format!("{}-direct-{label}", kafka_config.submission_group_prefix);
        kafka_config.transactional_id = format!("{}-direct-{label}", kafka_config.transactional_id);
        self.runtime_with_kafka(label, kafka_config)
    }

    fn blobstore(&self) -> Result<RemoteCasStore> {
        Ok(RemoteCasStore::new(self.blobstore_config.clone())?)
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

fn broker_runtime_context(label: &str, partition_count: u32) -> Result<BrokerRuntimeContext> {
    ensure_profile_env_loaded();
    let kafka_config = broker_kafka_config(label, partition_count)?
        .ok_or_else(|| anyhow!("Kafka not configured"))?;
    let mut blobstore_config =
        broker_blobstore_config(label)?.ok_or_else(|| anyhow!("blobstore not configured"))?;
    blobstore_config.pack_threshold_bytes = 0;
    Ok(BrokerRuntimeContext {
        kafka_config,
        blobstore_config,
        partition_count,
    })
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

fn broker_kafka_config(label: &str, partition_count: u32) -> Result<Option<KafkaConfig>> {
    let mut config = KafkaConfig::default();
    let Some(bootstrap) = config.bootstrap_servers.clone() else {
        return Ok(None);
    };
    if bootstrap.trim().is_empty() {
        return Ok(None);
    }
    let (ingress_topic, journal_topic, projection_topic) = unique_kafka_topics(partition_count)?;
    config.ingress_topic = ingress_topic;
    config.journal_topic = journal_topic;
    config.projection_topic = projection_topic;
    let suffix = format!("{label}-{}", uuid::Uuid::new_v4());
    config.submission_group_prefix = format!("{}-{suffix}", config.submission_group_prefix);
    config.transactional_id = format!("{}-{suffix}", config.transactional_id);
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

fn unique_kafka_topics(partition_count: u32) -> Result<(String, String, String)> {
    let suffix = format!("prof-shared-{}-{}", partition_count, uuid::Uuid::new_v4());
    let ingress = format!("aos-ingress-{suffix}");
    let journal = format!("aos-journal-{suffix}");
    let projection = format!("aos-projection-{suffix}");
    let mut config = KafkaConfig::default();
    config.ingress_topic = ingress.clone();
    config.journal_topic = journal.clone();
    config.projection_topic = projection.clone();
    ensure_kafka_topics(&config, partition_count)?;
    Ok((ingress, journal, projection))
}

fn ensure_kafka_topics(config: &KafkaConfig, partition_count: u32) -> Result<()> {
    create_kafka_topic(&config.ingress_topic, partition_count, false)?;
    create_kafka_topic(&config.journal_topic, partition_count, false)?;
    create_kafka_topic(&config.projection_topic, partition_count, true)?;
    Ok(())
}

fn create_kafka_topic(topic: &str, partitions: u32, compacted: bool) -> Result<()> {
    let mut args = vec![
        "exec".to_owned(),
        "aos-redpanda".to_owned(),
        "rpk".to_owned(),
        "topic".to_owned(),
        "create".to_owned(),
        topic.to_owned(),
        "--partitions".to_owned(),
        partitions.to_string(),
        "--replicas".to_owned(),
        "1".to_owned(),
    ];
    if compacted {
        args.push("--topic-config".to_owned());
        args.push("cleanup.policy=compact".to_owned());
    }
    let output = Command::new("docker")
        .args(&args)
        .output()
        .with_context(|| format!("create Kafka topic {topic}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(anyhow!(
        "create Kafka topic {topic} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    ))
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
        std::fs::create_dir_all(&dir).expect("create hosted prof module cache dir");
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
    let mut reader = HostedKafkaBackend::new(1, kafka_config)?;
    let deadline = Instant::now() + WAIT_DEADLINE;
    while Instant::now() < deadline {
        reader.recover_partition_from_broker(world.effective_partition)?;
        for frame in reader.world_frames(world.world_id) {
            for record in &frame.records {
                if let aos_kernel::journal::JournalRecord::EffectIntent(intent) = record {
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
    let pc = &state["pc"];
    match pc.get("$tag").and_then(|tag| tag.as_str()) {
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

async fn wait_until_fetch_notify_done(
    runtime: &HostedWorkerRuntime,
    world: &HostedWorldSummary,
    next_request_id: u64,
) -> Result<()> {
    let _: serde_json::Value = wait_probe_stage("fetch_notify_done", || {
        let state = runtime.state_json(
            world.universe_id,
            world.world_id,
            "demo/FetchNotify@1",
            None,
        )?;
        Ok(state.filter(|state| fetch_notify_done(state, next_request_id)))
    })
    .await?
    .0;
    Ok(())
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
        let mut reader = HostedKafkaBackend::new(1, kafka_config)?;
        let mut seen = HashSet::new();
        let mut registry = AdapterRegistry::new(AdapterRegistryConfig {
            effect_timeout: Duration::from_secs(5),
        });
        registry.register(Box::new(StubHttpAdapter));

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
                    let receipt = registry
                        .execute_batch(vec![intent.clone()])
                        .await
                        .into_iter()
                        .next()
                        .ok_or_else(|| anyhow!("adapter registry returned no receipt"))?;
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
                                aos_cbor::Hash::of_bytes(&intent.intent_hash).to_hex()
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

#[derive(Debug, Clone, Copy)]
struct LatencyStats {
    min: Duration,
    avg: Duration,
    p50: Duration,
    p95: Duration,
    p99: Duration,
    max: Duration,
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
    let mut by_stage: BTreeMap<&str, Vec<Duration>> = BTreeMap::new();
    let mut totals = Vec::with_capacity(reports.len());
    for report in reports {
        totals.push(report.total);
        for stage in &report.stages {
            by_stage
                .entry(stage.name.as_str())
                .or_default()
                .push(stage.elapsed);
        }
    }
    println!();
    println!("summary");
    for (name, values) in by_stage {
        println!(
            "  {:32} min={:>6} ms avg={:>6} ms max={:>6} ms",
            name,
            values.iter().min().copied().unwrap_or_default().as_millis(),
            average_duration(&values).as_millis(),
            values.iter().max().copied().unwrap_or_default().as_millis(),
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
