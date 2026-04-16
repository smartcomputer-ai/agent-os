#![forbid(unsafe_code)]

//! Shared node APIs, models, and internal backends for daemonized runtimes.

extern crate self as aos_node;

pub mod bootstrap;
pub mod config;
pub mod control;
mod env;
pub mod execution;
pub mod harness;
pub mod infra;
pub mod model;
pub mod node;
mod paths;
pub mod services;
pub mod test_support;
pub mod worker;

pub use env::load_dotenv_candidates;
pub use infra::{blobstore, kafka, sqlite, vault};

pub use execution::*;
pub use harness::{
    NodeForkResult, NodeHarnessControl, NodeHarnessError, NodeHarnessStep, NodeWorldHarness,
};
pub use infra::blobstore::FsCas;
pub use model::forking::rewrite_snapshot_for_fork_policy;
pub use model::*;
pub use node::*;
pub use paths::LocalStatePaths;
pub use worker::{
    CreateWorldAccepted, HostedWorker, HostedWorkerRuntime, HostedWorldSummary, SubmissionAccepted,
    SubmitEventRequest, SupervisorOutcome, SupervisorRunProfile, WorkerError, WorkerSupervisor,
    WorkerSupervisorHandle,
};
