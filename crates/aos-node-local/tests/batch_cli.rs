mod common;

#[path = "../../aos-runtime/tests/helpers.rs"]
mod runtime_helpers;

use std::process::Command;
use std::sync::Arc;

use aos_node::control::NodeControl;
use aos_node::{CommandRecord, CreateWorldRequest, CreateWorldSource, HostedStore, WorldStore};
use aos_node_local::{LocalControl, SqliteNodeStore};
use aos_runtime::manifest_loader::store_loaded_manifest;
use aos_sqlite::LocalStatePaths;
use runtime_helpers::{fixtures, simple_state_manifest};
use serde_json::Value;

use common::world;

fn bootstrap_real_world(
    state_root: &std::path::Path,
) -> Result<(Arc<LocalControl>, aos_node::WorldId), Box<dyn std::error::Error>> {
    let control = LocalControl::open_batch(state_root)?;
    let universe = control.local_universe_id();
    assert_eq!(universe, common::universe());

    let paths = LocalStatePaths::new(state_root.to_path_buf());
    let store = Arc::new(SqliteNodeStore::open_with_paths(&paths)?);
    let persistence: Arc<dyn WorldStore> = store;
    let hosted = Arc::new(HostedStore::new(persistence, universe));
    let manifest_store = fixtures::new_mem_store();
    let loaded = simple_state_manifest(&manifest_store);
    let manifest_hash = store_loaded_manifest(hosted.as_ref(), &loaded)?;

    let created = control.create_world(
        universe,
        CreateWorldRequest {
            world_id: Some(world()),
            handle: Some("demo".into()),
            placement_pin: None,
            created_at_ns: 42,
            source: CreateWorldSource::Manifest {
                manifest_hash: manifest_hash.to_hex(),
            },
        },
    )?;
    Ok((control, created.record.world_id))
}

fn run_batch(state_root: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aos-node-local"))
        .arg("batch")
        .arg("--state-root")
        .arg(state_root)
        .args(args)
        .output()
        .expect("run aos-node-local batch")
}

#[test]
fn batch_worlds_status_and_manifest_operate_on_persisted_local_worlds() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = LocalStatePaths::new(temp.path().join(".aos"));
    let _ = bootstrap_real_world(paths.root()).expect("bootstrap real world");

    let worlds = run_batch(paths.root(), &["worlds"]);
    assert!(
        worlds.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&worlds.stderr)
    );
    let worlds: Vec<aos_node::WorldRuntimeInfo> =
        serde_json::from_slice(&worlds.stdout).expect("decode worlds json");
    assert_eq!(worlds.len(), 1);
    assert_eq!(worlds[0].meta.handle, "demo");

    let status = run_batch(paths.root(), &["status", "--world", "demo"]);
    assert!(
        status.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: Value = serde_json::from_slice(&status.stdout).expect("decode status json");
    assert_eq!(status["runtime"]["meta"]["handle"], "demo");
    assert_eq!(status["runtime"]["world_id"], world().to_string());

    let manifest = run_batch(paths.root(), &["manifest", "--world", "demo"]);
    assert!(
        manifest.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&manifest.stderr)
    );
    let manifest: Value = serde_json::from_slice(&manifest.stdout).expect("decode manifest json");
    assert!(
        manifest["manifest_hash"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(
        manifest["manifest"]["modules"].as_array().map(Vec::len),
        Some(1)
    );
}

#[test]
fn batch_command_submit_and_get_operate_on_persisted_local_worlds() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = LocalStatePaths::new(temp.path().join(".aos"));
    let (_control, _world_id) = bootstrap_real_world(paths.root()).expect("bootstrap real world");

    let submit = run_batch(
        paths.root(),
        &[
            "command",
            "submit",
            "--world",
            "demo",
            "--command",
            "world-pause",
            "--command-id",
            "pause-1",
            "--payload-json",
            "{}",
        ],
    );
    assert!(
        submit.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&submit.stderr)
    );
    let record: CommandRecord =
        serde_json::from_slice(&submit.stdout).expect("decode command record");
    assert!(record.journal_height.is_some());
    assert_eq!(record.command_id, "pause-1");
    assert!(matches!(record.status, aos_node::CommandStatus::Succeeded));

    let get = run_batch(
        paths.root(),
        &[
            "command",
            "get",
            "--world",
            "demo",
            "--command-id",
            "pause-1",
        ],
    );
    assert!(
        get.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&get.stderr)
    );
    let fetched: CommandRecord =
        serde_json::from_slice(&get.stdout).expect("decode fetched command record");
    assert_eq!(fetched.command_id, "pause-1");
    assert!(matches!(fetched.status, aos_node::CommandStatus::Succeeded));
}

#[test]
fn batch_status_accepts_world_uuid_selectors() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = LocalStatePaths::new(temp.path().join(".aos"));
    let (_control, world_id) = bootstrap_real_world(paths.root()).expect("bootstrap real world");

    let status = run_batch(paths.root(), &["status", "--world", &world_id.to_string()]);
    assert!(
        status.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: Value = serde_json::from_slice(&status.stdout).expect("decode status json");
    assert_eq!(status["runtime"]["world_id"], world_id.to_string());
}
