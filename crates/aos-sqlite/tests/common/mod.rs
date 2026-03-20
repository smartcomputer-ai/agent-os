use aos_node::UniverseId;
use aos_sqlite::{LocalStatePaths, SqliteNodeStore};
use tempfile::TempDir;
use uuid::Uuid;

pub fn temp_state_root() -> (TempDir, LocalStatePaths) {
    let temp = TempDir::new().expect("create temp dir");
    let paths = LocalStatePaths::new(temp.path().join(".aos"));
    (temp, paths)
}

pub fn open_store(paths: &LocalStatePaths) -> SqliteNodeStore {
    SqliteNodeStore::open_with_paths(paths).expect("open sqlite node store")
}

pub fn universe() -> UniverseId {
    UniverseId::from(Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap())
}
