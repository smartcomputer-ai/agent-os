use aos_air_exec::Value as ExprValue;
use aos_effects::builtins::TimerSetReceipt;
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_kernel::journal::mem::MemJournal;
use aos_testkit::fixtures::{self, START_SCHEMA};
use aos_testkit::TestWorld;
use serde_cbor;

mod helpers;
use helpers::{fulfillment_manifest, timer_manifest};

/// Journal replay without snapshots should restore reducer state identically.
#[test]
fn journal_replay_restores_state() {
    let store = fixtures::new_mem_store();
    let manifest_run = fulfillment_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest_run).unwrap();

    let input = fixtures::plan_input_record(vec![("id", ExprValue::Text("123".into()))]);
    world.submit_event_value(START_SCHEMA, &input);
    world.tick_n(2).unwrap();

    let mut effects = world.drain_effects();
    assert_eq!(effects.len(), 1);
    let effect = effects.remove(0);
    let receipt_payload = serde_cbor::to_vec(&ExprValue::Text("done".into())).unwrap();
    let receipt = EffectReceipt {
        intent_hash: effect.intent_hash,
        adapter_id: "adapter.http".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: receipt_payload,
        cost_cents: None,
        signature: vec![],
    };
    world.kernel.handle_receipt(receipt).unwrap();
    world.kernel.tick_until_idle().unwrap();

    let final_state = world
        .kernel
        .reducer_state("com.acme/ResultReducer@1")
        .cloned()
        .unwrap();
    let journal_entries = world.kernel.dump_journal().unwrap();

    let manifest_replay = fulfillment_manifest(&store);
    let replay_journal = MemJournal::from_entries(&journal_entries);
    let replay_world =
        TestWorld::with_store_and_journal(store.clone(), manifest_replay, Box::new(replay_journal))
            .unwrap();

    assert_eq!(
        replay_world
            .kernel
            .reducer_state("com.acme/ResultReducer@1"),
        Some(&final_state)
    );
}

/// Timer receipts from reducers should replay correctly from journal, including event routing.
#[test]
fn reducer_timer_receipt_replays_from_journal() {
    let store = fixtures::new_mem_store();
    let manifest = timer_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();
    world.submit_event_value(START_SCHEMA, &fixtures::plan_input_record(vec![]));
    world.tick_n(1).unwrap();

    let effect = world.drain_effects().pop().expect("timer effect");
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
    world.kernel.handle_receipt(receipt).unwrap();
    world.tick_n(1).unwrap();

    let final_state = world
        .kernel
        .reducer_state("com.acme/TimerHandler@1")
        .cloned()
        .unwrap();
    let journal_entries = world.kernel.dump_journal().unwrap();

    let replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        timer_manifest(&store),
        Box::new(MemJournal::from_entries(&journal_entries)),
    )
    .unwrap();

    assert_eq!(
        replay_world.kernel.reducer_state("com.acme/TimerHandler@1"),
        Some(&final_state)
    );
}
