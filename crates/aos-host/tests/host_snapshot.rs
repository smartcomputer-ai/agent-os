use std::sync::Arc;

use aos_host::config::HostConfig;
use aos_host::fixtures;
use aos_host::{ExternalEvent, WorldHost};
use aos_kernel::KernelConfig;
use aos_store::FsStore;
use aos_wasm_abi::{ReducerEffect, ReducerOutput};
use serde_cbor;
use serde_json;
use tempfile::TempDir;

/// Ensure WorldHost preserves queued intents across snapshot/reopen.
#[tokio::test]
async fn worldhost_snapshot_preserves_effect_queue() {
    let tmp = TempDir::new().unwrap();
    let world_root = tmp.path();
    let store = Arc::new(FsStore::open(world_root).unwrap());

    let loaded = build_timer_manifest(&store);

    let mut host = WorldHost::from_loaded_manifest(
        store.clone(),
        loaded,
        world_root,
        HostConfig::default(),
        KernelConfig::default(),
    )
    .unwrap();

    host.enqueue_external(ExternalEvent::DomainEvent {
        schema: "demo/TimerEvent@1".into(),
        value: serde_cbor::to_vec(&serde_json::json!({})).unwrap(),
    })
    .unwrap();
    host.drain().unwrap();

    // Effect should be queued but not yet dispatched.
    assert_eq!(host.kernel().queued_effects_snapshot().len(), 1);

    host.snapshot().unwrap();
    drop(host);

    // Reopen and ensure the queued intent was restored from snapshot/journal.
    let loaded2 = build_timer_manifest(&store);
    let host2 = WorldHost::from_loaded_manifest(
        store.clone(),
        loaded2,
        world_root,
        HostConfig::default(),
        KernelConfig::default(),
    )
    .unwrap();
    assert_eq!(host2.kernel().queued_effects_snapshot().len(), 1);
}

fn build_timer_manifest(store: &Arc<FsStore>) -> aos_kernel::LoadedManifest {
    let effect = ReducerEffect::with_cap_slot(
        aos_effects::EffectKind::TIMER_SET,
        serde_cbor::to_vec(&serde_json::json!({
            "deliver_at_ns": 99u64,
            "key": "demo"
        }))
        .unwrap(),
        "default",
    );
    let output = ReducerOutput {
        state: Some(vec![0xAA]),
        domain_events: vec![],
        effects: vec![effect],
        ann: None,
    };
    let module = fixtures::stub_reducer_module(store, "demo/TimerReducer@1", &output);
    fixtures::build_loaded_manifest(
        vec![],
        vec![],
        vec![module],
        vec![fixtures::routing_event(
            "demo/TimerEvent@1",
            "demo/TimerReducer@1",
        )],
    )
}
