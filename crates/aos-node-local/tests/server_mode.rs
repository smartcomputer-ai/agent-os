mod common;

#[path = "../../aos-runtime/tests/helpers.rs"]
mod runtime_helpers;

use std::sync::Arc;

use aos_cbor::{Hash, to_canonical_cbor};
use aos_kernel::Store;
use aos_node::{CborPayload, CreateWorldRequest, CreateWorldSource, DomainEventIngress};
use aos_node_local::{FsCas, LocalIngressQueue, LocalLogRuntime, LocalStatePaths, LocalWorker};
use aos_runtime::manifest_loader::store_loaded_manifest;
use runtime_helpers::{fixtures, simple_state_manifest};

use common::world;

fn copy_manifest_module_blobs(
    source: &std::sync::Arc<fixtures::TestStore>,
    target: &FsCas,
    loaded: &aos_kernel::LoadedManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    for module in loaded.modules.values() {
        let hash = Hash::from_hex_str(module.wasm_hash.as_str())?;
        let bytes = source.get_blob(hash)?;
        let stored = target.put_blob(&bytes)?;
        assert_eq!(stored, hash, "copied wasm blob hash mismatch");
    }
    Ok(())
}

fn bootstrap_runtime(
    state_root: &std::path::Path,
) -> Result<Arc<LocalLogRuntime>, Box<dyn std::error::Error>> {
    let paths = LocalStatePaths::new(state_root.to_path_buf());
    let runtime = LocalLogRuntime::open(paths.clone())?;
    let cas = FsCas::open_with_paths(&paths)?;
    let fixture_store = fixtures::new_mem_store();
    let loaded = simple_state_manifest(&fixture_store);
    copy_manifest_module_blobs(&fixture_store, &cas, &loaded)?;
    let manifest_hash = store_loaded_manifest(&cas, &loaded)?;
    runtime.create_world(CreateWorldRequest {
        world_id: Some(world()),
        universe_id: aos_node::UniverseId::nil(),
        created_at_ns: 123,
        source: CreateWorldSource::Manifest {
            manifest_hash: manifest_hash.to_hex(),
        },
    })?;
    Ok(runtime)
}

#[test]
fn server_mode_worker_drains_ephemeral_ingress_into_authoritative_frames()
-> Result<(), Box<dyn std::error::Error>> {
    let (_temp, paths) = common::temp_state_root();
    let runtime = bootstrap_runtime(paths.root())?;
    let ingress = Arc::new(LocalIngressQueue::default());
    let worker = LocalWorker::new(runtime.clone(), ingress.clone());

    let head_before = runtime.journal_head(world())?.journal_head;
    let seq = ingress.enqueue(runtime.build_event_submission(
        world(),
        DomainEventIngress {
            schema: fixtures::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&fixtures::start_event("queued-1"))?),
            key: None,
            correlation_id: Some("queued-1".into()),
        },
    )?);

    assert_eq!(seq.to_string(), "0000000000000000");
    assert_eq!(runtime.journal_head(world())?.journal_head, head_before);

    let outcome = worker.run_once()?;
    assert_eq!(outcome.submissions_drained, 1);
    assert_eq!(outcome.frames_appended, 1);
    assert!(runtime.journal_head(world())?.journal_head > head_before);

    let state = runtime.state_get(world(), "com.acme/Simple@1", None)?;
    assert_eq!(state.state_b64.as_deref(), Some("qg=="));
    Ok(())
}

#[test]
fn queued_server_submission_is_not_durable_across_restart() -> Result<(), Box<dyn std::error::Error>>
{
    let (_temp, paths) = common::temp_state_root();
    let runtime = bootstrap_runtime(paths.root())?;
    let ingress = LocalIngressQueue::default();

    let _ = ingress.enqueue(runtime.build_event_submission(
        world(),
        DomainEventIngress {
            schema: fixtures::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&fixtures::start_event(
                "queued-restart",
            ))?),
            key: None,
            correlation_id: Some("queued-restart".into()),
        },
    )?);

    drop(runtime);
    drop(ingress);

    let reopened = LocalLogRuntime::open(paths.clone())?;
    let state = reopened.state_get(world(), "com.acme/Simple@1", None)?;
    assert!(state.state_b64.is_none());
    Ok(())
}
