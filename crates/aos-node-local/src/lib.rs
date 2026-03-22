#![forbid(unsafe_code)]

mod batch;
mod control;
#[allow(dead_code)]
mod workspace;

pub use aos_node::{
    FsCas, LocalBlobPlanes, LocalBlobStoreConfig, LocalControl, LocalIngressQueue, LocalLogRuntime,
    LocalRuntimeError, LocalSqliteConfig, LocalSqlitePlanes, LocalStatePaths, LocalSupervisor,
    LocalSupervisorConfig, LocalWorker, LocalWorkerOutcome,
};
pub use batch::{BatchArgs, BatchCommand, run_batch};
pub use control::{LocalHttpConfig, serve};
