use aos_air_exec::Value as ExprValue;
use aos_air_types::{EffectKind as AirEffectKind, OriginKind, PolicyDecision, PolicyMatch, PolicyRule};
use aos_effects::builtins::TimerSetReceipt;
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_kernel::journal::fs::FsJournal;
use aos_kernel::journal::mem::MemJournal;
use aos_kernel::journal::{
    IntentOriginRecord, JournalKind, JournalRecord, PolicyDecisionOutcome,
};
use helpers::fixtures::{self, START_SCHEMA, TestWorld};
use serde_cbor;
use tempfile::TempDir;

mod helpers;
use helpers::{attach_default_policy, fulfillment_manifest, timer_manifest};

/// Journal replay without snapshots should restore reducer state identically.
#[test]
fn journal_replay_restores_state() {
    let store = fixtures::new_mem_store();
    let manifest_run = fulfillment_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest_run).unwrap();

    world
        .submit_event_result(START_SCHEMA, &serde_json::json!({ "id": "123" }))
        .expect("submit start event");
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
        .unwrap();
    let journal_entries = world.kernel.dump_journal().unwrap();
    let start_schema = journal_entries
        .iter()
        .find(|entry| entry.kind == JournalKind::DomainEvent)
        .map(|entry| {
            let record: JournalRecord = serde_cbor::from_slice(&entry.payload).unwrap();
            match record {
                JournalRecord::DomainEvent(event) => event.schema,
                _ => unreachable!(),
            }
        })
        .expect("journal missing domain event entry");
    assert_eq!(start_schema, START_SCHEMA);

    let manifest_replay = fulfillment_manifest(&store);
    let replay_journal = MemJournal::from_entries(&journal_entries);
    let replay_world =
        TestWorld::with_store_and_journal(store.clone(), manifest_replay, Box::new(replay_journal))
            .unwrap();

    assert_eq!(
        replay_world
            .kernel
            .reducer_state("com.acme/ResultReducer@1"),
        Some(final_state)
    );
}

/// Timer receipts from reducers should replay correctly from journal, including event routing.
#[test]
fn reducer_timer_receipt_replays_from_journal() {
    let store = fixtures::new_mem_store();
    let manifest = timer_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("timer"))
        .expect("submit start event");
    world.tick_n(2).unwrap();

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
        Some(final_state)
    );
}

/// Journal replay alone (without snapshots) should hydrate plan-origin intents waiting on receipts.
#[test]
fn plan_journal_replay_resumes_waiting_receipt() {
    let store = fixtures::new_mem_store();
    let manifest = fulfillment_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    world
        .submit_event_result(START_SCHEMA, &serde_json::json!({ "id": "123" }))
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let effect = world
        .drain_effects()
        .pop()
        .expect("expected pending effect before shutdown");

    let journal_entries = world.kernel.dump_journal().unwrap();
    let (recorded_intent_hash, recorded_plan_id) = journal_entries
        .iter()
        .find(|entry| entry.kind == JournalKind::EffectIntent)
        .map(|entry| {
            let record: JournalRecord = serde_cbor::from_slice(&entry.payload).unwrap();
            match record {
                JournalRecord::EffectIntent(record) => {
                    let plan_id = match record.origin {
                        IntentOriginRecord::Plan { plan_id, .. } => plan_id,
                        _ => unreachable!(),
                    };
                    (record.intent_hash, plan_id)
                }
                _ => unreachable!(),
            }
        })
        .expect("journal should contain effect intent entry");
    assert_eq!(recorded_intent_hash, effect.intent_hash);

    let mut replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        fulfillment_manifest(&store),
        Box::new(MemJournal::from_entries(&journal_entries)),
    )
    .unwrap();

    let pending = replay_world.kernel.pending_plan_receipts();
    assert_eq!(pending.len(), 1, "pending receipt should be restored");
    assert_eq!(pending[0].0, recorded_plan_id);
    assert_eq!(pending[0].1, recorded_intent_hash);
    let waits = replay_world.kernel.debug_plan_waits();
    assert_eq!(
        waits.len(),
        1,
        "expected one plan instance waiting on a receipt"
    );
    assert_eq!(
        waits[0].0, recorded_plan_id,
        "plan id should match recorded value"
    );
    assert_eq!(
        waits[0].1,
        vec![recorded_intent_hash],
        "pending hash should match journal"
    );

    let receipt_payload = serde_cbor::to_vec(&ExprValue::Text("done".into())).unwrap();
    let receipt = EffectReceipt {
        intent_hash: recorded_intent_hash,
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
        Some(vec![0xEE])
    );
}

/// Policy decisions should be journaled for plan-origin effects.
#[test]
fn policy_decision_is_journaled() {
    let store = fixtures::new_mem_store();
    let mut manifest = fulfillment_manifest(&store);
    let policy = aos_air_types::DefPolicy {
        name: "com.acme/allow-plan-http@1".into(),
        rules: vec![PolicyRule {
            when: PolicyMatch {
                effect_kind: Some(AirEffectKind::http_request()),
                origin_kind: Some(OriginKind::Plan),
                ..Default::default()
            },
            decision: PolicyDecision::Allow,
        }],
    };
    attach_default_policy(&mut manifest, policy.clone());

    let mut world = TestWorld::with_store(store, manifest).unwrap();
    world
        .submit_event_result(START_SCHEMA, &serde_json::json!({ "id": "123" }))
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let journal_entries = world.kernel.dump_journal().unwrap();
    let record = journal_entries
        .iter()
        .find(|entry| entry.kind == JournalKind::PolicyDecision)
        .map(|entry| serde_cbor::from_slice::<JournalRecord>(&entry.payload).unwrap())
        .expect("policy decision entry missing");

    match record {
        JournalRecord::PolicyDecision(decision) => {
            assert_eq!(decision.policy_name, policy.name);
            assert_eq!(decision.rule_index, Some(0));
            assert_eq!(decision.decision, PolicyDecisionOutcome::Allow);
        }
        _ => unreachable!("expected policy decision record"),
    }
}

/// FsJournal should persist entries to disk and allow a fresh kernel to resume state.
#[test]
fn fs_journal_persists_across_restarts() {
    let store = fixtures::new_mem_store();
    let temp = TempDir::new().unwrap();

    let final_state = {
        let mut world = TestWorld::with_store_and_journal(
            store.clone(),
            fulfillment_manifest(&store),
            Box::new(FsJournal::open(temp.path()).unwrap()),
        )
        .unwrap();

        world
            .submit_event_result(START_SCHEMA, &serde_json::json!({ "id": "123" }))
            .expect("submit start event");
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

        world
            .kernel
            .reducer_state("com.acme/ResultReducer@1")
            .unwrap()
    };

    let replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        fulfillment_manifest(&store),
        Box::new(FsJournal::open(temp.path()).unwrap()),
    )
    .unwrap();

    assert_eq!(
        replay_world
            .kernel
            .reducer_state("com.acme/ResultReducer@1"),
        Some(final_state)
    );
    assert!(!replay_world.kernel.dump_journal().unwrap().is_empty());
}
