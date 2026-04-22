use aos_air_types::{
    DefModule, DefSchema, HashRef, ModuleRuntime, TypeExpr, TypeRecord, TypeRef, TypeVariant,
    WasmArtifact,
};
use aos_effects::builtins::{BlobPutParams, TimerSetParams, TimerSetReceipt};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_kernel::Kernel;
use aos_kernel::Store;
use aos_kernel::journal::Journal;
use aos_kernel::journal::JournalKind;
use aos_wasm_abi::{WorkflowEffect, WorkflowOutput};
use std::sync::Arc;
use wat::parse_str;

use helpers::fixtures::{self, START_SCHEMA, TestWorld, WorkflowAbi};

#[path = "support/helpers.rs"]
mod helpers;
use helpers::{simple_state_manifest, timer_manifest};

#[test]
fn workflow_timer_snapshot_resumes_on_receipt() {
    let store = fixtures::new_mem_store();
    let manifest = timer_manifest(&store);
    let replay_manifest = manifest.clone();
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
        replay_manifest,
        Journal::from_entries(&entries).unwrap(),
    )
    .unwrap();

    let receipt = EffectReceipt {
        intent_hash: effect.intent_hash,
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
        replay_world
            .kernel
            .workflow_state("com.acme/TimerEmitter@1"),
        Some(vec![0x01])
    );
}

#[test]
fn workflow_snapshot_preserves_effect_queue() {
    let store = fixtures::new_mem_store();
    let manifest = workflow_resume_manifest(&store);
    let replay_manifest = manifest.clone();
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let start_event = serde_json::json!({
        "$tag": "Start",
        "$value": fixtures::start_event("queue")
    });
    world
        .submit_event_result("com.acme/WorkflowResumeEvent@1", &start_event)
        .expect("submit start event");
    world.tick_n(1).unwrap();

    world.kernel.create_snapshot().unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let mut replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        replay_manifest,
        Journal::from_entries(&entries).unwrap(),
    )
    .unwrap();
    let intents = replay_world.drain_effects().expect("drain effects");
    assert_eq!(intents.len(), 1);
    assert_eq!(
        intents[0].effect_op.as_str(),
        aos_effects::effect_ops::TIMER_SET
    );
}

#[test]
fn workflow_receipt_wait_survives_restart_and_resumes_continuation() {
    let store = fixtures::new_mem_store();
    let manifest = workflow_resume_manifest(&store);
    let replay_manifest = manifest.clone();
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
        initial_intent.effect_op.as_str(),
        aos_effects::effect_ops::TIMER_SET
    );

    world.kernel.create_snapshot().unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let mut replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        replay_manifest,
        Journal::from_entries(&entries).unwrap(),
    )
    .unwrap();

    let mut replay_queued = replay_world.drain_effects().expect("drain replay queue");
    assert_eq!(replay_queued.len(), 1);
    assert_eq!(
        replay_queued.remove(0).effect_op.as_str(),
        aos_effects::effect_ops::TIMER_SET
    );

    let receipt = EffectReceipt {
        intent_hash: initial_intent.intent_hash,
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
        resumed.remove(0).effect_op.as_str(),
        aos_effects::effect_ops::BLOB_PUT
    );
}

#[test]
fn workflow_authorized_effect_snapshot_replay_has_no_cap_decisions() {
    let store = fixtures::new_mem_store();
    let manifest = workflow_resume_manifest(&store);
    let replay_manifest = manifest.clone();
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let start_event = serde_json::json!({
        "$tag": "Start",
        "$value": fixtures::start_event("cap")
    });
    world
        .submit_event_result("com.acme/WorkflowResumeEvent@1", &start_event)
        .expect("submit start");
    world.tick_n(1).unwrap();

    world.kernel.create_snapshot().unwrap();
    let entries = world.kernel.dump_journal().unwrap();
    let mut replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        replay_manifest,
        Journal::from_entries(&entries).unwrap(),
    )
    .unwrap();
    let intents = replay_world.drain_effects().expect("drain effects");
    assert_eq!(intents.len(), 1);
    assert_eq!(
        intents[0].effect_op.as_str(),
        aos_effects::effect_ops::TIMER_SET
    );
}

#[test]
fn snapshot_replay_restores_state() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let replay_manifest = manifest.clone();
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("simple"))
        .expect("submit start event");
    world.tick_n(1).unwrap();

    world.kernel.create_snapshot().unwrap();

    let final_state = world.kernel.workflow_state("com.acme/Simple@1").unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        replay_manifest,
        Journal::from_entries(&entries).unwrap(),
    )
    .unwrap();

    assert_eq!(
        replay_world.kernel.workflow_state("com.acme/Simple@1"),
        Some(final_state)
    );
}

#[test]
fn snapshot_creation_quiesces_runtime() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let replay_manifest = manifest.clone();
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("quiesce"))
        .expect("submit start event");
    world.kernel.create_snapshot().unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        replay_manifest,
        Journal::from_entries(&entries).unwrap(),
    )
    .unwrap();

    assert_eq!(
        replay_world.kernel.workflow_state("com.acme/Simple@1"),
        Some(vec![0xAA])
    );
}

#[test]
fn workflow_manifest_records_restore_queued_intent_without_policy_reevaluation() {
    let store = fixtures::new_mem_store();
    let manifest = workflow_resume_manifest(&store);
    let replay_manifest = manifest.clone();
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let start_event = serde_json::json!({
        "$tag": "Start",
        "$value": fixtures::start_event("policy")
    });
    world
        .submit_event_result("com.acme/WorkflowResumeEvent@1", &start_event)
        .expect("submit start event");
    world.tick_n(1).unwrap();

    world.kernel.create_snapshot().unwrap();
    let entries = world.kernel.dump_journal().unwrap();

    let mut replay_world = TestWorld::with_store_and_journal(
        store.clone(),
        replay_manifest,
        Journal::from_entries(&entries).unwrap(),
    )
    .unwrap();

    let mut intents = replay_world.drain_effects().expect("drain effects");
    assert_eq!(
        intents.len(),
        1,
        "restored queued intent should bypass replay-time policy reevaluation"
    );
    let effect = intents.remove(0);
    assert_eq!(
        effect.effect_op.as_str(),
        aos_effects::effect_ops::TIMER_SET
    );

    let receipt = EffectReceipt {
        intent_hash: effect.intent_hash,
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
            delivered_at_ns: 23,
            key: Some("resume".into()),
        })
        .unwrap(),
        cost_cents: None,
        signature: vec![],
    };
    replay_world.kernel.handle_receipt(receipt).unwrap();
    replay_world.kernel.tick_until_idle().unwrap();

    let mut followups = replay_world.drain_effects().expect("drain followups");
    assert_eq!(followups.len(), 1);
    assert_eq!(
        followups.remove(0).effect_op.as_str(),
        aos_effects::effect_ops::BLOB_PUT
    );
}

fn workflow_resume_manifest(
    store: &Arc<helpers::fixtures::TestStore>,
) -> aos_kernel::manifest::LoadedManifest {
    let start_output = WorkflowOutput {
        state: Some(vec![0x51]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::new(
            "sys/timer.set@1",
            serde_cbor::to_vec(&TimerSetParams {
                deliver_at_ns: 5,
                key: Some("resume".into()),
            })
            .unwrap(),
        )],
        ann: None,
    };
    let receipt_output = WorkflowOutput {
        state: Some(vec![0x52]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::new(
            "sys/blob.put@1",
            serde_cbor::to_vec(&BlobPutParams {
                bytes: b"resumed".to_vec(),
                blob_ref: None,
                refs: None,
            })
            .unwrap(),
        )],
        ann: None,
    };
    let mut workflow = sequenced_workflow_module(
        store,
        "com.acme/WorkflowResume@1",
        &start_output,
        &receipt_output,
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/WorkflowResumeState@1"),
        event: fixtures::schema("com.acme/WorkflowResumeEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![
            aos_effects::effect_ops::TIMER_SET.into(),
            aos_effects::effect_ops::BLOB_PUT.into(),
        ],
    });

    let mut loaded = fixtures::build_loaded_manifest(
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
}

fn sequenced_workflow_module<S: Store + ?Sized>(
    store: &Arc<S>,
    name: impl Into<String>,
    first: &WorkflowOutput,
    then: &WorkflowOutput,
) -> fixtures::TestModule {
    let name = name.into();
    let first_bytes = first.encode().expect("encode first workflow output");
    let then_bytes = then.encode().expect("encode second workflow output");
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

    let wasm_bytes = parse_str(&wat).expect("compile sequenced workflow wat");
    let wasm_hash = store.put_blob(&wasm_bytes).expect("store workflow wasm");
    let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).expect("hash ref");
    fixtures::TestModule {
        name: name.clone(),
        module: DefModule {
            name,
            runtime: ModuleRuntime::Wasm {
                artifact: WasmArtifact::WasmModule {
                    hash: wasm_hash_ref,
                },
            },
        },
        key_schema: None,
        abi: fixtures::ModuleAbi { workflow: None },
    }
}
