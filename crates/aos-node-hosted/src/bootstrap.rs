use std::path::PathBuf;

use aos_kernel::SharedSecretResolver;
use aos_node::LocalStatePaths;
use aos_node::api::ControlError;

use crate::blobstore::BlobStoreConfig;
use crate::control::control_error_from_materializer;
use crate::control::control_error_from_worker;
use crate::kafka::KafkaConfig;
use crate::materializer::MaterializerSqliteStore;
use crate::services::{
    HostedCasService, HostedJournalService, HostedMetaService, HostedProjectionStore,
    HostedReplayService, HostedSecretService, HostedSubmissionService,
};
use crate::vault::HostedVault;
use crate::worker::{HostedWorkerRuntime, WorkerError};

pub struct ControlDeps {
    pub state_root: PathBuf,
    pub default_universe_id: aos_node::UniverseId,
    pub submissions: HostedSubmissionService,
    pub cas: HostedCasService,
    pub secrets: HostedSecretService,
    pub projections: HostedProjectionStore,
    pub replay: HostedReplayService,
}

pub struct MaterializerDeps {
    pub paths: LocalStatePaths,
    pub journal: HostedJournalService,
    pub kafka_config: KafkaConfig,
    pub stores: HostedCasService,
    pub secret_resolver: SharedSecretResolver,
}

pub fn build_control_deps_broker(
    partition_count: u32,
    state_root: impl Into<PathBuf>,
    default_universe_id: aos_node::UniverseId,
    kafka_config: KafkaConfig,
    blobstore_config: BlobStoreConfig,
) -> Result<ControlDeps, ControlError> {
    let paths = LocalStatePaths::new(state_root.into());
    paths.ensure_root().map_err(|err| {
        control_error_from_worker(WorkerError::Persist(aos_node::PersistError::backend(
            err.to_string(),
        )))
    })?;
    let projections = MaterializerSqliteStore::from_paths(&paths)
        .map(HostedProjectionStore::new)
        .map_err(control_error_from_materializer)?;
    let journal = HostedJournalService::new(partition_count, kafka_config)
        .map_err(control_error_from_worker)?;
    let journal_topic = journal.journal_topic().map_err(control_error_from_worker)?;
    let meta =
        HostedMetaService::standalone(journal_topic, partition_count, blobstore_config.clone());
    let cas = HostedCasService::standalone(paths.clone(), blobstore_config.clone());
    let vault = HostedVault::new(blobstore_config).map_err(|err| {
        control_error_from_worker(WorkerError::Build(anyhow::anyhow!(
            "initialize hosted vault: {err}"
        )))
    })?;
    Ok(ControlDeps {
        state_root: paths.root().to_path_buf(),
        default_universe_id,
        submissions: HostedSubmissionService::new(
            default_universe_id,
            journal.clone(),
            meta.clone(),
            cas.clone(),
        ),
        cas: cas.clone(),
        secrets: HostedSecretService::new(vault.clone()),
        projections,
        replay: HostedReplayService::new(journal, cas, meta, vault),
    })
}

pub fn build_control_deps_from_worker_runtime(
    runtime: HostedWorkerRuntime,
) -> Result<ControlDeps, ControlError> {
    let default_universe_id = runtime
        .default_universe_id()
        .map_err(control_error_from_worker)?;
    let partition_count = runtime
        .partition_count()
        .map_err(control_error_from_worker)?;
    let kafka_config = runtime.kafka_config().map_err(control_error_from_worker)?;
    let blobstore_config = runtime
        .blobstore_config()
        .map_err(control_error_from_worker)?;
    let projections = MaterializerSqliteStore::from_paths(runtime.paths())
        .map(HostedProjectionStore::new)
        .map_err(control_error_from_materializer)?;
    let broker_mode = runtime
        .uses_broker_kafka()
        .map_err(control_error_from_worker)?;
    let (submissions, cas, secrets, replay) = if broker_mode {
        let journal = HostedJournalService::new(partition_count, kafka_config)
            .map_err(control_error_from_worker)?;
        let journal_topic = journal.journal_topic().map_err(control_error_from_worker)?;
        let meta =
            HostedMetaService::standalone(journal_topic, partition_count, blobstore_config.clone());
        let cas = HostedCasService::standalone(runtime.paths().clone(), blobstore_config.clone());
        let vault = HostedVault::new(blobstore_config).map_err(|err| {
            control_error_from_worker(crate::worker::WorkerError::Build(anyhow::anyhow!(
                "initialize hosted vault: {err}"
            )))
        })?;
        (
            HostedSubmissionService::from_runtime(
                runtime.clone(),
                default_universe_id,
                journal.clone(),
                meta.clone(),
                cas.clone(),
            ),
            cas.clone(),
            HostedSecretService::new(vault.clone()),
            HostedReplayService::new(journal, cas, meta, vault),
        )
    } else {
        let secrets = runtime.vault().map_err(control_error_from_worker)?;
        let journal = journal_service_from_worker_runtime(runtime.clone());
        let cas = cas_service_from_worker_runtime(runtime.clone());
        let meta = meta_service_from_worker_runtime(runtime.clone());
        (
            HostedSubmissionService::from_runtime(
                runtime.clone(),
                default_universe_id,
                journal.clone(),
                meta.clone(),
                cas.clone(),
            ),
            cas.clone(),
            HostedSecretService::new(secrets),
            HostedReplayService::new(
                journal,
                cas,
                meta,
                runtime.vault().map_err(control_error_from_worker)?,
            ),
        )
    };
    Ok(ControlDeps {
        state_root: runtime.paths().root().to_path_buf(),
        default_universe_id,
        submissions,
        cas,
        secrets,
        projections,
        replay,
    })
}

pub fn build_materializer_deps(
    partition_count: u32,
    state_root: impl Into<PathBuf>,
    default_universe_id: aos_node::UniverseId,
    kafka_config: KafkaConfig,
    blobstore_config: BlobStoreConfig,
) -> Result<MaterializerDeps, crate::worker::WorkerError> {
    let paths = LocalStatePaths::new(state_root.into());
    paths.ensure_root().map_err(|err| {
        crate::worker::WorkerError::Persist(aos_node::PersistError::backend(err.to_string()))
    })?;
    let vault = crate::vault::HostedVault::new(blobstore_config.clone()).map_err(|err| {
        crate::worker::WorkerError::Build(anyhow::anyhow!("initialize hosted vault: {err}"))
    })?;
    Ok(MaterializerDeps {
        paths: paths.clone(),
        journal: HostedJournalService::new(partition_count, kafka_config.clone())?,
        kafka_config,
        stores: HostedCasService::standalone(paths, blobstore_config),
        secret_resolver: std::sync::Arc::new(vault.resolver_for_universe(default_universe_id)),
    })
}

pub fn build_worker_runtime_broker(
    partition_count: u32,
    state_root: impl Into<PathBuf>,
    default_universe_id: aos_node::UniverseId,
    kafka_config: KafkaConfig,
    blobstore_config: BlobStoreConfig,
) -> Result<HostedWorkerRuntime, WorkerError> {
    HostedWorkerRuntime::new_broker_with_state_root_and_universe(
        partition_count,
        state_root,
        default_universe_id,
        kafka_config,
        blobstore_config,
    )
}

fn cas_service_from_worker_runtime(runtime: HostedWorkerRuntime) -> HostedCasService {
    HostedCasService::from_provider_with_module_cache_dir(
        {
            let runtime = runtime.clone();
            move |universe_id| runtime.cas_store_for_domain(universe_id)
        },
        move |universe_id| {
            Ok(runtime
                .paths()
                .for_universe(universe_id)
                .wasmtime_cache_dir())
        },
    )
}

fn journal_service_from_worker_runtime(runtime: HostedWorkerRuntime) -> HostedJournalService {
    HostedJournalService::from_callbacks(
        {
            let runtime = runtime.clone();
            move || runtime.refresh_materializer_source()
        },
        {
            let runtime = runtime.clone();
            move || runtime.partition_count()
        },
        {
            let runtime = runtime.clone();
            move || runtime.journal_topic()
        },
        {
            let runtime = runtime.clone();
            move |partition| runtime.partition_entries(partition)
        },
        {
            let runtime = runtime.clone();
            move |world_id| {
                let partition = runtime.effective_partition(world_id)?;
                runtime.refresh_materializer_source()?;
                Ok(runtime
                    .partition_entries(partition)?
                    .into_iter()
                    .filter(|entry| entry.frame.world_id == world_id)
                    .map(|entry| entry.frame)
                    .collect())
            }
        },
        move |submission| runtime.submit_submission(submission),
    )
}

fn meta_service_from_worker_runtime(runtime: HostedWorkerRuntime) -> HostedMetaService {
    HostedMetaService::from_callbacks(
        {
            let runtime = runtime.clone();
            move |universe_id, world_id, command_id| match runtime.get_command_record(
                universe_id,
                world_id,
                command_id,
            ) {
                Ok(record) => Ok(Some(record)),
                Err(WorkerError::UnknownCommand { .. }) => Ok(None),
                Err(err) => Err(err),
            }
        },
        {
            let runtime = runtime.clone();
            move |universe_id, world_id, record| {
                runtime.put_command_record(universe_id, world_id, record)
            }
        },
        move |universe_id, partition| runtime.latest_checkpoint(universe_id, partition),
    )
}
