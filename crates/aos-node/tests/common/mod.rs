#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Once, OnceLock};
use std::time::{Duration, Instant};

use aos_air_types::{
    AirNode, DefModule, DefSchema, EffectBinding, HashRef, Manifest, NamedRef, Routing,
    RoutingEvent, SchemaRef, builtins,
};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_node::blobstore::{
    BlobStoreConfig, HostedBlobMetaStore, RemoteCasStore, scoped_blobstore_config,
};
use aos_node::config::HostedWorkerConfig;
use aos_node::kafka::KafkaConfig;
use aos_node::{BackendError, CreateWorldRequest, CreateWorldSource, UniverseId};
use aos_node::{HostedWorker, HostedWorkerRuntime, HostedWorldSummary, WorkerSupervisorHandle};
use aos_wasm_build::builder::{BuildRequest, Builder};
use axum::body::to_bytes;
use axum::http::StatusCode;
use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::config::ClientConfig;
use rdkafka::error::RDKafkaErrorCode;

const TEST_WAIT_SLEEP: Duration = Duration::from_millis(5);
const TEST_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct PreparedManifest {
    blobs: Vec<Vec<u8>>,
    manifest_bytes: Vec<u8>,
}

#[derive(Clone)]
pub struct BrokerRuntimeTestContext {
    pub kafka_config: KafkaConfig,
    pub blobstore_config: BlobStoreConfig,
    pub partition_count: u32,
}

impl BrokerRuntimeTestContext {
    pub fn worker_runtime(&self, label: &str) -> HostedWorkerRuntime {
        self.worker_runtime_in_universe(label, aos_node::UniverseId::nil())
    }

    pub fn worker_runtime_in_universe(
        &self,
        label: &str,
        universe_id: UniverseId,
    ) -> HostedWorkerRuntime {
        self.runtime_with_kafka(
            label,
            universe_id,
            worker_kafka_config_for_label(self, label),
        )
    }

    pub fn control_runtime(&self, label: &str) -> HostedWorkerRuntime {
        self.control_runtime_in_universe(label, aos_node::UniverseId::nil())
    }

    pub fn control_runtime_in_universe(
        &self,
        label: &str,
        universe_id: UniverseId,
    ) -> HostedWorkerRuntime {
        self.runtime_with_kafka(
            label,
            universe_id,
            control_kafka_config_for_label(self, label),
        )
    }

    pub fn remote_cas_for_universe(&self, universe_id: UniverseId) -> RemoteCasStore {
        RemoteCasStore::new(scoped_blobstore_config(&self.blobstore_config, universe_id))
            .expect("open remote CAS for test")
    }

    pub fn blob_meta_for_universe(&self, universe_id: UniverseId) -> HostedBlobMetaStore {
        HostedBlobMetaStore::new(scoped_blobstore_config(&self.blobstore_config, universe_id))
            .expect("open blob metadata store for test")
    }

    fn runtime_with_kafka(
        &self,
        label: &str,
        universe_id: UniverseId,
        kafka_config: KafkaConfig,
    ) -> HostedWorkerRuntime {
        HostedWorkerRuntime::new_kafka_with_state_root_and_universe(
            self.partition_count,
            temp_state_root(&format!("broker-{label}")),
            universe_id,
            kafka_config,
            self.blobstore_config.clone(),
        )
        .expect("open broker-backed runtime")
    }
}

pub fn embedded_runtime(partition_count: u32) -> HostedWorkerRuntime {
    HostedWorkerRuntime::new_embedded_kafka(partition_count).expect("embedded runtime")
}

pub fn hosted_worker() -> HostedWorker {
    HostedWorker::new(HostedWorkerConfig::default())
}

pub async fn wait_for_worker(supervisor: &mut WorkerSupervisorHandle) {
    supervisor
        .wait_for_progress(TEST_WAIT_SLEEP)
        .await
        .expect("background worker stays alive");
}

pub async fn wait_for_checkpoint(
    runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisorHandle,
    world: &HostedWorldSummary,
) -> aos_node::WorldCheckpointRef {
    let deadline = Instant::now() + TEST_WAIT_TIMEOUT;
    while Instant::now() < deadline {
        wait_for_worker(supervisor).await;
        let _ = runtime.trace_summary(world.universe_id, world.world_id);
        if let Some(checkpoint) = runtime
            .latest_world_checkpoint(world.universe_id, world.world_id)
            .unwrap()
        {
            return checkpoint;
        }
    }

    let checkpoint = runtime
        .latest_world_checkpoint(world.universe_id, world.world_id)
        .unwrap()
        .unwrap();
    checkpoint
}

pub fn hosted_universe_id(runtime: &HostedWorkerRuntime) -> UniverseId {
    runtime.default_universe_id().expect("default universe id")
}

pub fn create_counter_world(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> HostedWorldSummary {
    let manifest_hash = upload_counter_manifest(runtime, universe_id);
    create_world_from_manifest(runtime, universe_id, manifest_hash)
}

pub fn create_world_from_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
    manifest_hash: String,
) -> HostedWorldSummary {
    let accepted = runtime
        .create_world(
            universe_id,
            CreateWorldRequest {
                world_id: None,
                universe_id,
                created_at_ns: 1,
                source: CreateWorldSource::Manifest { manifest_hash },
            },
        )
        .expect("create hosted world");
    runtime
        .get_world(universe_id, accepted.world_id)
        .expect("world summary")
}

pub fn upload_counter_manifest(runtime: &HostedWorkerRuntime, universe_id: UniverseId) -> String {
    static PREPARED: OnceLock<PreparedManifest> = OnceLock::new();
    upload_prepared_manifest(
        runtime,
        universe_id,
        PREPARED.get_or_init(prepare_counter_manifest),
    )
}

pub fn upload_timer_manifest(runtime: &HostedWorkerRuntime, universe_id: UniverseId) -> String {
    static PREPARED: OnceLock<PreparedManifest> = OnceLock::new();
    upload_prepared_manifest(
        runtime,
        universe_id,
        PREPARED.get_or_init(prepare_timer_manifest),
    )
}

pub fn upload_fetch_notify_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> String {
    static PREPARED: OnceLock<PreparedManifest> = OnceLock::new();
    upload_prepared_manifest(
        runtime,
        universe_id,
        PREPARED.get_or_init(prepare_fetch_notify_manifest),
    )
}

pub fn upload_workspace_manifest(runtime: &HostedWorkerRuntime, universe_id: UniverseId) -> String {
    static PREPARED: OnceLock<PreparedManifest> = OnceLock::new();
    upload_prepared_manifest(
        runtime,
        universe_id,
        PREPARED.get_or_init(prepare_workspace_manifest),
    )
}

pub fn upload_fabric_exec_progress_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> String {
    static PREPARED: OnceLock<PreparedManifest> = OnceLock::new();
    upload_prepared_manifest(
        runtime,
        universe_id,
        PREPARED.get_or_init(prepare_fabric_exec_progress_manifest),
    )
}

pub fn upload_authored_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
    world_root: &Path,
) -> String {
    let manifest_path = world_root.join("air/manifest.air.json");
    let manifest_text = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|err| panic!("read {}: {err}", manifest_path.display()));
    if manifest_text.contains("\"demo/CounterSM@1\"") {
        upload_counter_manifest(runtime, universe_id)
    } else if manifest_text.contains("\"demo/TimerSM@1\"") {
        upload_timer_manifest(runtime, universe_id)
    } else if manifest_text.contains("\"demo/FetchNotify@1\"") {
        upload_fetch_notify_manifest(runtime, universe_id)
    } else if manifest_text.contains("\"demo/WorkspaceDemo@1\"") {
        upload_workspace_manifest(runtime, universe_id)
    } else {
        panic!(
            "unsupported authored test world root {}",
            world_root.display()
        );
    }
}

pub fn upload_manifest_for_world_root_in_domain(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
    world_root: &Path,
) -> String {
    upload_authored_manifest(runtime, universe_id, world_root)
}

pub fn seed_counter_world(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> HostedWorldSummary {
    create_counter_world(runtime, universe_id)
}

pub fn seed_timer_world(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> HostedWorldSummary {
    seed_timer_builtins(runtime, universe_id);
    let manifest_hash = upload_timer_manifest(runtime, universe_id);
    create_world_from_manifest(runtime, universe_id, manifest_hash)
}

pub fn seed_fetch_notify_world(
    _ctx: &BrokerRuntimeTestContext,
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> HostedWorldSummary {
    let manifest_hash = upload_fetch_notify_manifest(runtime, universe_id);
    create_world_from_manifest(runtime, universe_id, manifest_hash)
}

pub fn seed_workspace_world(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> HostedWorldSummary {
    let manifest_hash = upload_workspace_manifest(runtime, universe_id);
    create_world_from_manifest(runtime, universe_id, manifest_hash)
}

pub fn ensure_hosted_test_env_loaded() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = aos_node::load_dotenv_candidates();
    });
}

pub fn kafka_broker_enabled() -> bool {
    ensure_hosted_test_env_loaded();
    std::env::var("AOS_KAFKA_BOOTSTRAP_SERVERS")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

pub fn blobstore_bucket_enabled() -> bool {
    ensure_hosted_test_env_loaded();
    std::env::var("AOS_BLOBSTORE_BUCKET")
        .or_else(|_| std::env::var("AOS_S3_BUCKET"))
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

pub fn broker_kafka_config(label: &str, _partition_count: u32) -> Option<KafkaConfig> {
    ensure_hosted_test_env_loaded();
    let bootstrap_servers = std::env::var("AOS_KAFKA_BOOTSTRAP_SERVERS").ok()?;
    if bootstrap_servers.trim().is_empty() {
        return None;
    }

    let unique = format!("{label}-{}", uuid::Uuid::new_v4());
    let mut config = KafkaConfig::default();
    config.bootstrap_servers = Some(bootstrap_servers);
    config.journal_topic = format!("aos-journal-{unique}");
    config.transactional_id = format!("aos-node-tests-{unique}");
    Some(config)
}

pub async fn ensure_kafka_topics(
    config: &KafkaConfig,
    partition_count: u32,
) -> Result<(), BackendError> {
    let bootstrap_servers = config.bootstrap_servers.as_ref().ok_or_else(|| {
        BackendError::Persist(aos_node::PersistError::backend(
            "Kafka bootstrap servers are not configured".to_owned(),
        ))
    })?;
    let admin: AdminClient<_> = ClientConfig::new()
        .set("bootstrap.servers", bootstrap_servers)
        .create()
        .map_err(|err| {
            BackendError::Persist(aos_node::PersistError::backend(format!(
                "create Kafka admin client: {err}"
            )))
        })?;

    let topics = [NewTopic::new(
        &config.journal_topic,
        partition_count as i32,
        TopicReplication::Fixed(1),
    )];
    let results = admin
        .create_topics(&topics, &AdminOptions::new())
        .await
        .map_err(|err| {
            BackendError::Persist(aos_node::PersistError::backend(format!(
                "create Kafka topics: {err}"
            )))
        })?;

    for result in results {
        if let Err((topic, code)) = result
            && code != RDKafkaErrorCode::TopicAlreadyExists
        {
            return Err(BackendError::Persist(aos_node::PersistError::backend(
                format!("create Kafka topic {topic}: {code}"),
            )));
        }
    }
    Ok(())
}

pub fn broker_blobstore_config(label: &str) -> Option<BlobStoreConfig> {
    ensure_hosted_test_env_loaded();
    let bucket = std::env::var("AOS_BLOBSTORE_BUCKET")
        .or_else(|_| std::env::var("AOS_S3_BUCKET"))
        .ok()?;
    if bucket.trim().is_empty() {
        return None;
    }

    let mut config = BlobStoreConfig::default();
    config.bucket = Some(bucket);
    config.prefix = format!("aos-node-tests/{label}/{}", uuid::Uuid::new_v4());
    Some(config)
}

pub fn broker_runtime_test_context(
    label: &str,
    partition_count: u32,
) -> Option<BrokerRuntimeTestContext> {
    let partition_count = partition_count.max(1);
    let kafka_config = broker_kafka_config(label, partition_count)?;
    let blobstore_config = broker_blobstore_config(label)?;
    ensure_kafka_topics_sync(kafka_config.clone(), partition_count).ok()?;
    Some(BrokerRuntimeTestContext {
        kafka_config,
        blobstore_config,
        partition_count,
    })
}

pub fn worker_config() -> HostedWorkerConfig {
    HostedWorkerConfig {
        worker_id: format!("test-worker-{}", uuid::Uuid::new_v4()),
        checkpoint_interval: Duration::from_millis(25),
        checkpoint_every_events: Some(100),
        max_local_continuation_slices_per_flush: 64,
        max_uncommitted_slices_per_world: 256,
        owned_worlds: None,
    }
}

pub fn temp_state_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("aos-node-{label}-{}", uuid::Uuid::new_v4()))
}

pub async fn response_json<T: serde::de::DeserializeOwned>(
    response: axum::response::Response,
) -> T {
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("decode response body")
}

fn worker_kafka_config_for_label(ctx: &BrokerRuntimeTestContext, label: &str) -> KafkaConfig {
    let mut kafka_config = ctx.kafka_config.clone();
    kafka_config.transactional_id = format!("{}-worker-{label}", kafka_config.transactional_id);
    kafka_config
}

fn control_kafka_config_for_label(ctx: &BrokerRuntimeTestContext, label: &str) -> KafkaConfig {
    let mut kafka_config = ctx.kafka_config.clone();
    kafka_config.transactional_id = format!("{}-control-{label}", kafka_config.transactional_id);
    kafka_config
}

fn ensure_kafka_topics_sync(config: KafkaConfig, partition_count: u32) -> Result<(), BackendError> {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build Kafka topic setup runtime")
            .block_on(async move { ensure_kafka_topics(&config, partition_count).await })
    })
    .join()
    .unwrap_or_else(|_| {
        Err(BackendError::Persist(aos_node::PersistError::backend(
            "join Kafka topic setup thread".to_owned(),
        )))
    })
}

fn upload_prepared_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
    prepared: &PreparedManifest,
) -> String {
    for bytes in &prepared.blobs {
        runtime
            .put_blob(universe_id, bytes)
            .expect("upload manifest blob");
    }
    runtime
        .put_blob(universe_id, &prepared.manifest_bytes)
        .expect("upload manifest")
        .to_hex()
}

pub fn seed_timer_builtins(runtime: &HostedWorkerRuntime, universe_id: UniverseId) {
    let upload_builtin = |bytes: Vec<u8>, expected: Hash| {
        let uploaded = runtime
            .put_blob(universe_id, &bytes)
            .expect("upload timer builtin");
        assert_eq!(uploaded, expected);
    };

    let schema = builtins::find_builtin_schema("sys/TimerSetParams@1").expect("timer params");
    upload_builtin(
        to_canonical_cbor(&schema.schema).expect("encode timer params"),
        schema.hash,
    );
    let schema = builtins::find_builtin_schema("sys/TimerSetReceipt@1").expect("timer receipt");
    upload_builtin(
        to_canonical_cbor(&schema.schema).expect("encode timer receipt"),
        schema.hash,
    );
    let schema = builtins::find_builtin_schema("sys/TimerFired@1").expect("timer fired");
    upload_builtin(
        to_canonical_cbor(&schema.schema).expect("encode timer fired"),
        schema.hash,
    );
    let effect = builtins::find_builtin_effect("sys/timer.set@1").expect("timer effect");
    upload_builtin(
        to_canonical_cbor(&effect.effect).expect("encode timer effect"),
        effect.hash,
    );
}

fn prepare_fetch_notify_manifest() -> PreparedManifest {
    let wasm_bytes = compile_fixture_workflow(fetch_notify_world_root().join("workflow"));
    let schemas =
        load_json_file::<Vec<DefSchema>>(&fetch_notify_world_root().join("air/schemas.air.json"));
    let wasm_hash = Hash::of_bytes(&wasm_bytes);
    let modules = load_fixture_modules(
        &fetch_notify_world_root().join("air/module.air.json"),
        &wasm_hash,
    );

    let mut blobs = vec![wasm_bytes];
    let mut schema_refs = store_defs(&mut blobs, schemas.into_iter().map(AirNode::Defschema));
    schema_refs.extend([
        builtin_schema_ref("sys/HttpRequestParams@1"),
        builtin_schema_ref("sys/HttpRequestReceipt@1"),
        builtin_schema_ref("sys/EffectReceiptEnvelope@1"),
    ]);
    let module_refs = store_defs(&mut blobs, modules.into_iter().map(AirNode::Defmodule));
    let effect_refs = vec![builtin_effect_ref("sys/http.request@1")];

    let manifest = Manifest {
        air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
        schemas: schema_refs,
        modules: module_refs,
        effects: effect_refs,
        effect_bindings: Vec::new(),
        secrets: Vec::new(),
        routing: Some(Routing {
            subscriptions: vec![RoutingEvent {
                event: schema_ref("demo/FetchNotifyEvent@1"),
                module: "demo/FetchNotify@1".into(),
                key_field: None,
            }],
            inboxes: Vec::new(),
        }),
    };

    PreparedManifest {
        blobs,
        manifest_bytes: to_canonical_cbor(&manifest).expect("encode fetch notify manifest"),
    }
}

fn prepare_workspace_manifest() -> PreparedManifest {
    let wasm_bytes = compile_fixture_workflow(workspace_world_root().join("workflow"));
    let schemas =
        load_json_file::<Vec<DefSchema>>(&workspace_world_root().join("air/schemas.air.json"));
    let wasm_hash = Hash::of_bytes(&wasm_bytes);
    let modules = load_fixture_modules(
        &workspace_world_root().join("air/module.air.json"),
        &wasm_hash,
    );

    let mut blobs = vec![wasm_bytes];
    let mut schema_refs = store_defs(&mut blobs, schemas.into_iter().map(AirNode::Defschema));
    schema_refs.extend([
        builtin_schema_ref("sys/EffectReceiptEnvelope@1"),
        builtin_schema_ref("sys/WorkspaceCommitMeta@1"),
        builtin_schema_ref("sys/WorkspaceCommit@1"),
        builtin_schema_ref("sys/WorkspaceResolveParams@1"),
        builtin_schema_ref("sys/WorkspaceResolveReceipt@1"),
        builtin_schema_ref("sys/WorkspaceEmptyRootParams@1"),
        builtin_schema_ref("sys/WorkspaceEmptyRootReceipt@1"),
        builtin_schema_ref("sys/WorkspaceListParams@1"),
        builtin_schema_ref("sys/WorkspaceListEntry@1"),
        builtin_schema_ref("sys/WorkspaceListReceipt@1"),
        builtin_schema_ref("sys/WorkspaceWriteBytesParams@1"),
        builtin_schema_ref("sys/WorkspaceWriteBytesReceipt@1"),
        builtin_schema_ref("sys/WorkspaceDiffParams@1"),
        builtin_schema_ref("sys/WorkspaceDiffChange@1"),
        builtin_schema_ref("sys/WorkspaceDiffReceipt@1"),
    ]);
    let mut module_refs = store_defs(&mut blobs, modules.into_iter().map(AirNode::Defmodule));
    let (workspace_module_ref, workspace_module_blobs) = authored_builtin_module("sys/Workspace@1");
    blobs.extend(workspace_module_blobs);
    module_refs.push(workspace_module_ref);
    let effect_refs = vec![
        builtin_effect_ref("sys/workspace.resolve@1"),
        builtin_effect_ref("sys/workspace.empty_root@1"),
        builtin_effect_ref("sys/workspace.write_bytes@1"),
        builtin_effect_ref("sys/workspace.list@1"),
        builtin_effect_ref("sys/workspace.diff@1"),
    ];

    let manifest = Manifest {
        air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
        schemas: schema_refs,
        modules: module_refs,
        effects: effect_refs,
        effect_bindings: Vec::new(),
        secrets: Vec::new(),
        routing: Some(Routing {
            subscriptions: vec![
                RoutingEvent {
                    event: schema_ref("demo/WorkspaceEvent@1"),
                    module: "demo/WorkspaceDemo@1".into(),
                    key_field: None,
                },
                RoutingEvent {
                    event: schema_ref("sys/WorkspaceCommit@1"),
                    module: "sys/Workspace@1".into(),
                    key_field: Some("workspace".into()),
                },
            ],
            inboxes: Vec::new(),
        }),
    };

    PreparedManifest {
        blobs,
        manifest_bytes: to_canonical_cbor(&manifest).expect("encode workspace manifest"),
    }
}

fn prepare_fabric_exec_progress_manifest() -> PreparedManifest {
    let wasm_bytes = compile_fixture_workflow(fabric_exec_progress_world_root().join("workflow"));
    let schemas = load_json_file::<Vec<DefSchema>>(
        &fabric_exec_progress_world_root().join("air/schemas.air.json"),
    );
    let wasm_hash = Hash::of_bytes(&wasm_bytes);
    let modules = load_fixture_modules(
        &fabric_exec_progress_world_root().join("air/module.air.json"),
        &wasm_hash,
    );

    let mut blobs = vec![wasm_bytes];
    let mut schema_refs = store_defs(&mut blobs, schemas.into_iter().map(AirNode::Defschema));
    schema_refs.extend([
        builtin_schema_ref("sys/EffectReceiptEnvelope@1"),
        builtin_schema_ref("sys/EffectStreamFrame@1"),
        builtin_schema_ref("sys/HostExecParams@1"),
        builtin_schema_ref("sys/HostExecReceipt@1"),
        builtin_schema_ref("sys/HostExecProgressFrame@1"),
    ]);
    let module_refs = store_defs(&mut blobs, modules.into_iter().map(AirNode::Defmodule));
    let manifest = Manifest {
        air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
        schemas: schema_refs,
        modules: module_refs,
        effects: vec![builtin_effect_ref("sys/host.exec@1")],
        effect_bindings: vec![EffectBinding {
            kind: aos_air_types::EffectKind::host_exec(),
            adapter_id: "host.exec.fabric".to_string(),
        }],
        secrets: Vec::new(),
        routing: Some(Routing {
            subscriptions: vec![RoutingEvent {
                event: schema_ref("demo/FabricExecProgressEvent@1"),
                module: "demo/FabricExecProgress@1".into(),
                key_field: None,
            }],
            inboxes: Vec::new(),
        }),
    };

    PreparedManifest {
        blobs,
        manifest_bytes: to_canonical_cbor(&manifest).expect("encode fabric exec manifest"),
    }
}

fn prepare_counter_manifest() -> PreparedManifest {
    let wasm_bytes = compile_fixture_workflow(counter_world_root().join("workflow"));
    let schemas =
        load_json_file::<Vec<DefSchema>>(&counter_world_root().join("air/schemas.air.json"));
    let wasm_hash = Hash::of_bytes(&wasm_bytes);
    let modules = load_fixture_modules(
        &counter_world_root().join("air/module.air.json"),
        &wasm_hash,
    );

    let mut blobs = vec![wasm_bytes];
    let schema_refs = store_defs(&mut blobs, schemas.into_iter().map(AirNode::Defschema));
    let module_refs = store_defs(&mut blobs, modules.into_iter().map(AirNode::Defmodule));

    let manifest = Manifest {
        air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
        schemas: schema_refs,
        modules: module_refs,
        effects: Vec::new(),
        effect_bindings: Vec::new(),
        secrets: Vec::new(),
        routing: Some(Routing {
            subscriptions: vec![RoutingEvent {
                event: schema_ref("demo/CounterEvent@1"),
                module: "demo/CounterSM@1".into(),
                key_field: None,
            }],
            inboxes: Vec::new(),
        }),
    };

    PreparedManifest {
        blobs,
        manifest_bytes: to_canonical_cbor(&manifest).expect("encode counter manifest"),
    }
}

fn prepare_timer_manifest() -> PreparedManifest {
    let wasm_bytes = compile_fixture_workflow(timer_world_root().join("workflow"));
    let schemas =
        load_json_file::<Vec<DefSchema>>(&timer_world_root().join("air/schemas.air.json"));
    let wasm_hash = Hash::of_bytes(&wasm_bytes);
    let modules = load_fixture_modules(&timer_world_root().join("air/module.air.json"), &wasm_hash);

    let mut blobs = vec![wasm_bytes];
    let mut schema_refs = store_defs(&mut blobs, schemas.into_iter().map(AirNode::Defschema));
    schema_refs.extend([
        builtin_schema_ref("sys/TimerSetParams@1"),
        builtin_schema_ref("sys/TimerSetReceipt@1"),
        builtin_schema_ref("sys/TimerFired@1"),
    ]);
    let module_refs = store_defs(&mut blobs, modules.into_iter().map(AirNode::Defmodule));

    let manifest = Manifest {
        air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
        schemas: schema_refs,
        modules: module_refs,
        effects: vec![builtin_effect_ref("sys/timer.set@1")],
        effect_bindings: Vec::new(),
        secrets: Vec::new(),
        routing: Some(Routing {
            subscriptions: vec![RoutingEvent {
                event: schema_ref("demo/TimerEvent@1"),
                module: "demo/TimerSM@1".into(),
                key_field: None,
            }],
            inboxes: Vec::new(),
        }),
    };

    PreparedManifest {
        blobs,
        manifest_bytes: to_canonical_cbor(&manifest).expect("encode timer manifest"),
    }
}

fn store_defs(blobs: &mut Vec<Vec<u8>>, defs: impl IntoIterator<Item = AirNode>) -> Vec<NamedRef> {
    let mut refs = Vec::new();
    for node in defs {
        let name = air_node_name(&node);
        let bytes = to_canonical_cbor(&node).expect("encode AIR node");
        let hash = Hash::of_bytes(&bytes);
        blobs.push(bytes);
        refs.push(NamedRef {
            name,
            hash: HashRef::new(hash.to_hex()).expect("AIR node hash ref"),
        });
    }
    refs
}

fn air_node_name(node: &AirNode) -> String {
    match node {
        AirNode::Defschema(schema) => schema.name.clone(),
        AirNode::Defmodule(module) => module.name.clone(),
        AirNode::Defeffect(effect) => effect.name.clone(),
        AirNode::Defsecret(secret) => secret.name.clone(),
        AirNode::Manifest(_) => panic!("manifest is not stored as a named AIR node in tests"),
    }
}

fn builtin_schema_ref(name: &str) -> NamedRef {
    let builtin = builtins::find_builtin_schema(name).expect("builtin schema");
    NamedRef {
        name: builtin.schema.name.clone(),
        hash: builtin.hash_ref.clone(),
    }
}

fn builtin_effect_ref(name: &str) -> NamedRef {
    let builtin = builtins::find_builtin_effect(name).expect("builtin effect");
    NamedRef {
        name: builtin.effect.name.clone(),
        hash: builtin.hash_ref.clone(),
    }
}

fn schema_ref(name: &str) -> SchemaRef {
    SchemaRef::new(name).expect("schema ref")
}

fn load_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let bytes = fs::read(path).expect("read fixture json");
    serde_json::from_slice(&bytes).expect("parse fixture json")
}

fn load_fixture_modules(path: &Path, wasm_hash: &Hash) -> Vec<DefModule> {
    let mut values = load_json_file::<Vec<serde_json::Value>>(path);
    let wasm_hash = wasm_hash.to_hex();
    for value in &mut values {
        value["wasm_hash"] = serde_json::Value::String(wasm_hash.clone());
    }
    serde_json::from_value(serde_json::Value::Array(values)).expect("parse fixture modules")
}

fn compile_fixture_workflow(workflow_dir: PathBuf) -> Vec<u8> {
    let mut request = BuildRequest::new(
        workflow_dir
            .to_str()
            .expect("workflow dir should be valid utf-8"),
    );
    request.config.release = false;
    Builder::compile(request)
        .expect("compile fixture workflow")
        .wasm_bytes
}

pub fn counter_world_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| authored_smoke_world_root("00-counter", "aos-node-counter-tests"))
        .clone()
}

pub fn timer_world_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| authored_smoke_world_root("01-hello-timer", "aos-node-hello-timer-tests"))
        .clone()
}

pub fn fetch_notify_world_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| authored_smoke_world_root("03-fetch-notify", "aos-node-fetch-notify-tests"))
        .clone()
}

pub fn workspace_world_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| authored_smoke_world_root("09-workspaces", "aos-node-workspace-tests"))
        .clone()
}

pub fn fabric_exec_progress_world_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        authored_smoke_world_root("13-fabric-exec-progress", "aos-node-fabric-exec-tests")
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

fn authored_smoke_world_root(fixture_name: &str, temp_prefix: &str) -> PathBuf {
    let src = smoke_fixture_root(fixture_name);
    let signature = fixture_copy_signature(&src);
    let dst = std::env::temp_dir().join(format!("{temp_prefix}-{signature}"));
    if fixture_copy_is_ready(&dst) {
        patch_fixture_workflow_manifest(&dst);
        return dst;
    }
    if dst.exists() {
        let _ = fs::remove_dir_all(&dst);
    }

    copy_fixture_dir(&src, &dst);
    let aos_state = dst.join(".aos");
    if aos_state.exists() {
        let _ = fs::remove_dir_all(&aos_state);
    }
    patch_fixture_workflow_manifest(&dst);
    dst
}

fn fixture_copy_is_ready(dst: &Path) -> bool {
    [
        dst.join("air/manifest.air.json"),
        dst.join("air/module.air.json"),
        dst.join("workflow/Cargo.toml"),
        dst.join("workflow/src/lib.rs"),
    ]
    .into_iter()
    .all(|path| path.exists())
}

fn patch_fixture_workflow_manifest(dst: &Path) {
    let crates_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("workspace crates root");
    let cargo_toml = dst.join("workflow/Cargo.toml");
    let cargo_text = fs::read_to_string(&cargo_toml).expect("read copied workflow Cargo.toml");
    let cargo_text = cargo_text.replace("../../../../", &format!("{}/", crates_root.display()));
    fs::write(cargo_toml, cargo_text).expect("patch copied workflow Cargo.toml");
}

fn authored_builtin_module(name: &str) -> (NamedRef, Vec<Vec<u8>>) {
    let builtin = builtins::find_builtin_module(name).expect("builtin module");
    let mut module = builtin.module.clone();
    let wasm_bytes = builtin_module_wasm_bytes(name);
    let wasm_hash = Hash::of_bytes(&wasm_bytes).to_hex();
    module.wasm_hash = HashRef::new(wasm_hash).expect("builtin module wasm hash ref");
    let module_bytes =
        to_canonical_cbor(&AirNode::Defmodule(module.clone())).expect("encode builtin module");
    let module_hash = Hash::of_bytes(&module_bytes);
    (
        NamedRef {
            name: module.name,
            hash: HashRef::new(module_hash.to_hex()).expect("builtin module hash ref"),
        },
        vec![wasm_bytes, module_bytes],
    )
}

fn builtin_module_wasm_bytes(name: &str) -> Vec<u8> {
    let bin = match name {
        "sys/Workspace@1" => "workspace",
        other => panic!("unsupported builtin test module {other}"),
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
            .expect("build builtin test module");
        assert!(
            status.success(),
            "building builtin test module {bin} failed with {status}"
        );
    }
    fs::read(&wasm_path)
        .unwrap_or_else(|err| panic!("read builtin module wasm {}: {err}", wasm_path.display()))
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

fn fixture_copy_signature(src: &Path) -> String {
    let mut bytes = Vec::new();
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
    dir_is_root_hidden(root, path, name)
}

fn dir_is_root_hidden(root: &Path, path: &Path, name: &std::ffi::OsStr) -> bool {
    path.parent() == Some(root) && matches!(name.to_str(), Some(".git" | ".aos" | "target"))
}
