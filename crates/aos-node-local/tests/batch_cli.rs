mod common;

#[path = "../../aos-runtime/tests/helpers.rs"]
mod runtime_helpers;

use std::process::Command;
use std::sync::Arc;

use aos_cbor::Hash;
use aos_effect_types::{GovPatchInput, GovProposeParams};
use aos_kernel::Store;
use aos_kernel::governance::ManifestPatch;
use aos_node::{CommandRecord, CreateWorldRequest, CreateWorldSource};
use aos_node_local::{FsCas, LocalControl, LocalStatePaths};
use aos_runtime::manifest_loader::store_loaded_manifest;
use base64::Engine;
use runtime_helpers::{fixtures, simple_state_manifest};
use serde_json::Value;

use common::world;

fn copy_manifest_module_blobs(
    source: &std::sync::Arc<fixtures::TestStore>,
    target: &FsCas,
    loaded: &aos_kernel::LoadedManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    for module in loaded.modules.values() {
        let hash = aos_cbor::Hash::from_hex_str(module.wasm_hash.as_str())?;
        let bytes = source.get_blob(hash)?;
        let stored = target.put_blob(&bytes)?;
        assert_eq!(stored, hash, "copied wasm blob hash mismatch");
    }
    Ok(())
}

fn bootstrap_real_world(
    state_root: &std::path::Path,
) -> Result<(Arc<LocalControl>, aos_node::WorldId), Box<dyn std::error::Error>> {
    let control = LocalControl::open_batch(state_root)?;

    let paths = LocalStatePaths::new(state_root.to_path_buf());
    let cas = FsCas::open_with_paths(&paths)?;
    let manifest_store = fixtures::new_mem_store();
    let loaded = simple_state_manifest(&manifest_store);
    copy_manifest_module_blobs(&manifest_store, &cas, &loaded)?;
    let manifest_hash = store_loaded_manifest(&cas, &loaded)?;

    let created = control.create_world(CreateWorldRequest {
        world_id: Some(world()),
        universe_id: aos_node::UniverseId::nil(),
        created_at_ns: 42,
        source: CreateWorldSource::Manifest {
            manifest_hash: manifest_hash.to_hex(),
        },
    })?;
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
    assert_eq!(worlds[0].world_id, world());

    let status = run_batch(paths.root(), &["status", "--world", &world().to_string()]);
    assert!(
        status.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: Value = serde_json::from_slice(&status.stdout).expect("decode status json");
    assert_eq!(status["runtime"]["world_id"], world().to_string());

    let manifest = run_batch(paths.root(), &["manifest", "--world", &world().to_string()]);
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
    let (control, world_id) = bootstrap_real_world(paths.root()).expect("bootstrap real world");
    let manifest = control.manifest(world_id).expect("manifest");
    let manifest_hash = Hash::from_hex_str(&manifest.manifest_hash).expect("manifest hash");
    let cas = FsCas::open_with_paths(&paths).expect("open cas");
    let payload = serde_cbor::to_vec(&GovProposeParams {
        patch: GovPatchInput::PatchCbor(
            serde_cbor::to_vec(&ManifestPatch {
                manifest: cas.get_node(manifest_hash).expect("manifest node"),
                nodes: Vec::new(),
            })
            .expect("patch cbor"),
        ),
        summary: None,
        manifest_base: None,
        description: Some("batch test proposal".into()),
    })
    .expect("proposal params cbor");

    let submit = run_batch(
        paths.root(),
        &[
            "command",
            "submit",
            "--world",
            &world().to_string(),
            "--command",
            "gov-propose",
            "--command-id",
            "proposal-1",
            "--payload-b64",
            &base64::engine::general_purpose::STANDARD.encode(payload),
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
    assert_eq!(record.command_id, "proposal-1");
    assert!(matches!(record.status, aos_node::CommandStatus::Succeeded));

    let get = run_batch(
        paths.root(),
        &[
            "command",
            "get",
            "--world",
            &world().to_string(),
            "--command-id",
            "proposal-1",
        ],
    );
    assert!(
        get.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&get.stderr)
    );
    let fetched: CommandRecord =
        serde_json::from_slice(&get.stdout).expect("decode fetched command record");
    assert_eq!(fetched.command_id, "proposal-1");
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
