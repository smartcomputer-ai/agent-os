use std::path::PathBuf;

use aos_node::control::ControlError;

use crate::blobstore::BlobStoreConfig;
use crate::control::control_error_from_worker;
use crate::kafka::KafkaConfig;
use crate::services::{
    HostedCasService, HostedJournalService, HostedMetaService, HostedReplayService,
    HostedSecretService, HostedSubmissionService, KafkaDebugService,
};
use crate::vault::HostedVault;
use crate::worker::{HostedWorkerRuntime, WorkerError};

pub struct ControlDeps {
    pub state_root: PathBuf,
    pub default_universe_id: aos_node::UniverseId,
    pub runtime: HostedWorkerRuntime,
    pub submissions: HostedSubmissionService,
    pub cas: HostedCasService,
    pub secrets: HostedSecretService,
    pub replay: HostedReplayService,
}

pub fn build_control_deps_from_worker_runtime(
    runtime: HostedWorkerRuntime,
) -> Result<ControlDeps, ControlError> {
    let default_universe_id = runtime
        .default_universe_id()
        .map_err(control_error_from_worker)?;
    let blobstore_config = runtime
        .blobstore_config()
        .map_err(control_error_from_worker)?;
    let broker_mode = runtime
        .uses_broker_kafka()
        .map_err(control_error_from_worker)?;
    let (submissions, cas, secrets, replay) = if broker_mode {
        let kafka_config = runtime.kafka_config().map_err(control_error_from_worker)?;
        let partition_count = runtime
            .partition_count()
            .map_err(control_error_from_worker)?;
        let journal = HostedJournalService::new(partition_count, kafka_config)
            .map_err(control_error_from_worker)?;
        let meta = HostedMetaService::standalone(blobstore_config.clone());
        let cas = HostedCasService::standalone(runtime.paths().clone(), blobstore_config.clone());
        let vault = HostedVault::new(blobstore_config).map_err(|err| {
            control_error_from_worker(crate::worker::WorkerError::Build(anyhow::anyhow!(
                "initialize node vault: {err}"
            )))
        })?;
        (
            HostedSubmissionService::from_runtime(
                runtime.clone(),
                default_universe_id,
                journal.clone(),
                meta.clone(),
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
        runtime,
        submissions,
        cas,
        secrets,
        replay,
    })
}

pub fn build_kafka_debug_service_from_worker_runtime(
    runtime: HostedWorkerRuntime,
) -> KafkaDebugService {
    KafkaDebugService::from_runtime(runtime)
}

pub fn build_worker_runtime_kafka(
    partition_count: u32,
    state_root: impl Into<PathBuf>,
    default_universe_id: aos_node::UniverseId,
    kafka_config: KafkaConfig,
    blobstore_config: BlobStoreConfig,
) -> Result<HostedWorkerRuntime, WorkerError> {
    HostedWorkerRuntime::new_kafka_with_state_root_and_universe(
        partition_count,
        state_root,
        default_universe_id,
        kafka_config,
        blobstore_config,
    )
}

pub fn build_worker_runtime_sqlite(
    state_root: impl Into<PathBuf>,
    default_universe_id: aos_node::UniverseId,
    blobstore_config: BlobStoreConfig,
) -> Result<HostedWorkerRuntime, WorkerError> {
    HostedWorkerRuntime::new_sqlite_with_state_root_and_universe(
        state_root,
        default_universe_id,
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
            move || runtime.refresh_journal_source()
        },
        {
            let runtime = runtime.clone();
            move |world_id| runtime.world_frames(world_id)
        },
        {
            let runtime = runtime.clone();
            move |world_id, after_world_seq, cursor| {
                runtime.world_tail_frames(world_id, after_world_seq, cursor)
            }
        },
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
        move |universe_id, world_id| runtime.latest_world_checkpoint(universe_id, world_id),
    )
}
