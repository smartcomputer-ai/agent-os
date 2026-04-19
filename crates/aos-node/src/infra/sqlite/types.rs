use std::path::PathBuf;

use aos_node::LocalStatePaths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostedSqliteJournalConfig {
    pub(crate) db_path: PathBuf,
    pub(crate) busy_timeout_ms: u64,
}

impl HostedSqliteJournalConfig {
    pub(crate) fn for_paths(paths: &LocalStatePaths) -> Self {
        Self {
            db_path: paths.root().join("journal.sqlite3"),
            busy_timeout_ms: 5_000,
        }
    }
}
