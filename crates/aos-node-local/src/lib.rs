#![forbid(unsafe_code)]

mod batch;
mod control;
mod supervisor;
mod workspace;

pub use aos_sqlite::{LocalSecretConfig, LocalSecretResolver, LocalSecretService, SqliteNodeStore};
pub use batch::{BatchArgs, BatchCommand, run_batch};
pub use control::{LocalControl, LocalHttpConfig, serve};
pub use supervisor::{LocalSupervisor, LocalSupervisorConfig};
