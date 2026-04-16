use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, anyhow};

use crate::UniverseId;
use crate::blobstore::BlobStoreConfig;
use crate::bootstrap::{
    ControlDeps, build_control_deps_from_worker_runtime, build_worker_runtime_kafka,
    build_worker_runtime_sqlite,
};
use crate::config::HostedWorkerConfig;
use crate::control::{
    ControlFacade, ControlHttpConfig, serve as serve_control_http,
    serve_with_ready as serve_control_http_with_ready,
};
use crate::kafka::KafkaConfig;
use crate::worker::{HostedWorker, HostedWorkerRuntime, WorkerError};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum NodeRole {
    Worker,
    Control,
    All,
}

#[derive(Clone, Debug)]
pub enum NodeJournalBackend {
    Kafka {
        partition_count: u32,
        kafka_config: KafkaConfig,
        blobstore_config: BlobStoreConfig,
    },
    Sqlite {
        blobstore_config: BlobStoreConfig,
    },
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum NodeJournalBackendKind {
    Kafka,
    Sqlite,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum NodeBlobBackendKind {
    Local,
    ObjectStore,
}

#[derive(Clone, Debug)]
pub struct NodeConfig {
    pub role: NodeRole,
    pub state_root: PathBuf,
    pub default_universe_id: UniverseId,
    pub journal: NodeJournalBackend,
    pub worker: HostedWorkerConfig,
    pub control: ControlHttpConfig,
}

impl NodeConfig {
    pub fn journal_backend_kind(&self) -> NodeJournalBackendKind {
        self.journal.kind()
    }

    pub fn blobstore_config(&self) -> &BlobStoreConfig {
        self.journal.blobstore_config()
    }

    pub fn partition_count(&self) -> Option<u32> {
        match &self.journal {
            NodeJournalBackend::Kafka {
                partition_count, ..
            } => Some(*partition_count),
            NodeJournalBackend::Sqlite { .. } => None,
        }
    }
}

impl NodeJournalBackend {
    pub fn kind(&self) -> NodeJournalBackendKind {
        match self {
            NodeJournalBackend::Kafka { .. } => NodeJournalBackendKind::Kafka,
            NodeJournalBackend::Sqlite { .. } => NodeJournalBackendKind::Sqlite,
        }
    }

    pub fn blobstore_config(&self) -> &BlobStoreConfig {
        match self {
            NodeJournalBackend::Kafka {
                blobstore_config, ..
            }
            | NodeJournalBackend::Sqlite {
                blobstore_config, ..
            } => blobstore_config,
        }
    }
}

pub fn default_node_state_root() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".aos-node")
}

pub fn kafka_journal_backend_from_env(
    partition_count: u32,
    blob_backend: NodeBlobBackendKind,
) -> anyhow::Result<NodeJournalBackend> {
    if blob_backend != NodeBlobBackendKind::ObjectStore {
        return Err(anyhow!(
            "Kafka journal mode requires --blob-backend object-store"
        ));
    }
    let kafka_config = KafkaConfig::default();
    require_kafka_journal_config(&kafka_config)?;
    let blobstore_config = blobstore_config_from_env(blob_backend)?;
    Ok(NodeJournalBackend::Kafka {
        partition_count,
        kafka_config,
        blobstore_config,
    })
}

pub fn sqlite_journal_backend_from_env(
    blob_backend: NodeBlobBackendKind,
) -> anyhow::Result<NodeJournalBackend> {
    Ok(NodeJournalBackend::Sqlite {
        blobstore_config: blobstore_config_from_env(blob_backend)?,
    })
}

pub fn blobstore_config_from_env(
    blob_backend: NodeBlobBackendKind,
) -> anyhow::Result<BlobStoreConfig> {
    let mut config = BlobStoreConfig::default();
    match blob_backend {
        NodeBlobBackendKind::Local => {
            config.bucket = None;
            config.endpoint = None;
            config.region = None;
            Ok(config)
        }
        NodeBlobBackendKind::ObjectStore => {
            require_blobstore_config(&config)?;
            Ok(config)
        }
    }
}

pub fn require_kafka_journal_config(config: &KafkaConfig) -> anyhow::Result<()> {
    let bootstrap_servers = config
        .bootstrap_servers
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if bootstrap_servers.is_none() {
        return Err(anyhow!(
            "AOS_KAFKA_BOOTSTRAP_SERVERS must be set for Kafka journal mode; embedded Kafka is not supported in the node binary"
        ));
    }
    Ok(())
}

pub fn require_blobstore_config(config: &BlobStoreConfig) -> anyhow::Result<()> {
    let bucket = config
        .bucket
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if bucket.is_none() {
        return Err(anyhow!(
            "AOS_BLOBSTORE_BUCKET or AOS_S3_BUCKET must be set for Kafka journal mode"
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
    Ok(())
}

pub fn build_node_runtime(config: &NodeConfig) -> Result<HostedWorkerRuntime, WorkerError> {
    match &config.journal {
        NodeJournalBackend::Kafka {
            partition_count,
            kafka_config,
            blobstore_config,
        } => build_worker_runtime_kafka(
            *partition_count,
            &config.state_root,
            config.default_universe_id,
            kafka_config.clone(),
            blobstore_config.clone(),
        ),
        NodeJournalBackend::Sqlite { blobstore_config } => build_worker_runtime_sqlite(
            &config.state_root,
            config.default_universe_id,
            blobstore_config.clone(),
        ),
    }
}

pub async fn serve_node(config: NodeConfig) -> anyhow::Result<()> {
    let worker_runtime = build_node_runtime(&config).map_err(anyhow::Error::from)?;
    log_node_init(&config);
    match config.role {
        NodeRole::Worker => serve_worker(worker_runtime, config.worker)
            .await
            .map_err(anyhow::Error::from),
        NodeRole::Control => {
            let control_deps = build_control_deps_from_worker_runtime(worker_runtime)?;
            serve_control(control_deps, config.control).await
        }
        NodeRole::All => {
            let control_deps = build_control_deps_from_worker_runtime(worker_runtime.clone())?;
            serve_all(control_deps, worker_runtime, config.worker, config.control).await
        }
    }
}

pub async fn serve_control(deps: ControlDeps, config: ControlHttpConfig) -> anyhow::Result<()> {
    let facade = Arc::new(ControlFacade::new(deps)?);
    serve_control_http(config, facade).await
}

pub async fn serve_control_with_ready(
    deps: ControlDeps,
    config: ControlHttpConfig,
    ready: Option<tokio::sync::oneshot::Sender<std::net::SocketAddr>>,
) -> anyhow::Result<()> {
    let facade = Arc::new(ControlFacade::new(deps)?);
    serve_control_http_with_ready(config, facade, ready).await
}

pub async fn serve_worker(
    runtime: HostedWorkerRuntime,
    config: HostedWorkerConfig,
) -> Result<(), WorkerError> {
    let worker = HostedWorker::new(config);
    let mut supervisor = worker.with_worker_runtime(runtime);
    supervisor.serve_forever().await
}

pub async fn serve_all(
    control_deps: ControlDeps,
    worker_runtime: HostedWorkerRuntime,
    worker_config: HostedWorkerConfig,
    control_config: ControlHttpConfig,
) -> anyhow::Result<()> {
    let warmup_runtime = worker_runtime.clone();
    let (control_ready_tx, control_ready_rx) = tokio::sync::oneshot::channel();
    let mut worker_task = tokio::spawn(async move {
        serve_worker(worker_runtime, worker_config)
            .await
            .map_err(|err| anyhow!("node worker failed: {err}"))
    });
    let mut control_task = tokio::spawn(async move {
        serve_control_with_ready(control_deps, control_config, Some(control_ready_tx)).await
    });
    let warmup_task = tokio::spawn(async move {
        warm_owned_worlds_after_control_ready(control_ready_rx, warmup_runtime).await
    });
    let mut worker_selected = false;
    let mut control_selected = false;

    let outcome = tokio::select! {
        result = &mut worker_task => {
            worker_selected = true;
            tracing::error!("node worker task completed; shutting down remaining roles");
            control_task.abort();
            match result {
                Ok(Ok(())) => Err(anyhow!("node worker exited unexpectedly")),
                Ok(Err(err)) => Err(err),
                Err(err) => Err(anyhow!("node worker task join failed: {err}")),
            }
        }
        result = &mut control_task => {
            control_selected = true;
            tracing::error!("node control task completed; shutting down remaining roles");
            worker_task.abort();
            match result {
                Ok(result) => result,
                Err(err) => Err(anyhow!("node control task join failed: {err}")),
            }
        }
    };

    if !worker_selected {
        let _ = worker_task.await;
    }
    if !control_selected {
        let _ = control_task.await;
    }
    warmup_task.abort();
    let _ = warmup_task.await;
    outcome
}

async fn warm_owned_worlds_after_control_ready(
    control_ready_rx: tokio::sync::oneshot::Receiver<std::net::SocketAddr>,
    runtime: HostedWorkerRuntime,
) {
    let Ok(bind) = control_ready_rx.await else {
        tracing::debug!("aos-node owned world warmup skipped because control never became ready");
        return;
    };

    loop {
        match runtime.scheduler_attached() {
            Ok(true) => break,
            Ok(false) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            Err(err) => {
                tracing::warn!(
                    bind = %bind,
                    error = %err,
                    "aos-node owned world warmup skipped because worker scheduler state was unavailable"
                );
                return;
            }
        }
    }

    match runtime.activate_owned_worlds_best_effort() {
        Ok(summary) => {
            tracing::info!(
                bind = %bind,
                attempted = summary.attempted,
                opened = summary.opened,
                failed = summary.failed,
                "aos-node owned world warmup completed"
            );
        }
        Err(err) => {
            tracing::warn!(
                bind = %bind,
                error = %err,
                "aos-node owned world warmup failed before opening worlds"
            );
        }
    }
}

fn log_node_init(config: &NodeConfig) {
    let roles = match config.role {
        NodeRole::Worker => "worker",
        NodeRole::Control => "control",
        NodeRole::All => "control,worker",
    };
    match &config.journal {
        NodeJournalBackend::Kafka {
            partition_count,
            kafka_config,
            blobstore_config,
        } => {
            tracing::info!(
                bind = %config.control.bind_addr,
                worker_id = %config.worker.worker_id,
                kafka_partitions = *partition_count,
                default_universe_id = %config.default_universe_id,
                state_root = %config.state_root.display(),
                journal_backend = "kafka",
                blob_backend = "object-store",
                kafka_bootstrap_servers = %kafka_config.bootstrap_servers.as_deref().unwrap_or("<missing>"),
                kafka_journal_topic = %kafka_config.journal_topic,
                blobstore_bucket = %blobstore_config.bucket.as_deref().unwrap_or("<missing>"),
                blobstore_endpoint = %blobstore_config.endpoint.as_deref().unwrap_or("<auto>"),
                blobstore_region = %blobstore_config.region.as_deref().unwrap_or("<auto>"),
                blobstore_prefix = %blobstore_config.prefix,
                roles,
                "aos node initialized"
            );
        }
        NodeJournalBackend::Sqlite { blobstore_config } => {
            let blob_backend = if blobstore_config.bucket.is_some() {
                "object-store"
            } else {
                "local"
            };
            tracing::info!(
                bind = %config.control.bind_addr,
                worker_id = %config.worker.worker_id,
                default_universe_id = %config.default_universe_id,
                state_root = %config.state_root.display(),
                journal_backend = "sqlite",
                blob_backend,
                blobstore_bucket = %blobstore_config.bucket.as_deref().unwrap_or("<local>"),
                blobstore_endpoint = %blobstore_config.endpoint.as_deref().unwrap_or("<local>"),
                blobstore_region = %blobstore_config.region.as_deref().unwrap_or("<local>"),
                blobstore_prefix = %blobstore_config.prefix,
                roles,
                "aos node initialized"
            );
        }
    }
}

pub fn ensure_node_state_root(config: &NodeConfig) -> anyhow::Result<()> {
    std::fs::create_dir_all(&config.state_root)
        .with_context(|| format!("create node state root {}", config.state_root.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kafka_journal_requires_object_store_blob_backend() {
        let err = kafka_journal_backend_from_env(1, NodeBlobBackendKind::Local)
            .expect_err("kafka should reject local blob backend");
        assert!(
            err.to_string()
                .contains("Kafka journal mode requires --blob-backend object-store")
        );
    }

    #[test]
    fn local_blob_backend_ignores_object_store_bucket_env() {
        let config =
            blobstore_config_from_env(NodeBlobBackendKind::Local).expect("local blob config");
        assert_eq!(config.bucket, None);
        assert_eq!(config.endpoint, None);
        assert_eq!(config.region, None);
    }
}
