#![allow(dead_code)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Once, OnceLock};
use std::time::{Duration, Instant};

use aos_air_types::{AirNode, Manifest, builtins};
use aos_authoring::bundle::import_genesis;
use aos_authoring::{WorkflowBuildProfile, build_bundle_from_local_world_with_profile};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_kernel::{ManifestLoader, Store};
use aos_node::{BlobPlane, CreateWorldRequest, CreateWorldSource, PlaneError, UniverseId};
use aos_node_hosted::blobstore::{
    BlobStoreConfig, HostedBlobMetaStore, RemoteCasStore, scoped_blobstore_config,
};
use aos_node_hosted::config::HostedWorkerConfig;
use aos_node_hosted::kafka::{HostedKafkaBackend, KafkaConfig};
use aos_node_hosted::worker::HostedWorkerRuntime;
use aos_node_hosted::{HostedWorker, HostedWorldSummary, WorkerSupervisor};
use axum::body::to_bytes;
use axum::http::StatusCode;
use serde::de::DeserializeOwned;
use serde_json::json;

const TEST_WAIT_DEADLINE: Duration = Duration::from_secs(10);
const TEST_WAIT_SLEEP: Duration = Duration::from_millis(5);

#[derive(Clone)]
struct PreparedAuthoredManifest {
    blobs: Vec<Vec<u8>>,
    manifest_bytes: Vec<u8>,
}

pub(crate) fn smoke_fixture_root(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../aos-smoke/fixtures")
        .join(name)
        .canonicalize()
        .expect("smoke fixture path")
}

pub(crate) fn authored_smoke_world_root(
    fixture_name: &str,
    temp_prefix: &str,
    workflow_module: &str,
) -> PathBuf {
    let src = smoke_fixture_root(fixture_name);
    let signature = fixture_copy_signature(&src, workflow_module);
    let dst = std::env::temp_dir().join(format!(
        "{temp_prefix}-{signature}-pid{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
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
            "air": { "dir": "air" },
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

fn fixture_copy_signature(src: &Path, workflow_module: &str) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(src.to_string_lossy().as_bytes());
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

pub(crate) fn counter_world_root() -> PathBuf {
    authored_smoke_world_root("00-counter", "aos-node-hosted-counter", "demo/CounterSM@1")
}

pub(crate) fn fetch_notify_world_root() -> PathBuf {
    authored_smoke_world_root(
        "03-fetch-notify",
        "aos-node-hosted-fetch-notify",
        "demo/FetchNotify@1",
    )
}

pub(crate) fn timer_world_root() -> PathBuf {
    authored_smoke_world_root("01-hello-timer", "aos-node-hosted-timer", "demo/TimerSM@1")
}

pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) {
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

pub(crate) fn worker_config() -> HostedWorkerConfig {
    HostedWorkerConfig {
        worker_id: "test-worker".into(),
        partition_count: 1,
        supervisor_poll_interval: Duration::from_millis(1),
        checkpoint_interval: Duration::from_secs(3600),
        checkpoint_every_events: None,
        checkpoint_on_create: false,
    }
}

pub(crate) fn temp_state_root(label: &str) -> PathBuf {
    ensure_shared_module_cache_env();
    std::env::temp_dir().join(format!("aos-node-hosted-{label}-{}", uuid::Uuid::new_v4()))
}

pub(crate) fn embedded_runtime(partitions: u32) -> HostedWorkerRuntime {
    HostedWorkerRuntime::new_embedded_with_state_root(partitions, temp_state_root("embedded"))
        .unwrap()
}

pub(crate) fn broker_runtime(partitions: u32) -> HostedWorkerRuntime {
    HostedWorkerRuntime::new_with_state_root(partitions, temp_state_root("broker")).unwrap()
}

pub(crate) fn hosted_universe_id(runtime: &HostedWorkerRuntime) -> UniverseId {
    runtime
        .default_universe_id()
        .expect("hosted default universe")
}

#[derive(Clone)]
pub(crate) struct BrokerRuntimeTestContext {
    pub kafka_config: KafkaConfig,
    pub blobstore_config: BlobStoreConfig,
    partition_count: u32,
}

impl BrokerRuntimeTestContext {
    fn runtime_with_kafka(&self, label: &str, kafka_config: KafkaConfig) -> HostedWorkerRuntime {
        self.runtime_with_kafka_and_universe(label, kafka_config, aos_node::local_universe_id())
    }

    fn runtime_with_kafka_and_universe(
        &self,
        label: &str,
        kafka_config: KafkaConfig,
        universe_id: UniverseId,
    ) -> HostedWorkerRuntime {
        HostedWorkerRuntime::new_broker_with_state_root_and_universe(
            self.partition_count,
            temp_state_root(&format!("broker-{label}")),
            universe_id,
            kafka_config,
            self.blobstore_config.clone(),
        )
        .unwrap()
    }

    pub(crate) fn worker_runtime(&self, label: &str) -> HostedWorkerRuntime {
        self.runtime_with_kafka(label, self.kafka_config.clone())
    }

    pub(crate) fn worker_runtime_in_universe(
        &self,
        label: &str,
        universe_id: UniverseId,
    ) -> HostedWorkerRuntime {
        self.runtime_with_kafka_and_universe(label, self.kafka_config.clone(), universe_id)
    }

    pub(crate) fn direct_worker_runtime(
        &self,
        label: &str,
        partitions: &[u32],
    ) -> HostedWorkerRuntime {
        self.direct_worker_runtime_in_universe(label, partitions, aos_node::local_universe_id())
    }

    pub(crate) fn direct_worker_runtime_in_universe(
        &self,
        label: &str,
        partitions: &[u32],
        universe_id: UniverseId,
    ) -> HostedWorkerRuntime {
        let mut kafka_config = self.kafka_config.clone();
        kafka_config.direct_assigned_partitions = partitions.iter().copied().collect();
        kafka_config.direct_assignment_start_from_end = true;
        kafka_config.submission_group_prefix =
            format!("{}-direct-{label}", kafka_config.submission_group_prefix);
        kafka_config.transactional_id = format!("{}-direct-{label}", kafka_config.transactional_id);
        self.runtime_with_kafka_and_universe(label, kafka_config, universe_id)
    }

    pub(crate) fn control_runtime(&self, label: &str) -> HostedWorkerRuntime {
        let mut kafka_config = self.kafka_config.clone();
        kafka_config.submission_group_prefix =
            format!("{}-control-{label}", kafka_config.submission_group_prefix);
        kafka_config.transactional_id =
            format!("{}-control-{label}", kafka_config.transactional_id);
        self.runtime_with_kafka(label, kafka_config)
    }

    pub(crate) fn control_runtime_in_universe(
        &self,
        label: &str,
        universe_id: UniverseId,
    ) -> HostedWorkerRuntime {
        let mut kafka_config = self.kafka_config.clone();
        kafka_config.submission_group_prefix =
            format!("{}-control-{label}", kafka_config.submission_group_prefix);
        kafka_config.transactional_id =
            format!("{}-control-{label}", kafka_config.transactional_id);
        self.runtime_with_kafka_and_universe(label, kafka_config, universe_id)
    }

    pub(crate) fn blob_meta(&self) -> HostedBlobMetaStore {
        self.blob_meta_for_universe(UniverseId::nil())
    }

    pub(crate) fn blob_meta_for_universe(&self, universe_id: UniverseId) -> HostedBlobMetaStore {
        HostedBlobMetaStore::new(scoped_blobstore_config(&self.blobstore_config, universe_id))
            .unwrap()
    }

    pub(crate) fn remote_cas(&self) -> RemoteCasStore {
        self.remote_cas_for_universe(UniverseId::nil())
    }

    pub(crate) fn remote_cas_for_universe(&self, universe_id: UniverseId) -> RemoteCasStore {
        RemoteCasStore::new(scoped_blobstore_config(&self.blobstore_config, universe_id)).unwrap()
    }
}

pub(crate) fn broker_runtime_test_context(
    label: &str,
    partition_count: u32,
) -> Option<BrokerRuntimeTestContext> {
    let mut blobstore_config = broker_blobstore_config(label)?;
    // Worker-flow broker tests rely on multiple runtimes reopening the same authored world.
    // Force direct writes so small AIR/module blobs are readable without per-runtime pack metadata.
    blobstore_config.pack_threshold_bytes = 0;
    Some(BrokerRuntimeTestContext {
        kafka_config: broker_kafka_config(label, partition_count)?,
        blobstore_config,
        partition_count,
    })
}

pub(crate) fn broker_blobstore_config(label: &str) -> Option<BlobStoreConfig> {
    ensure_hosted_test_env_loaded();
    let mut config = BlobStoreConfig::default();
    let bucket = config.bucket.clone()?;
    if bucket.trim().is_empty() {
        return None;
    }
    let suffix = format!("{label}-{}", uuid::Uuid::new_v4());
    config.prefix = match config.prefix.trim_matches('/') {
        "" => suffix,
        prefix => format!("{prefix}/{suffix}"),
    };
    Some(config)
}

pub(crate) fn broker_kafka_config(label: &str, partition_count: u32) -> Option<KafkaConfig> {
    ensure_hosted_test_env_loaded();
    let mut config = KafkaConfig::default();
    let bootstrap = config.bootstrap_servers.clone()?;
    if bootstrap.trim().is_empty() {
        return None;
    }
    let suffix = format!("{label}-{}", uuid::Uuid::new_v4());
    let ingress_topic = format!("aos-ingress-{suffix}");
    let journal_topic = format!("aos-journal-{suffix}");
    config.ingress_topic = ingress_topic;
    config.journal_topic = journal_topic;
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
    if ensure_kafka_topics(&config, partition_count).is_err() {
        return None;
    }
    Some(config)
}

pub(crate) fn upload_authored_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
    world_root: &Path,
) -> String {
    let prepared = prepare_authored_manifest(world_root);
    upload_prepared_manifest(runtime, universe_id, &prepared)
}

fn upload_prepared_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
    prepared: &PreparedAuthoredManifest,
) -> String {
    for bytes in &prepared.blobs {
        runtime.put_blob(universe_id, bytes).unwrap();
    }
    runtime
        .put_blob(universe_id, &prepared.manifest_bytes)
        .unwrap()
        .to_hex()
}

fn upload_prepared_manifest_for_domain(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
    prepared: &PreparedAuthoredManifest,
) -> String {
    for bytes in &prepared.blobs {
        runtime.put_blob(universe_id, bytes).unwrap();
    }
    runtime
        .put_blob(universe_id, &prepared.manifest_bytes)
        .unwrap()
        .to_hex()
}

fn prepare_authored_manifest(world_root: &Path) -> PreparedAuthoredManifest {
    let (store, bundle, _) = build_bundle_from_local_world_with_profile(
        &world_root,
        false,
        WorkflowBuildProfile::Release,
    )
    .unwrap();
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
    let imported = import_genesis(&store, &bundle).unwrap();
    let loaded = ManifestLoader::load_from_bytes(&store, &imported.manifest_bytes).unwrap();

    let manifest: Manifest = serde_cbor::from_slice(&imported.manifest_bytes).unwrap();

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
            assert_eq!(Hash::of_bytes(&bytes), hash);
            push_blob(bytes);
        } else if let Some(builtin) = builtins::find_builtin_module(named.name.as_str()) {
            push_blob(to_canonical_cbor(&builtin.module).unwrap());
        } else {
            panic!("missing module ref {}", named.name);
        }
    }
    let manifest_value: serde_cbor::Value =
        serde_cbor::from_slice(&imported.manifest_bytes).unwrap();
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

pub(crate) fn upload_counter_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> String {
    static PREPARED: OnceLock<PreparedAuthoredManifest> = OnceLock::new();
    let prepared = PREPARED.get_or_init(|| prepare_authored_manifest(&counter_world_root()));
    upload_prepared_manifest(runtime, universe_id, prepared)
}

pub(crate) fn upload_manifest_for_world_root(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
    world_root: &Path,
) -> String {
    let prepared = prepare_authored_manifest(world_root);
    upload_prepared_manifest(runtime, universe_id, &prepared)
}

pub(crate) fn upload_manifest_for_world_root_in_domain(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
    world_root: &Path,
) -> String {
    let prepared = prepare_authored_manifest(world_root);
    upload_prepared_manifest_for_domain(runtime, universe_id, &prepared)
}

pub(crate) fn upload_fetch_notify_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> String {
    static PREPARED: OnceLock<PreparedAuthoredManifest> = OnceLock::new();
    let prepared = PREPARED.get_or_init(|| prepare_authored_manifest(&fetch_notify_world_root()));
    upload_prepared_manifest(runtime, universe_id, prepared)
}

pub(crate) fn upload_timer_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> String {
    static PREPARED: OnceLock<PreparedAuthoredManifest> = OnceLock::new();
    let prepared = PREPARED.get_or_init(|| prepare_authored_manifest(&timer_world_root()));
    upload_prepared_manifest(runtime, universe_id, prepared)
}

pub(crate) fn seed_http_builtins(ctx: &BrokerRuntimeTestContext, universe_id: UniverseId) {
    let blobstore = ctx.remote_cas();
    let http_request_params = builtins::find_builtin_schema("sys/HttpRequestParams@1").unwrap();
    let uploaded = blobstore
        .put_blob(
            universe_id,
            &to_canonical_cbor(&http_request_params.schema).unwrap(),
        )
        .unwrap();
    assert_eq!(uploaded, http_request_params.hash);

    let http_request_receipt = builtins::find_builtin_schema("sys/HttpRequestReceipt@1").unwrap();
    let uploaded = blobstore
        .put_blob(
            universe_id,
            &to_canonical_cbor(&http_request_receipt.schema).unwrap(),
        )
        .unwrap();
    assert_eq!(uploaded, http_request_receipt.hash);

    let http_request_effect = builtins::find_builtin_effect("sys/http.request@1").unwrap();
    let uploaded = blobstore
        .put_blob(
            universe_id,
            &to_canonical_cbor(&http_request_effect.effect).unwrap(),
        )
        .unwrap();
    assert_eq!(uploaded, http_request_effect.hash);

    let http_out_cap = builtins::find_builtin_cap("sys/http.out@1").unwrap();
    let uploaded = blobstore
        .put_blob(universe_id, &to_canonical_cbor(&http_out_cap.cap).unwrap())
        .unwrap();
    assert_eq!(uploaded, http_out_cap.hash);

    let cap_enforcer = builtins::find_builtin_module("sys/CapEnforceHttpOut@1").unwrap();
    let uploaded = blobstore
        .put_blob(
            universe_id,
            &to_canonical_cbor(&cap_enforcer.module).unwrap(),
        )
        .unwrap();
    assert_eq!(uploaded, cap_enforcer.hash);
}

pub(crate) fn seed_timer_builtins(runtime: &HostedWorkerRuntime, universe_id: UniverseId) {
    let timer_set_params = builtins::find_builtin_schema("sys/TimerSetParams@1").unwrap();
    let uploaded = runtime
        .put_blob(
            universe_id,
            &to_canonical_cbor(&timer_set_params.schema).unwrap(),
        )
        .unwrap();
    assert_eq!(uploaded, timer_set_params.hash);

    let timer_set_receipt = builtins::find_builtin_schema("sys/TimerSetReceipt@1").unwrap();
    let uploaded = runtime
        .put_blob(
            universe_id,
            &to_canonical_cbor(&timer_set_receipt.schema).unwrap(),
        )
        .unwrap();
    assert_eq!(uploaded, timer_set_receipt.hash);

    let timer_fired = builtins::find_builtin_schema("sys/TimerFired@1").unwrap();
    let uploaded = runtime
        .put_blob(
            universe_id,
            &to_canonical_cbor(&timer_fired.schema).unwrap(),
        )
        .unwrap();
    assert_eq!(uploaded, timer_fired.hash);

    let timer_effect = builtins::find_builtin_effect("sys/timer.set@1").unwrap();
    let uploaded = runtime
        .put_blob(
            universe_id,
            &to_canonical_cbor(&timer_effect.effect).unwrap(),
        )
        .unwrap();
    assert_eq!(uploaded, timer_effect.hash);

    let timer_cap = builtins::find_builtin_cap("sys/timer@1").unwrap();
    let uploaded = runtime
        .put_blob(universe_id, &to_canonical_cbor(&timer_cap.cap).unwrap())
        .unwrap();
    assert_eq!(uploaded, timer_cap.hash);
}

pub(crate) async fn create_counter_world(
    runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
    universe_id: UniverseId,
) -> HostedWorldSummary {
    let manifest_hash = upload_counter_manifest(runtime, universe_id);
    create_world_from_manifest(runtime, supervisor, universe_id, manifest_hash).await
}

pub(crate) fn seed_counter_world(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> HostedWorldSummary {
    let manifest_hash = upload_counter_manifest(runtime, universe_id);
    seed_world_from_manifest(runtime, universe_id, manifest_hash, false)
}

pub(crate) fn seed_fetch_notify_world(
    ctx: &BrokerRuntimeTestContext,
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> HostedWorldSummary {
    let manifest_hash = upload_fetch_notify_manifest(runtime, universe_id);
    seed_http_builtins(ctx, universe_id);
    seed_world_from_manifest(runtime, universe_id, manifest_hash, false)
}

pub(crate) fn seed_timer_world(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
) -> HostedWorldSummary {
    let manifest_hash = upload_timer_manifest(runtime, universe_id);
    seed_timer_builtins(runtime, universe_id);
    seed_world_from_manifest(runtime, universe_id, manifest_hash, false)
}

pub(crate) async fn create_world_from_manifest(
    runtime: &HostedWorkerRuntime,
    supervisor: &mut WorkerSupervisor,
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
        .unwrap();
    let deadline = std::time::Instant::now() + TEST_WAIT_DEADLINE;
    while std::time::Instant::now() < deadline {
        supervisor.run_once().await.unwrap();
        if let Ok(world) = runtime.get_world(universe_id, accepted.world_id) {
            return world;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    runtime.get_world(universe_id, accepted.world_id).unwrap()
}

pub(crate) fn seed_world_from_manifest(
    runtime: &HostedWorkerRuntime,
    universe_id: UniverseId,
    manifest_hash: String,
    publish_checkpoint: bool,
) -> HostedWorldSummary {
    runtime
        .seed_world(
            universe_id,
            CreateWorldRequest {
                world_id: None,
                universe_id,
                created_at_ns: 1,
                source: CreateWorldSource::Manifest { manifest_hash },
            },
            publish_checkpoint,
        )
        .unwrap()
}

pub(crate) fn kafka_broker_enabled() -> bool {
    ensure_hosted_test_env_loaded();
    std::env::var("AOS_KAFKA_BOOTSTRAP_SERVERS")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

pub(crate) fn blobstore_bucket_enabled() -> bool {
    ensure_hosted_test_env_loaded();
    std::env::var("AOS_BLOBSTORE_BUCKET")
        .ok()
        .or_else(|| std::env::var("AOS_S3_BUCKET").ok())
        .is_some_and(|value| !value.trim().is_empty())
}

pub(crate) fn ensure_hosted_test_env_loaded() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        aos_node_hosted::load_dotenv_candidates().expect("load hosted test .env");
        let test_ns = format!("aos-node-hosted-test-{}", uuid::Uuid::new_v4());
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
        let dir = std::env::temp_dir().join("aos-node-hosted-test-module-cache");
        std::fs::create_dir_all(&dir).expect("create hosted test module cache dir");
        dir
    });
    if std::env::var_os("AOS_MODULE_CACHE_DIR").is_none() {
        unsafe {
            std::env::set_var("AOS_MODULE_CACHE_DIR", cache_dir);
        }
    }
}

pub(crate) async fn response_json<T: DeserializeOwned>(response: axum::response::Response) -> T {
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("decode json body")
}

pub(crate) fn hosted_worker() -> HostedWorker {
    HostedWorker::new(worker_config())
}

pub(crate) async fn wait_for_kafka_assignment(
    kafka: &mut HostedKafkaBackend,
) -> Result<Vec<u32>, PlaneError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        kafka.sync_assignments_and_poll()?;
        let assigned = kafka.assigned_partitions();
        if !assigned.is_empty() {
            return Ok(assigned);
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    Ok(kafka.assigned_partitions())
}

pub(crate) async fn wait_for_kafka_pending_submissions(
    kafka: &mut HostedKafkaBackend,
    min_pending: usize,
) -> Result<(), PlaneError> {
    let deadline = Instant::now() + TEST_WAIT_DEADLINE;
    while Instant::now() < deadline {
        kafka.sync_assignments_and_poll()?;
        if kafka.pending_submission_count() >= min_pending {
            return Ok(());
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    Ok(())
}

fn ensure_kafka_topics(config: &KafkaConfig, partition_count: u32) -> Result<(), String> {
    create_kafka_topic(&config.ingress_topic, partition_count, false)?;
    create_kafka_topic(&config.journal_topic, partition_count, false)?;
    create_kafka_topic(&config.projection_topic, partition_count, true)?;
    Ok(())
}

fn create_kafka_topic(topic: &str, partitions: u32, compacted: bool) -> Result<(), String> {
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
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last_error = String::new();
    while Instant::now() < deadline {
        let output = Command::new("docker")
            .args(&args)
            .output()
            .map_err(|err| format!("create Kafka topic {topic}: {err}"))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let combined = [stderr.as_str(), stdout.as_str()]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" | ");
        if combined.contains("TOPIC_ALREADY_EXISTS")
            || combined.contains("already exists")
            || kafka_topic_exists(topic)?
        {
            return Ok(());
        }
        last_error = combined;
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!("create Kafka topic {topic} failed: {last_error}"))
}

fn kafka_topic_exists(topic: &str) -> Result<bool, String> {
    let output = Command::new("docker")
        .args(["exec", "aos-redpanda", "rpk", "topic", "describe", topic])
        .output()
        .map_err(|err| format!("describe Kafka topic {topic}: {err}"))?;
    Ok(output.status.success())
}
