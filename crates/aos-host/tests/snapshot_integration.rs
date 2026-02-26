use aos_air_exec::Value as ExprValue;
use aos_air_types::{
    DefModule, DefPolicy, DefSchema, EffectKind as AirEffectKind, HashRef, ModuleAbi, ModuleKind,
    OriginKind, PolicyDecision, PolicyMatch, PolicyRule, ReducerAbi, TypeExpr, TypeRecord, TypeRef,
    TypeVariant,
};
use aos_effects::builtins::{BlobPutParams, TimerSetParams, TimerSetReceipt};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_kernel::journal::fs::FsJournal;
use aos_kernel::journal::mem::MemJournal;
use aos_kernel::journal::JournalKind;
use aos_kernel::Kernel;
use aos_store::{FsStore, Store};
use aos_wasm_abi::{ReducerEffect, ReducerOutput};
use helpers::fixtures::{self, TestWorld, START_SCHEMA};
use serde_cbor;
use std::sync::Arc;
use tempfile::TempDir;
use wat::parse_str;

mod helpers;
use helpers::{
    attach_default_policy, await_event_manifest, fulfillment_manifest, simple_state_manifest,
    timer_manifest,
};

fn deny_plan_http_policy() -> DefPolicy {
    DefPolicy {
        name: "com.acme/deny-plan-http@1".into(),
        rules: vec![PolicyRule {
            when: PolicyMatch {
                effect_kind: Some(AirEffectKind::http_request()),
                origin_kind: Some(OriginKind::Workflow),
                ..Default::default()
            },
            decision: PolicyDecision::Deny,
        }],
    }
}

/// Plan execution should resume correctly after snapshot when awaiting a receipt.
#[test]
#[ignore = "P2: plan-runtime snapshot path retired; replace with workflow-native fixture"]
fn plan_snapshot_resumes_after_receipt() {
    let store = fixtures::new_mem_store();
    let manifest = fulfillment_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let input = fixtures::start_event("123");
    world
        .submit_event_result(START_SCHEMA, &input)
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let effect = world
        .drain_effects()
        .expect("drain effects")
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
        Some(vec![0xEE])
    );
}

/// Effect intents should persist in the queue across snapshot/restore.
#[test]
#[ignore = "P2: plan-runtime snapshot path retired; replace with workflow-native fixture"]
fn plan_snapshot_preserves_effect_queue() {
    let store = fixtures::new_mem_store();
    let manifest = fulfillment_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let input = fixtures::start_event("123");
    world
        .submit_event_result(START_SCHEMA, &input)
        .expect("submit start event");
    world.tick_n(2).unwrap();

    world.kernel.create_snapshot().unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let mut replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        fulfillment_manifest(&store),
        Box::new(MemJournal::from_entries(&entries)),
    )
    .unwrap();

    let mut intents = replay_world.drain_effects().expect("drain effects");
    assert_eq!(
        intents.len(),
        1,
        "effect queue should persist across snapshot"
    );
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
        Some(vec![0xEE])
    );
}

/// Plan awaiting domain event should resume correctly after snapshot.
#[test]
#[ignore = "P2: plan-runtime snapshot path retired; replace with workflow-native fixture"]
fn plan_snapshot_resumes_after_event() {
    let store = fixtures::new_mem_store();
    let manifest = await_event_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let input = fixtures::start_event("evt");
    world
        .submit_event_result(START_SCHEMA, &input)
        .expect("submit start event");
    world.tick_n(2).unwrap();
    world.tick_n(2).unwrap();

    world.kernel.create_snapshot().unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let mut replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        await_event_manifest(&store),
        Box::new(MemJournal::from_entries(&entries)),
    )
    .unwrap();

    replay_world
        .submit_event_result("com.acme/EmitUnblock@1", &serde_json::json!({}))
        .expect("submit unblock event");
    replay_world.kernel.tick_until_idle().unwrap();

    assert_eq!(
        replay_world.kernel.reducer_state("com.acme/EventResult@1"),
        Some(vec![0xAB])
    );
}

/// Reducer-emitted timer effects should resume correctly on receipt after snapshot.
#[test]
fn reducer_timer_snapshot_resumes_on_receipt() {
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
        replay_world.kernel.reducer_state("com.acme/TimerEmitter@1"),
        Some(vec![0x01])
    );
}

#[test]
fn workflow_receipt_wait_survives_restart_and_resumes_continuation() {
    let store = fixtures::new_mem_store();
    let manifest = workflow_resume_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let start_event = serde_json::json!({
        "$tag": "Start",
        "$value": fixtures::start_event("resume-1")
    });
    world
        .submit_event_result("com.acme/WorkflowResumeEvent@1", &start_event)
        .expect("submit start event");
    world.tick_n(1).unwrap();

    let mut queued = world.drain_effects().expect("drain initial queue");
    assert_eq!(queued.len(), 1);
    let initial_intent = queued.remove(0);
    assert_eq!(
        initial_intent.kind.as_str(),
        aos_effects::EffectKind::TIMER_SET
    );

    world.kernel.create_snapshot().unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let mut replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        workflow_resume_manifest(&store),
        Box::new(MemJournal::from_entries(&entries)),
    )
    .unwrap();

    let mut replay_queued = replay_world.drain_effects().expect("drain replay queue");
    assert_eq!(replay_queued.len(), 1);
    assert_eq!(
        replay_queued.remove(0).kind.as_str(),
        aos_effects::EffectKind::TIMER_SET
    );

    let receipt = EffectReceipt {
        intent_hash: initial_intent.intent_hash,
        adapter_id: "adapter.timer".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
            delivered_at_ns: 15,
            key: Some("resume".into()),
        })
        .unwrap(),
        cost_cents: None,
        signature: vec![],
    };
    replay_world.kernel.handle_receipt(receipt).unwrap();
    replay_world.kernel.tick_until_idle().unwrap();

    let mut resumed = replay_world.drain_effects().expect("drain resumed queue");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        resumed.remove(0).kind.as_str(),
        aos_effects::EffectKind::BLOB_PUT
    );
}

/// Cap decisions should be preserved in journals across snapshot/replay cycles.
#[test]
#[ignore = "P2: plan-runtime snapshot fixture retired; replace with workflow-native cap policy coverage"]
fn cap_decisions_survive_snapshot_replay() {
    let store = fixtures::new_mem_store();
    let manifest = fulfillment_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("123"))
        .expect("submit start event");
    world.tick_n(2).unwrap();

    world.kernel.create_snapshot().unwrap();
    let entries = world.kernel.dump_journal().unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.kind == JournalKind::CapDecision),
        "expected cap decision entry"
    );

    let mut replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        fulfillment_manifest(&store),
        Box::new(MemJournal::from_entries(&entries)),
    )
    .unwrap();

    let intents = replay_world.drain_effects().expect("drain effects");
    assert_eq!(intents.len(), 1, "effect queue should survive replay");
}

/// Simple snapshot/restore without any in-flight effects should restore reducer state.
#[test]
fn snapshot_replay_restores_state() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("simple"))
        .expect("submit start event");
    world.tick_n(1).unwrap();

    world.kernel.create_snapshot().unwrap();

    let final_state = world.kernel.reducer_state("com.acme/Simple@1").unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        simple_state_manifest(&store),
        Box::new(MemJournal::from_entries(&entries)),
    )
    .unwrap();

    assert_eq!(
        replay_world.kernel.reducer_state("com.acme/Simple@1"),
        Some(final_state)
    );
}

/// Snapshot blobs persisted via FsStore plus FsJournal should restore after a fresh process.
#[test]
fn fs_store_and_journal_restore_snapshot() {
    let store_dir = TempDir::new().unwrap();
    let journal_dir = TempDir::new().unwrap();
    let store = Arc::new(FsStore::open(store_dir.path()).unwrap());

    let manifest = fs_persistent_manifest(&store);
    let journal = FsJournal::open(journal_dir.path()).unwrap();
    let mut kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(journal)).unwrap();

    let event_bytes = serde_cbor::to_vec(&serde_json::json!({ "id": "fs" })).unwrap();
    kernel
        .submit_domain_event(START_SCHEMA.to_string(), event_bytes)
        .expect("submit domain event");
    kernel.tick_until_idle().unwrap();
    kernel.create_snapshot().unwrap();

    drop(kernel);

    let manifest_reload = fs_persistent_manifest(&store);
    let journal_reload = FsJournal::open(journal_dir.path()).unwrap();
    let kernel_replay =
        Kernel::from_loaded_manifest(store.clone(), manifest_reload, Box::new(journal_reload))
            .unwrap();

    assert_eq!(
        kernel_replay.reducer_state("com.acme/SimpleFs@1"),
        Some(vec![0xAA])
    );
}

fn workflow_resume_manifest(
    store: &Arc<helpers::fixtures::TestStore>,
) -> aos_kernel::manifest::LoadedManifest {
    let start_output = ReducerOutput {
        state: Some(vec![0x51]),
        domain_events: vec![],
        effects: vec![ReducerEffect::new(
            aos_effects::EffectKind::TIMER_SET,
            serde_cbor::to_vec(&TimerSetParams {
                deliver_at_ns: 5,
                key: Some("resume".into()),
            })
            .unwrap(),
        )],
        ann: None,
    };
    let receipt_output = ReducerOutput {
        state: Some(vec![0x52]),
        domain_events: vec![],
        effects: vec![ReducerEffect::with_cap_slot(
            aos_effects::EffectKind::BLOB_PUT,
            serde_cbor::to_vec(&BlobPutParams {
                bytes: b"resumed".to_vec(),
                blob_ref: None,
                refs: None,
            })
            .unwrap(),
            "blob",
        )],
        ann: None,
    };
    let mut workflow = sequenced_reducer_module(
        store,
        "com.acme/WorkflowResume@1",
        &start_output,
        &receipt_output,
    );
    workflow.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/WorkflowResumeState@1"),
        event: fixtures::schema("com.acme/WorkflowResumeEvent@1"),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![
            aos_effects::EffectKind::TIMER_SET.into(),
            aos_effects::EffectKind::BLOB_PUT.into(),
        ],
        cap_slots: Default::default(),
    });

    let mut loaded = fixtures::build_loaded_manifest(
        vec![],
        vec![],
        vec![workflow],
        vec![fixtures::routing_event(
            "com.acme/WorkflowResumeEvent@1",
            "com.acme/WorkflowResume@1",
        )],
    );
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            fixtures::def_text_record_schema(START_SCHEMA, vec![("id", fixtures::text_type())]),
            DefSchema {
                name: "com.acme/WorkflowResumeEvent@1".into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: indexmap::IndexMap::from([
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
                    ]),
                }),
            },
            DefSchema {
                name: "com.acme/WorkflowResumeState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: indexmap::IndexMap::new(),
                }),
            },
        ],
    );
    loaded
        .manifest
        .module_bindings
        .get_mut("com.acme/WorkflowResume@1")
        .expect("workflow binding")
        .slots
        .insert("blob".into(), "blob_cap".into());
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

fn fs_persistent_manifest(store: &Arc<FsStore>) -> aos_kernel::manifest::LoadedManifest {
    let mut reducer = fixtures::stub_reducer_module(
        store,
        "com.acme/SimpleFs@1",
        &ReducerOutput {
            state: Some(vec![0xAA]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    reducer.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/SimpleFsState@1"),
        event: fixtures::schema(START_SCHEMA),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let routing = vec![fixtures::routing_event(START_SCHEMA, &reducer.name)];
    let mut loaded = fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing);
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            fixtures::def_text_record_schema(START_SCHEMA, vec![("id", fixtures::text_type())]),
            DefSchema {
                name: "com.acme/SimpleFsState@1".into(),
                ty: fixtures::text_type(),
            },
        ],
    );
    loaded
}

/// Snapshot creation should automatically drain pending scheduler work before persisting state.
#[test]
fn snapshot_creation_quiesces_runtime() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("quiesce"))
        .expect("submit start event");
    // No manual ticks before snapshot; create_snapshot should quiesce the runtime.
    world.kernel.create_snapshot().unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        simple_state_manifest(&store),
        Box::new(MemJournal::from_entries(&entries)),
    )
    .unwrap();

    assert_eq!(
        replay_world.kernel.reducer_state("com.acme/Simple@1"),
        Some(vec![0xAA])
    );
}

/// Manifest records in the journal override the supplied manifest on replay.
#[test]
#[ignore = "P2: plan-runtime snapshot fixture retired; replace with workflow-native manifest replay coverage"]
fn manifest_records_override_supplied_policy() {
    let store = fixtures::new_mem_store();
    let manifest = fulfillment_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let input = serde_json::json!({ "id": "first" });
    world
        .submit_event_result(START_SCHEMA, &input)
        .expect("submit start event");
    world.tick_n(2).unwrap();

    world.kernel.create_snapshot().unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let mut denying_manifest = fulfillment_manifest(&store);
    attach_default_policy(&mut denying_manifest, deny_plan_http_policy());

    let mut replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        denying_manifest,
        Box::new(MemJournal::from_entries(&entries)),
    )
    .unwrap();

    let mut intents = replay_world.drain_effects().expect("drain effects");
    assert_eq!(
        intents.len(),
        1,
        "restored intent queue should bypass policy re-check"
    );
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
        Some(vec![0xEE])
    );

    // New plan attempts are still allowed because the journal manifest is authoritative.
    replay_world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("blocked"))
        .expect("submit blocked start event");
    replay_world.kernel.tick_until_idle().unwrap();
}
