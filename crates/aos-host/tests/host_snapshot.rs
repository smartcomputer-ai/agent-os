#[path = "helpers.rs"]
mod helpers;

use std::sync::Arc;

use aos_host::config::HostConfig;
use aos_host::{ExternalEvent, WorldHost};
use aos_kernel::KernelConfig;
use aos_store::FsStore;
use aos_wasm_abi::{ReducerEffect, ReducerOutput};
use helpers::fixtures;
use serde_cbor;
use serde_json;
use tempfile::TempDir;

use aos_air_types::{DefSchema, ReducerAbi};

use helpers::{def_text_record_schema, insert_test_schemas, text_type};
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
        key: None,
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
    let mut module = fixtures::stub_reducer_module(store, "demo/TimerReducer@1", &output);
    module.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("demo/TimerState@1"),
        event: fixtures::schema("demo/TimerEvent@1"),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::TIMER_SET.into()],
        cap_slots: Default::default(),
    });
    let mut loaded = fixtures::build_loaded_manifest(
        vec![],
        vec![],
        vec![module],
        vec![fixtures::routing_event(
            "demo/TimerEvent@1",
            "demo/TimerReducer@1",
        )],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema("demo/TimerEvent@1", vec![]),
            DefSchema {
                name: "demo/TimerState@1".into(),
                ty: text_type(),
            },
        ],
    );
    loaded
}
