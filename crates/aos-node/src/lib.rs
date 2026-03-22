#![forbid(unsafe_code)]

//! Shared node APIs, models, and internal planes for daemonized runtime backends.

pub mod api;
mod embedded;
pub mod model;
pub mod planes;

pub use embedded::*;
pub use model::forking::rewrite_snapshot_for_fork_policy;
pub use model::*;
pub use planes::*;
