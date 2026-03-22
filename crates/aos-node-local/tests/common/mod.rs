#![allow(dead_code)]

use aos_node::WorldId;
use aos_node_local::LocalStatePaths;
use tempfile::TempDir;
use uuid::Uuid;

pub fn temp_state_root() -> (TempDir, LocalStatePaths) {
    let temp = TempDir::new().expect("create temp dir");
    let paths = LocalStatePaths::new(temp.path().join(".aos"));
    (temp, paths)
}

pub fn world() -> WorldId {
    WorldId::from(Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap())
}

pub fn world2() -> WorldId {
    WorldId::from(Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap())
}

pub fn world3() -> WorldId {
    WorldId::from(Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap())
}
