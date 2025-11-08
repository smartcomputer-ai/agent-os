use aos_air_exec::Value as ExprValue;
use aos_effects::builtins::TimerSetReceipt;
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_kernel::journal::mem::MemJournal;
use aos_testkit::fixtures::{self, START_SCHEMA};
use aos_testkit::TestWorld;
use aos_wasm_abi::ReducerOutput;
use serde_cbor;
use std::sync::Arc;

mod helpers;
use helpers::{await_event_manifest, fulfillment_manifest, timer_manifest};

/// Plan execution should resume correctly after snapshot when awaiting a receipt.
#[test]
fn plan_snapshot_resumes_after_receipt() {
    let store = fixtures::new_mem_store();
    let manifest = fulfillment_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let input = fixtures::plan_input_record(vec![("id", ExprValue::Text("123".into()))]);
    world.submit_event_value(START_SCHEMA, &input);
    world.tick_n(2).unwrap();

    let effect = world
        .drain_effects()
        .pop()
        .expect("expected effect before snapshot");

    world.kernel.create_snapshot().unwrap();
    let journal_entries = world.kernel.dump_journal().unwrap();

    let mut replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        fulfillment_manifest(&store),
        Box::new(MemJournal::from_entries(&journal_entries)),
    )
    .unwrap();

    let receipt_payload = serde_cbor::to_vec(&ExprValue::Text("done".into())).unwrap();
    let receipt = EffectReceipt {
        intent_hash: effect.intent_hash,
        adapter_id: "adapter.http".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: receipt_payload,
        cost_cents: None,
        signature: vec![],
    };
    replay_world.kernel.handle_receipt(receipt).unwrap();
    replay_world.kernel.tick_until_idle().unwrap();

    assert_eq!(
        replay_world
            .kernel
            .reducer_state("com.acme/ResultReducer@1"),
        Some(&vec![0xEE])
    );
}

/// Effect intents should persist in the queue across snapshot/restore.
#[test]
fn plan_snapshot_preserves_effect_queue() {
    let store = fixtures::new_mem_store();
    let manifest = fulfillment_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let input = fixtures::plan_input_record(vec![("id", ExprValue::Text("123".into()))]);
    world.submit_event_value(START_SCHEMA, &input);
    world.tick_n(2).unwrap();

    world.kernel.create_snapshot().unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let mut replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        fulfillment_manifest(&store),
        Box::new(MemJournal::from_entries(&entries)),
    )
    .unwrap();

    let mut intents = replay_world.drain_effects();
    assert_eq!(intents.len(), 1, "effect queue should persist across snapshot");
    let effect = intents.remove(0);

    let receipt_payload = serde_cbor::to_vec(&ExprValue::Text("done".into())).unwrap();
    let receipt = EffectReceipt {
        intent_hash: effect.intent_hash,
        adapter_id: "adapter.http".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: receipt_payload,
        cost_cents: None,
        signature: vec![],
    };
    replay_world.kernel.handle_receipt(receipt).unwrap();
    replay_world.kernel.tick_until_idle().unwrap();

    assert_eq!(
        replay_world
            .kernel
            .reducer_state("com.acme/ResultReducer@1"),
        Some(&vec![0xEE])
    );
}

/// Plan awaiting domain event should resume correctly after snapshot.
#[test]
fn plan_snapshot_resumes_after_event() {
    let store = fixtures::new_mem_store();
    let manifest = await_event_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let input = fixtures::plan_input_record(vec![("id", ExprValue::Text("evt".into()))]);
    world.submit_event_value(START_SCHEMA, &input);
    world.tick_n(1).unwrap();
    world.tick_n(1).unwrap();

    world.kernel.create_snapshot().unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let mut replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        await_event_manifest(&store),
        Box::new(MemJournal::from_entries(&entries)),
    )
    .unwrap();

    let unblock = fixtures::plan_input_record(vec![]);
    replay_world.submit_event_value("com.acme/EmitUnblock@1", &unblock);
    replay_world.kernel.tick_until_idle().unwrap();

    assert_eq!(
        replay_world.kernel.reducer_state("com.acme/EventResult@1"),
        Some(&vec![0xAB])
    );
}

/// Reducer-emitted timer effects should resume correctly on receipt after snapshot.
#[test]
fn reducer_timer_snapshot_resumes_on_receipt() {
    let store = fixtures::new_mem_store();
    let manifest = timer_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    world.submit_event_value(START_SCHEMA, &fixtures::plan_input_record(vec![]));
    world.tick_n(1).unwrap();

    let effect = world
        .drain_effects()
        .pop()
        .expect("expected timer effect before snapshot");

    world.kernel.create_snapshot().unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let mut replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        timer_manifest(&store),
        Box::new(MemJournal::from_entries(&entries)),
    )
    .unwrap();

    let receipt = EffectReceipt {
        intent_hash: effect.intent_hash,
        adapter_id: "adapter.timer".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
            delivered_at_ns: 10,
            key: Some("retry".into()),
        })
        .unwrap(),
        cost_cents: Some(1),
        signature: vec![1, 2, 3],
    };
    replay_world.kernel.handle_receipt(receipt).unwrap();
    replay_world.tick_n(2).unwrap();

    assert_eq!(
        replay_world.kernel.reducer_state("com.acme/TimerHandler@1"),
        Some(&vec![0xCC])
    );
}

/// Simple snapshot/restore without any in-flight effects should restore reducer state.
#[test]
fn snapshot_replay_restores_state() {
    fn build_manifest(store: &Arc<aos_testkit::TestStore>) -> aos_kernel::manifest::LoadedManifest {
        let reducer = fixtures::stub_reducer_module(
            store,
            "com.acme/Simple@1",
            &ReducerOutput {
                state: Some(vec![0xAA]),
                domain_events: vec![],
                effects: vec![],
                ann: None,
            },
        );
        let routing = vec![fixtures::routing_event(START_SCHEMA, &reducer.name)];
        fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing)
    }

    let store = fixtures::new_mem_store();
    let manifest = build_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();
    world.submit_event_value(START_SCHEMA, &fixtures::plan_input_record(vec![]));
    world.tick_n(1).unwrap();

    world.kernel.create_snapshot().unwrap();

    let final_state = world
        .kernel
        .reducer_state("com.acme/Simple@1")
        .cloned()
        .unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        build_manifest(&store),
        Box::new(MemJournal::from_entries(&entries)),
    )
    .unwrap();

    assert_eq!(
        replay_world.kernel.reducer_state("com.acme/Simple@1"),
        Some(&final_state)
    );
}
