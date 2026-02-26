use aos_air_exec::Value as ExprValue;
use aos_air_types::{
    DefModule, EffectKind as AirEffectKind, HashRef, ModuleAbi, ModuleKind, OriginKind,
    PolicyDecision, PolicyMatch, PolicyRule, ReducerAbi, TypeExpr, TypeRecord, TypeRef,
    TypeVariant,
};
use aos_effects::builtins::{BlobPutParams, BlobPutReceipt, TimerSetParams, TimerSetReceipt};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_kernel::journal::fs::FsJournal;
use aos_kernel::journal::mem::MemJournal;
use aos_kernel::journal::{IntentOriginRecord, JournalKind, JournalRecord, PolicyDecisionOutcome};
use aos_kernel::snapshot::WorkflowStatusSnapshot;
use helpers::fixtures::{self, START_SCHEMA, TestWorld};
use serde_cbor;
use serde_cbor::Value as CborValue;
use std::collections::BTreeMap;
use std::sync::Arc;
use tempfile::TempDir;
use wat::parse_str;

use aos_store::Store;
use aos_wasm_abi::{DomainEvent, ReducerEffect, ReducerOutput};

mod helpers;
use helpers::{attach_default_policy, fulfillment_manifest, timer_manifest};

/// Journal replay without snapshots should restore reducer state identically.
#[test]
#[ignore = "P2: plan runtime path retired; replaced by workflow fixtures"]
fn journal_replay_restores_state() {
    let store = fixtures::new_mem_store();
    let manifest_run = fulfillment_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest_run).unwrap();

    world
        .submit_event_result(START_SCHEMA, &serde_json::json!({ "id": "123" }))
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let mut effects = world.drain_effects().expect("drain effects");
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

/// Timer receipts from reducer/workflow modules should replay correctly from journal.
#[test]
fn reducer_timer_receipt_replays_from_journal() {
    let store = fixtures::new_mem_store();
    let manifest = timer_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("timer"))
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let effect = world
        .drain_effects()
        .expect("drain effects")
        .pop()
        .expect("timer effect");
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
        .reducer_state("com.acme/TimerEmitter@1")
        .unwrap();
    let journal_entries = world.kernel.dump_journal().unwrap();

    let replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        timer_manifest(&store),
        Box::new(MemJournal::from_entries(&journal_entries)),
    )
    .unwrap();

    assert_eq!(
        replay_world.kernel.reducer_state("com.acme/TimerEmitter@1"),
        Some(final_state)
    );
}

/// Journal replay alone (without snapshots) should hydrate plan-origin intents waiting on receipts.
#[test]
#[ignore = "P2: plan runtime path retired; replaced by workflow fixtures"]
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
        .expect("drain effects")
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

#[test]
fn workflow_no_plan_multi_effect_receipts_replay_from_journal() {
    let store = fixtures::new_mem_store();
    let manifest = no_plan_workflow_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let start_event = serde_json::json!({
        "$tag": "Start",
        "$value": fixtures::start_event("wf-1")
    });
    world
        .submit_event_result("com.acme/WorkflowEvent@1", &start_event)
        .expect("submit workflow start event");
    world.tick_n(1).unwrap();

    let mut effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 2, "workflow should emit multiple effects");
    effects.sort_by(|a, b| a.kind.as_str().cmp(b.kind.as_str()));
    assert_eq!(effects[0].kind.as_str(), aos_effects::EffectKind::BLOB_PUT);
    assert_eq!(effects[1].kind.as_str(), aos_effects::EffectKind::TIMER_SET);

    let snapshot = world.kernel.workflow_instances_snapshot();
    let workflow = snapshot
        .iter()
        .find(|instance| instance.instance_id.starts_with("com.acme/Workflow@1::"))
        .expect("workflow instance snapshot");
    assert_eq!(workflow.inflight_intents.len(), 2);
    assert_eq!(workflow.status, WorkflowStatusSnapshot::Waiting);

    for intent in effects {
        let receipt = match intent.kind.as_str() {
            aos_effects::EffectKind::BLOB_PUT => EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: "adapter.blob".into(),
                status: ReceiptStatus::Ok,
                payload_cbor: serde_cbor::to_vec(&BlobPutReceipt {
                    blob_ref: fixtures::fake_hash(0x21),
                    edge_ref: fixtures::fake_hash(0x22),
                    size: 8,
                })
                .unwrap(),
                cost_cents: Some(1),
                signature: vec![1, 2, 3],
            },
            aos_effects::EffectKind::TIMER_SET => EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: "adapter.timer".into(),
                status: ReceiptStatus::Ok,
                payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
                    delivered_at_ns: 42,
                    key: Some("wf".into()),
                })
                .unwrap(),
                cost_cents: Some(1),
                signature: vec![4, 5, 6],
            },
            other => panic!("unexpected effect kind in fixture: {other}"),
        };
        world.kernel.handle_receipt(receipt).unwrap();
    }
    world.kernel.tick_until_idle().unwrap();

    assert_eq!(
        world.kernel.reducer_state("com.acme/WorkflowResult@1"),
        Some(vec![0xFA]),
        "receipt-driven continuation should raise domain event to result reducer"
    );
    let settled = world.kernel.workflow_instances_snapshot();
    let workflow = settled
        .iter()
        .find(|instance| instance.instance_id.starts_with("com.acme/Workflow@1::"))
        .expect("workflow instance snapshot after receipts");
    assert!(workflow.inflight_intents.is_empty());
    assert_eq!(workflow.status, WorkflowStatusSnapshot::Completed);

    let journal_entries = world.kernel.dump_journal().unwrap();
    let replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        no_plan_workflow_manifest(&store),
        Box::new(MemJournal::from_entries(&journal_entries)),
    )
    .unwrap();
    assert_eq!(
        replay_world
            .kernel
            .reducer_state("com.acme/WorkflowResult@1"),
        Some(vec![0xFA])
    );
    let replay_instances = replay_world.kernel.workflow_instances_snapshot();
    assert_eq!(replay_instances.len(), settled.len());
    let replay_workflow = replay_instances
        .iter()
        .find(|instance| instance.instance_id.starts_with("com.acme/Workflow@1::"))
        .expect("replayed workflow instance");
    assert!(replay_workflow.inflight_intents.is_empty());
    assert_eq!(replay_workflow.status, WorkflowStatusSnapshot::Completed);
}

#[test]
fn malformed_workflow_receipt_without_rejected_variant_fails_and_clears_pending() {
    let store = fixtures::new_mem_store();
    let manifest = no_plan_workflow_manifest(&store);
    let mut world = TestWorld::with_store(store, manifest).unwrap();

    let start_event = serde_json::json!({
        "$tag": "Start",
        "$value": fixtures::start_event("wf-1")
    });
    world
        .submit_event_result("com.acme/WorkflowEvent@1", &start_event)
        .expect("submit workflow start event");
    world.tick_n(1).unwrap();

    let effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 2);
    let blob_intent = effects
        .iter()
        .find(|intent| intent.kind.as_str() == aos_effects::EffectKind::BLOB_PUT)
        .expect("blob.put intent");
    let timer_intent = effects
        .iter()
        .find(|intent| intent.kind.as_str() == aos_effects::EffectKind::TIMER_SET)
        .expect("timer.set intent");

    let malformed = EffectReceipt {
        intent_hash: blob_intent.intent_hash,
        adapter_id: "adapter.blob".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: vec![0xa0], // {} does not satisfy sys/BlobPutReceipt@1
        cost_cents: None,
        signature: vec![],
    };
    world
        .kernel
        .handle_receipt(malformed)
        .expect("fault handled");

    let pending_after = world.kernel.pending_reducer_receipts_snapshot();
    assert!(
        pending_after.is_empty(),
        "failed workflow should not keep pending receipts"
    );
    let status = world
        .kernel
        .workflow_instances_snapshot()
        .into_iter()
        .find(|instance| instance.instance_id.starts_with("com.acme/Workflow@1::"))
        .expect("workflow instance")
        .status;
    assert_eq!(status, WorkflowStatusSnapshot::Failed);
    assert_eq!(
        world.kernel.reducer_state("com.acme/WorkflowResult@1"),
        None
    );

    let timer_receipt = EffectReceipt {
        intent_hash: timer_intent.intent_hash,
        adapter_id: "adapter.timer".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
            delivered_at_ns: 42,
            key: Some("wf".into()),
        })
        .unwrap(),
        cost_cents: Some(1),
        signature: vec![4, 5, 6],
    };
    world
        .kernel
        .handle_receipt(timer_receipt)
        .expect("late timer receipt ignored");
    world.kernel.tick_until_idle().unwrap();
}

#[test]
fn malformed_workflow_receipt_with_rejected_variant_delivers_event_and_continues() {
    let store = fixtures::new_mem_store();
    let manifest = no_plan_workflow_manifest_with_rejected_variant(&store);
    let mut world = TestWorld::with_store(store, manifest).unwrap();

    let start_event = serde_json::json!({
        "$tag": "Start",
        "$value": fixtures::start_event("wf-1")
    });
    world
        .submit_event_result("com.acme/WorkflowEvent@1", &start_event)
        .expect("submit workflow start event");
    world.tick_n(1).unwrap();

    let effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 2);
    let blob_intent = effects
        .iter()
        .find(|intent| intent.kind.as_str() == aos_effects::EffectKind::BLOB_PUT)
        .expect("blob.put intent");
    let timer_intent = effects
        .iter()
        .find(|intent| intent.kind.as_str() == aos_effects::EffectKind::TIMER_SET)
        .expect("timer.set intent");

    let malformed = EffectReceipt {
        intent_hash: blob_intent.intent_hash,
        adapter_id: "adapter.blob".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: vec![0xa0],
        cost_cents: None,
        signature: vec![],
    };
    world
        .kernel
        .handle_receipt(malformed)
        .expect("fault handled");

    let pending_after_rejected = world.kernel.pending_reducer_receipts_snapshot();
    assert_eq!(pending_after_rejected.len(), 1);
    assert!(
        pending_after_rejected
            .iter()
            .any(|entry| entry.intent_hash == timer_intent.intent_hash)
    );

    let timer_receipt = EffectReceipt {
        intent_hash: timer_intent.intent_hash,
        adapter_id: "adapter.timer".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
            delivered_at_ns: 42,
            key: Some("wf".into()),
        })
        .unwrap(),
        cost_cents: Some(1),
        signature: vec![4, 5, 6],
    };
    world.kernel.handle_receipt(timer_receipt).unwrap();
    world.kernel.tick_until_idle().unwrap();

    assert_eq!(
        world.kernel.reducer_state("com.acme/WorkflowResult@1"),
        Some(vec![0xFA]),
        "workflow should continue once valid receipts arrive"
    );
}

/// Policy decisions should be journaled for plan-origin effects.
#[test]
#[ignore = "P2: plan runtime path retired; replaced by workflow fixtures"]
fn policy_decision_is_journaled() {
    let store = fixtures::new_mem_store();
    let mut manifest = fulfillment_manifest(&store);
    let policy = aos_air_types::DefPolicy {
        name: "com.acme/allow-plan-http@1".into(),
        rules: vec![PolicyRule {
            when: PolicyMatch {
                effect_kind: Some(AirEffectKind::http_request()),
                origin_kind: Some(OriginKind::Workflow),
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

/// Cap decisions should include a stable grant hash in the journal.
#[test]
#[ignore = "P2: plan runtime path retired; replaced by workflow fixtures"]
fn cap_decision_includes_grant_hash() {
    let store = fixtures::new_mem_store();
    let manifest = fulfillment_manifest(&store);
    let mut world = TestWorld::with_store(store, manifest).unwrap();

    world
        .submit_event_result(START_SCHEMA, &serde_json::json!({ "id": "123" }))
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let journal_entries = world.kernel.dump_journal().unwrap();
    let record = journal_entries
        .iter()
        .find(|entry| entry.kind == JournalKind::CapDecision)
        .map(|entry| serde_cbor::from_slice::<JournalRecord>(&entry.payload).unwrap())
        .expect("cap decision entry missing");

    let decision = match record {
        JournalRecord::CapDecision(decision) => decision,
        _ => unreachable!("expected cap decision record"),
    };

    let params_cbor =
        aos_cbor::to_canonical_cbor(&CborValue::Map(BTreeMap::new())).expect("params cbor");
    let expected = compute_grant_hash("sys/http.out@1", "http.out", &params_cbor, None);
    assert_eq!(decision.grant_hash, expected);
}

fn no_plan_workflow_manifest(
    store: &Arc<fixtures::TestStore>,
) -> aos_kernel::manifest::LoadedManifest {
    no_plan_workflow_manifest_impl(store, false)
}

fn no_plan_workflow_manifest_with_rejected_variant(
    store: &Arc<fixtures::TestStore>,
) -> aos_kernel::manifest::LoadedManifest {
    no_plan_workflow_manifest_impl(store, true)
}

fn no_plan_workflow_manifest_impl(
    store: &Arc<fixtures::TestStore>,
    include_rejected_variant: bool,
) -> aos_kernel::manifest::LoadedManifest {
    let start_output = ReducerOutput {
        state: Some(vec![0xDE, 0xAD, 0xBE, 0xEF]),
        domain_events: vec![],
        effects: vec![
            ReducerEffect::new(
                aos_effects::EffectKind::TIMER_SET,
                serde_cbor::to_vec(&TimerSetParams {
                    deliver_at_ns: 42,
                    key: Some("wf".into()),
                })
                .unwrap(),
            ),
            ReducerEffect::with_cap_slot(
                aos_effects::EffectKind::BLOB_PUT,
                serde_cbor::to_vec(&BlobPutParams {
                    bytes: b"workflow".to_vec(),
                    blob_ref: None,
                    refs: None,
                })
                .unwrap(),
                "blob",
            ),
        ],
        ann: None,
    };
    let receipt_output = ReducerOutput {
        state: None,
        domain_events: vec![DomainEvent::new(
            "com.acme/WorkflowDone@1",
            serde_cbor::to_vec(&serde_json::json!({ "id": "wf-1" })).unwrap(),
        )],
        effects: vec![],
        ann: None,
    };

    let mut workflow =
        sequenced_reducer_module(store, "com.acme/Workflow@1", &start_output, &receipt_output);
    workflow.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/WorkflowState@1"),
        event: fixtures::schema("com.acme/WorkflowEvent@1"),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![
            aos_effects::EffectKind::TIMER_SET.into(),
            aos_effects::EffectKind::BLOB_PUT.into(),
        ],
        cap_slots: Default::default(),
    });

    let mut result = fixtures::stub_reducer_module(
        store,
        "com.acme/WorkflowResult@1",
        &ReducerOutput {
            state: Some(vec![0xFA]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    result.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/WorkflowResultState@1"),
        event: fixtures::schema("com.acme/WorkflowDone@1"),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    let mut loaded = fixtures::build_loaded_manifest(
        vec![],
        vec![],
        vec![workflow, result],
        vec![
            fixtures::routing_event("com.acme/WorkflowEvent@1", "com.acme/Workflow@1"),
            fixtures::routing_event("com.acme/WorkflowDone@1", "com.acme/WorkflowResult@1"),
        ],
    );
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            helpers::def_text_record_schema(START_SCHEMA, vec![("id", helpers::text_type())]),
            aos_air_types::DefSchema {
                name: "com.acme/WorkflowEvent@1".into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: {
                        let mut variant = indexmap::IndexMap::from([
                            (
                                "Start".into(),
                                TypeExpr::Ref(TypeRef {
                                    reference: fixtures::schema(START_SCHEMA),
                                }),
                            ),
                            (
                                "Receipt".into(),
                                TypeExpr::Ref(TypeRef {
                                    reference: fixtures::schema("sys/EffectReceiptEnvelope@1"),
                                }),
                            ),
                        ]);
                        if include_rejected_variant {
                            variant.insert(
                                "ReceiptRejected".into(),
                                TypeExpr::Ref(TypeRef {
                                    reference: fixtures::schema("sys/EffectReceiptRejected@1"),
                                }),
                            );
                        }
                        variant
                    },
                }),
            },
            helpers::def_text_record_schema(
                "com.acme/WorkflowDone@1",
                vec![("id", helpers::text_type())],
            ),
            aos_air_types::DefSchema {
                name: "com.acme/WorkflowState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: indexmap::IndexMap::new(),
                }),
            },
            aos_air_types::DefSchema {
                name: "com.acme/WorkflowResultState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: indexmap::IndexMap::new(),
                }),
            },
        ],
    );
    if let Some(binding) = loaded
        .manifest
        .module_bindings
        .get_mut("com.acme/Workflow@1")
    {
        binding.slots.insert("blob".into(), "blob_cap".into());
    }
    loaded
}

fn sequenced_reducer_module<S: Store + ?Sized>(
    store: &Arc<S>,
    name: impl Into<String>,
    first: &ReducerOutput,
    then: &ReducerOutput,
) -> DefModule {
    let first_bytes = first.encode().expect("encode first reducer output");
    let then_bytes = then.encode().expect("encode second reducer output");
    let first_literal = first_bytes
        .iter()
        .map(|b| format!("\\{:02x}", b))
        .collect::<String>();
    let then_literal = then_bytes
        .iter()
        .map(|b| format!("\\{:02x}", b))
        .collect::<String>();
    let first_len = first_bytes.len();
    let then_len = then_bytes.len();
    let second_offset = first_len;
    let heap_start = first_len + then_len;
    let wat = format!(
        r#"(module
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const {heap_start}))
  (data (i32.const 0) "{first_literal}")
  (data (i32.const {second_offset}) "{then_literal}")
  (func (export "alloc") (param i32) (result i32)
    (local $old i32)
    global.get $heap
    local.tee $old
    local.get 0
    i32.add
    global.set $heap
    local.get $old)
  (func $is_receipt_event (param $ptr i32) (param $len i32) (result i32)
    (local $i i32)
    (block $not_found
      (loop $search
        local.get $i
        i32.const 6
        i32.add
        local.get $len
        i32.ge_u
        br_if $not_found

        local.get $ptr
        local.get $i
        i32.add
        i32.load8_u
        i32.const 82
        i32.eq
        if
          local.get $ptr
          local.get $i
          i32.add
          i32.const 1
          i32.add
          i32.load8_u
          i32.const 101
          i32.eq
          if
            local.get $ptr
            local.get $i
            i32.add
            i32.const 2
            i32.add
            i32.load8_u
            i32.const 99
            i32.eq
            if
              local.get $ptr
              local.get $i
              i32.add
              i32.const 3
              i32.add
              i32.load8_u
              i32.const 101
              i32.eq
              if
                local.get $ptr
                local.get $i
                i32.add
                i32.const 4
                i32.add
                i32.load8_u
                i32.const 105
                i32.eq
                if
                  local.get $ptr
                  local.get $i
                  i32.add
                  i32.const 5
                  i32.add
                  i32.load8_u
                  i32.const 112
                  i32.eq
                  if
                    local.get $ptr
                    local.get $i
                    i32.add
                    i32.const 6
                    i32.add
                    i32.load8_u
                    i32.const 116
                    i32.eq
                    if
                      i32.const 1
                      return
                    end
                  end
                end
              end
            end
          end
        end

        local.get $i
        i32.const 1
        i32.add
        local.set $i
        br $search
      )
    )
    i32.const 0
  )
  (func (export "step") (param i32 i32) (result i32 i32)
    local.get 0
    local.get 1
    call $is_receipt_event
    if (result i32 i32)
      (i32.const {second_offset})
      (i32.const {then_len})
    else
      (i32.const 0)
      (i32.const {first_len})
    end)
)"#
    );

    let wasm_bytes = parse_str(&wat).expect("compile sequenced reducer wat");
    let wasm_hash = store.put_blob(&wasm_bytes).expect("store reducer wasm");
    let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).expect("hash ref");
    DefModule {
        name: name.into(),
        module_kind: ModuleKind::Workflow,
        wasm_hash: wasm_hash_ref,
        key_schema: None,
        abi: ModuleAbi {
            reducer: None,
            pure: None,
        },
    }
}

fn compute_grant_hash(
    defcap_ref: &str,
    cap_type: &str,
    params_cbor: &[u8],
    expiry_ns: Option<u64>,
) -> [u8; 32] {
    let mut map = BTreeMap::new();
    map.insert(
        CborValue::Text("defcap_ref".into()),
        CborValue::Text(defcap_ref.into()),
    );
    map.insert(
        CborValue::Text("cap_type".into()),
        CborValue::Text(cap_type.into()),
    );
    map.insert(
        CborValue::Text("params_cbor".into()),
        CborValue::Bytes(params_cbor.to_vec()),
    );
    if let Some(expiry) = expiry_ns {
        map.insert(
            CborValue::Text("expiry_ns".into()),
            CborValue::Integer(expiry as i128),
        );
    }
    let hash = aos_cbor::Hash::of_cbor(&CborValue::Map(map)).expect("grant hash");
    *hash.as_bytes()
}

/// FsJournal should persist entries to disk and allow a fresh kernel to resume state.
#[test]
#[ignore = "P2: plan runtime path retired; replaced by workflow fixtures"]
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

        let mut effects = world.drain_effects().expect("drain effects");
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

/// Trace terminal classification derived from journal + live wait state should match after replay.
#[test]
#[ignore = "P2: plan runtime path retired; replaced by workflow fixtures"]
fn trace_terminal_classification_matches_after_replay() {
    let store = fixtures::new_mem_store();
    let manifest = fulfillment_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    world
        .submit_event_result(START_SCHEMA, &serde_json::json!({ "id": "trace-parity" }))
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let journal_entries = world.kernel.dump_journal().unwrap();
    let event_hash = journal_entries
        .iter()
        .find(|entry| entry.kind == JournalKind::DomainEvent)
        .and_then(|entry| serde_cbor::from_slice::<JournalRecord>(&entry.payload).ok())
        .and_then(|record| match record {
            JournalRecord::DomainEvent(event) => Some(event.event_hash),
            _ => None,
        })
        .expect("domain event hash missing from journal");
    assert!(
        !event_hash.is_empty(),
        "domain event hash should be populated for trace classification"
    );

    let original_terminal = classify_trace_terminal_state(&world.kernel, &event_hash)
        .expect("classify terminal state before replay");

    let replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        fulfillment_manifest(&store),
        Box::new(MemJournal::from_entries(&journal_entries)),
    )
    .unwrap();

    let replay_terminal = classify_trace_terminal_state(&replay_world.kernel, &event_hash)
        .expect("classify terminal state after replay");

    assert_eq!(
        original_terminal, replay_terminal,
        "terminal trace classification should be replay-stable"
    );
    assert_eq!(
        original_terminal, "waiting_receipt",
        "fixture should be pending receipt before external adapter response"
    );
}

fn classify_trace_terminal_state(
    kernel: &aos_kernel::Kernel<helpers::fixtures::TestStore>,
    event_hash: &str,
) -> Option<&'static str> {
    let entries = kernel.dump_journal().ok()?;
    let root_seq = entries.iter().find_map(|entry| {
        if entry.kind != JournalKind::DomainEvent {
            return None;
        }
        let record = serde_cbor::from_slice::<JournalRecord>(&entry.payload).ok()?;
        match record {
            JournalRecord::DomainEvent(event) if event.event_hash == event_hash => Some(entry.seq),
            _ => None,
        }
    })?;

    let mut has_receipt_error = false;
    let mut has_plan_error = false;
    let mut has_window_entries = false;
    for entry in entries.into_iter().filter(|entry| entry.seq >= root_seq) {
        let record = serde_cbor::from_slice::<JournalRecord>(&entry.payload).ok()?;
        has_window_entries = true;
        if let JournalRecord::EffectReceipt(receipt) = &record {
            if !matches!(receipt.status, ReceiptStatus::Ok) {
                has_receipt_error = true;
            }
        }
        if let JournalRecord::PlanEnded(ended) = &record {
            if matches!(ended.status, aos_kernel::journal::PlanEndStatus::Error) {
                has_plan_error = true;
            }
        }
    }

    let waiting_receipt_count = kernel.pending_plan_receipts().len()
        + kernel.pending_reducer_receipts_snapshot().len()
        + kernel.queued_effects_snapshot().len()
        + kernel
            .debug_plan_waits()
            .iter()
            .map(|(_, waits)| waits.len())
            .sum::<usize>();
    let waiting_event_count = kernel.debug_plan_waiting_events().len();

    Some(if has_receipt_error || has_plan_error {
        "failed"
    } else if waiting_receipt_count > 0 {
        "waiting_receipt"
    } else if waiting_event_count > 0 {
        "waiting_event"
    } else if has_window_entries {
        "completed"
    } else {
        "unknown"
    })
}
