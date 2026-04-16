#![forbid(unsafe_code)]

//! Shared node APIs, models, and internal backends for daemonized runtimes.

pub mod api;
mod embedded;
pub mod execution;
pub mod model;

pub use embedded::*;
pub use execution::*;
pub use model::forking::rewrite_snapshot_for_fork_policy;
pub use model::*;
