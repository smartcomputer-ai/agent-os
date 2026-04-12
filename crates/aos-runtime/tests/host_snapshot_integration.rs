#[path = "helpers.rs"]
mod helpers;

use std::sync::Arc;

use aos_effect_adapters::config::EffectAdapterConfig;
use aos_kernel::KernelConfig;
use aos_kernel::journal::{Journal, JournalRecord};
use aos_node::{FsCas, LocalStatePaths};
use aos_runtime::{ExternalEvent, JournalReplayOpen, TimerScheduler, WorldConfig, WorldHost};
use aos_wasm_abi::{WorkflowEffect, WorkflowOutput};
use helpers::fixtures;
use serde_cbor;
use serde_json;
use tempfile::TempDir;

use aos_air_types::{DefSchema, TypeExpr, TypeRef, TypeVariant, WorkflowAbi};
use indexmap::IndexMap;

use helpers::{def_text_record_schema, insert_test_schemas, text_type};
/// Ensure WorldHost preserves queued intents across snapshot/reopen.
#[tokio::test]
async fn worldhost_snapshot_preserves_effect_queue() {
    let tmp = TempDir::new().unwrap();
    let world_root = tmp.path();
    let paths = LocalStatePaths::from_world_root(world_root);
    let store = Arc::new(FsCas::open_with_paths(&paths).unwrap());

    let loaded = build_timer_manifest(&store);

    let mut host = WorldHost::from_loaded_manifest(
        store.clone(),
        loaded,
        world_root,
        WorldConfig::default(),
        EffectAdapterConfig::default(),
        KernelConfig::default(),
    )
    .unwrap();

    host.enqueue_external(ExternalEvent::DomainEvent {
        schema: "demo/TimerStart@1".into(),
        value: serde_cbor::to_vec(&serde_json::json!({})).unwrap(),
        key: None,
    })
    .unwrap();
    host.drain().unwrap();

    // Effect should be queued but not yet dispatched.
    assert_eq!(host.kernel().queued_effects_snapshot().len(), 1);

    host.snapshot().unwrap();
    let entries = host.kernel().dump_journal().unwrap();
    let active_baseline = entries
        .iter()
        .filter_map(
            |entry| match serde_cbor::from_slice::<JournalRecord>(&entry.payload).ok() {
                Some(JournalRecord::Snapshot(record)) => Some(record),
                _ => None,
            },
        )
        .last()
        .expect("latest snapshot record");
    drop(host);

    // Reopen from the snapshot+journal path and ensure timer state survives restart.
    let loaded2 = build_timer_manifest(&store);
    let mut host2 = WorldHost::from_loaded_manifest_with_journal_replay(
        store.clone(),
        loaded2,
        Journal::from_entries(&entries).unwrap(),
        WorldConfig::default(),
        EffectAdapterConfig::default(),
        KernelConfig::default(),
        Some(JournalReplayOpen {
            active_baseline,
            replay_seed: None,
        }),
    )
    .unwrap();
    let mut scheduler = TimerScheduler::new();
    scheduler.rehydrate_daemon_state(host2.kernel_mut());
    assert_eq!(host2.kernel().queued_effects_snapshot().len(), 0);
    let status = host2.quiescence_status(Some(&scheduler));
    assert_eq!(status.timers_pending, 1);
    assert!(!status.runtime_quiescent);
}

fn build_timer_manifest<S: aos_kernel::Store + ?Sized>(
    store: &Arc<S>,
) -> aos_kernel::LoadedManifest {
    let effect = WorkflowEffect::with_cap_slot(
        aos_effects::EffectKind::TIMER_SET,
        serde_cbor::to_vec(&serde_json::json!({
            "deliver_at_ns": 99u64,
            "key": "demo"
        }))
        .unwrap(),
        "default",
    );
    let output = WorkflowOutput {
        state: Some(vec![0xAA]),
        domain_events: vec![],
        effects: vec![effect],
        ann: None,
    };
    let mut module = fixtures::stub_workflow_module(store, "demo/TimerWorkflow@1", &output);
    module.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("demo/TimerState@1"),
        event: fixtures::schema("demo/TimerEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::TIMER_SET.into()],
        cap_slots: Default::default(),
    });
    let mut loaded = fixtures::build_loaded_manifest(
        vec![module],
        vec![fixtures::routing_event(
            "demo/TimerEvent@1",
            "demo/TimerWorkflow@1",
        )],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema("demo/TimerStart@1", vec![]),
            DefSchema {
                name: "demo/TimerEvent@1".into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("demo/TimerStart@1"),
                            }),
                        ),
                        (
                            "Fired".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(fixtures::SYS_TIMER_FIRED),
                            }),
                        ),
                    ]),
                }),
            },
            DefSchema {
                name: "demo/TimerState@1".into(),
                ty: text_type(),
            },
        ],
    );
    loaded
}
