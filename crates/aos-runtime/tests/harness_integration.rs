#[path = "helpers.rs"]
mod helpers;

use std::sync::Arc;

use aos_air_types::{
    DefModule, DefSchema, HashRef, ModuleAbi, ModuleKind, TypeExpr, TypeRecord, TypeRef,
    TypeVariant, WorkflowAbi,
};
use aos_effects::ReceiptStatus;
use aos_effects::builtins::{
    BlobGetReceipt, BlobPutReceipt, HttpRequestReceipt, LlmFinishReason, LlmGenerateReceipt,
    TimerSetParams, TimerSetReceipt, TokenUsage,
};
use aos_kernel::Store;
use aos_runtime::{EffectMode, HarnessBuilder};
use aos_wasm_abi::{WorkflowEffect, WorkflowOutput};
use helpers::fixtures::{self, START_SCHEMA, TestStore};
use indexmap::IndexMap;
use wat::parse_str;

fn timer_workflow_manifest(
    store: &Arc<TestStore>,
    deliver_at_ns: u64,
) -> aos_kernel::manifest::LoadedManifest {
    let start_output = WorkflowOutput {
        state: Some(vec![0x01]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::new(
            aos_effects::EffectKind::TIMER_SET,
            serde_cbor::to_vec(&TimerSetParams {
                deliver_at_ns,
                key: Some("retry".into()),
            })
            .unwrap(),
        )],
        ann: None,
    };
    let receipt_output = WorkflowOutput {
        state: Some(vec![0xCC]),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };
    let mut workflow = sequenced_workflow_module(
        store,
        "com.acme/TimerWorkflow@1",
        &start_output,
        &receipt_output,
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/TimerWorkflowState@1"),
        event: fixtures::schema("com.acme/TimerWorkflowEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::TIMER_SET.into()],
        cap_slots: Default::default(),
    });

    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow],
        vec![fixtures::routing_event(
            "com.acme/TimerWorkflowEvent@1",
            "com.acme/TimerWorkflow@1",
        )],
    );
    helpers::insert_test_schemas(
        &mut loaded,
        vec![
            helpers::def_text_record_schema(START_SCHEMA, vec![("id", helpers::text_type())]),
            DefSchema {
                name: "com.acme/TimerWorkflowEvent@1".into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
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
                name: "com.acme/TimerWorkflowState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
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
) -> DefModule {
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
                    i32.const 1
                    return
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
        br $search))
    i32.const 0)
  (func (export "step") (param $ptr i32) (param $len i32) (result i32 i32)
    local.get $ptr
    local.get $len
    call $is_receipt_event
    if (result i32 i32)
      i32.const {second_offset}
      i32.const {then_len}
    else
      i32.const 0
      i32.const {first_len}
    end))"#
    );
    let wasm_bytes = parse_str(&wat).expect("wat compile");
    let wasm_hash = store.put_blob(&wasm_bytes).expect("store wasm");
    let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).expect("hash ref");

    DefModule {
        name: name.into(),
        module_kind: ModuleKind::Workflow,
        wasm_hash: wasm_hash_ref,
        key_schema: None,
        abi: ModuleAbi {
            workflow: None,
            pure: None,
        },
    }
}

#[test]
fn world_harness_scripted_effect_flow_tracks_pending_receipts() {
    let store = fixtures::new_mem_store();
    let loaded = timer_workflow_manifest(&store, 1_000_000_000);
    let mut harness = HarnessBuilder::ephemeral(store, loaded)
        .effect_mode(EffectMode::Scripted)
        .build_world()
        .unwrap();

    harness
        .send_event(
            "com.acme/TimerWorkflowEvent@1",
            serde_json::json!({
                "$tag": "Start",
                "$value": fixtures::start_event("scripted"),
            }),
        )
        .unwrap();
    let status = harness.run_until_kernel_idle().unwrap();
    assert!(status.kernel.kernel_idle);
    assert_eq!(status.kernel.queued_effects, 1);
    assert_eq!(status.kernel.pending_workflow_receipts, 1);
    assert!(!status.runtime_quiescent);

    let effects = harness.pull_effects().unwrap();
    assert_eq!(effects.len(), 1);
    let receipt = harness
        .core()
        .receipt_timer_set_ok(effects[0].intent_hash, 1_000_000_000, Some("retry".into()))
        .unwrap();
    harness.apply_receipt(receipt).unwrap();
    harness.run_until_kernel_idle().unwrap();

    assert_eq!(
        harness
            .state_bytes("com.acme/TimerWorkflow@1", None)
            .unwrap(),
        vec![0xCC]
    );
}

#[test]
fn world_harness_time_jump_fires_next_due_timer() {
    let store = fixtures::new_mem_store();
    let loaded = timer_workflow_manifest(&store, 1_000_000_000);
    let mut harness = HarnessBuilder::ephemeral(store, loaded)
        .effect_mode(EffectMode::Twin)
        .build_world()
        .unwrap();
    harness.time_set(0);

    harness
        .send_event(
            "com.acme/TimerWorkflowEvent@1",
            serde_json::json!({
                "$tag": "Start",
                "$value": fixtures::start_event("timers"),
            }),
        )
        .unwrap();
    let cycle = harness.run_cycle_with_timers().unwrap();
    assert_eq!(cycle.effects_dispatched, 1);
    assert_eq!(cycle.receipts_applied, 0);

    let status = harness.quiescence_status();
    assert_eq!(status.timers_pending, 1);
    assert_eq!(status.next_timer_deadline_ns, Some(1_000_000_000));
    assert!(!status.runtime_quiescent);

    let jumped = harness.time_jump_next_due().unwrap();
    assert_eq!(jumped, Some(1_000_000_000));
    assert_eq!(
        harness
            .state_bytes("com.acme/TimerWorkflow@1", None)
            .unwrap(),
        vec![0xCC]
    );

    let status = harness.quiescence_status();
    assert_eq!(status.timers_pending, 0);
    assert!(status.runtime_quiescent);
}

#[test]
fn world_harness_reopen_preserves_pending_timer_without_requeueing_intent() {
    let store = fixtures::new_mem_store();
    let loaded = timer_workflow_manifest(&store, 1_000_000_000);
    let mut harness = HarnessBuilder::ephemeral(store, loaded)
        .effect_mode(EffectMode::Twin)
        .build_world()
        .unwrap();
    harness.time_set(0);

    harness
        .send_event(
            "com.acme/TimerWorkflowEvent@1",
            serde_json::json!({
                "$tag": "Start",
                "$value": fixtures::start_event("reopen"),
            }),
        )
        .unwrap();
    let cycle = harness.run_cycle_with_timers().unwrap();
    assert_eq!(cycle.effects_dispatched, 1);
    assert_eq!(cycle.receipts_applied, 0);
    assert_eq!(harness.quiescence_status().kernel.queued_effects, 0);
    assert_eq!(harness.quiescence_status().timers_pending, 1);

    let mut reopened = harness.reopen().unwrap();
    let reopened_status = reopened.quiescence_status();
    assert_eq!(reopened_status.kernel.queued_effects, 0);
    assert_eq!(reopened_status.timers_pending, 1);
    assert!(!reopened_status.runtime_quiescent);

    let jumped = reopened.time_jump_next_due().unwrap();
    assert_eq!(jumped, Some(1_000_000_000));
    assert_eq!(
        reopened
            .state_bytes("com.acme/TimerWorkflow@1", None)
            .unwrap(),
        vec![0xCC]
    );

    let final_status = reopened.quiescence_status();
    assert_eq!(final_status.kernel.queued_effects, 0);
    assert_eq!(final_status.timers_pending, 0);
    assert!(final_status.runtime_quiescent);
}

#[test]
fn workflow_harness_scopes_state_and_cells() {
    let store = fixtures::new_mem_store();
    let loaded = helpers::simple_state_manifest(&store);
    let mut harness = HarnessBuilder::ephemeral(store, loaded)
        .build_workflow("com.acme/Simple@1")
        .unwrap();

    harness
        .send_event(START_SCHEMA, fixtures::start_event("simple"))
        .unwrap();
    harness.run_cycle_batch().unwrap();

    assert_eq!(harness.state_bytes(None).unwrap(), vec![0xAA]);
    assert_eq!(harness.list_cells().unwrap().len(), 1);
}

#[test]
fn world_harness_reopen_replay_and_export_artifacts_work() {
    let store = fixtures::new_mem_store();
    let loaded = helpers::simple_state_manifest(&store);
    let mut harness = HarnessBuilder::ephemeral(store, loaded)
        .build_world()
        .unwrap();

    harness
        .send_command(START_SCHEMA, fixtures::start_event("replay"))
        .unwrap();
    harness.run_cycle_batch().unwrap();

    let reopened = harness.reopen().unwrap();
    assert_eq!(
        reopened.state_bytes("com.acme/Simple@1", None).unwrap(),
        vec![0xAA]
    );

    let replay = harness.replay_check().unwrap();
    assert!(replay.ok, "replay mismatches: {:?}", replay.mismatches);

    let artifacts = harness.export_artifacts().unwrap();
    assert!(artifacts.evidence.cycles_run >= 1);
    assert!(!artifacts.journal_entries.is_empty());
    assert!(artifacts.trace_summary.is_object());
}

#[test]
fn harness_receipt_helpers_encode_builtin_payloads() {
    let store = fixtures::new_mem_store();
    let loaded = helpers::simple_state_manifest(&store);
    let harness = HarnessBuilder::ephemeral(store, loaded)
        .build_world()
        .unwrap();
    let intent_hash = [0xAB; 32];
    let blob_ref =
        HashRef::new("sha256:1111111111111111111111111111111111111111111111111111111111111111")
            .unwrap();
    let edge_ref =
        HashRef::new("sha256:2222222222222222222222222222222222222222222222222222222222222222")
            .unwrap();
    let output_ref =
        HashRef::new("sha256:3333333333333333333333333333333333333333333333333333333333333333")
            .unwrap();

    let blob_put = harness
        .core()
        .receipt_blob_put_ok(
            intent_hash,
            &BlobPutReceipt {
                blob_ref: blob_ref.clone(),
                edge_ref,
                size: 3,
            },
        )
        .unwrap();
    assert_eq!(blob_put.status, ReceiptStatus::Ok);
    assert_eq!(blob_put.adapter_id, "adapter.blob.put.harness");
    assert_eq!(
        serde_cbor::from_slice::<BlobPutReceipt>(&blob_put.payload_cbor)
            .unwrap()
            .blob_ref
            .as_str(),
        blob_ref.as_str()
    );

    let blob_get = harness
        .core()
        .receipt_blob_get_ok(
            intent_hash,
            &BlobGetReceipt {
                blob_ref: blob_ref.clone(),
                size: 3,
                bytes: b"abc".to_vec(),
            },
        )
        .unwrap();
    assert_eq!(blob_get.adapter_id, "adapter.blob.get.harness");
    assert_eq!(
        serde_cbor::from_slice::<BlobGetReceipt>(&blob_get.payload_cbor)
            .unwrap()
            .bytes,
        b"abc".to_vec()
    );

    let http = harness
        .core()
        .receipt_http_request_ok(intent_hash, 200, "adapter.http.test")
        .unwrap();
    assert_eq!(http.adapter_id, "adapter.http.test");
    let http_payload = serde_cbor::from_slice::<HttpRequestReceipt>(&http.payload_cbor).unwrap();
    assert_eq!(http_payload.status, 200);
    assert!(http_payload.timings.end_ns >= http_payload.timings.start_ns);

    let llm = harness
        .core()
        .receipt_llm_generate_ok(
            intent_hash,
            &LlmGenerateReceipt {
                output_ref,
                raw_output_ref: None,
                provider_response_id: Some("resp_123".into()),
                finish_reason: LlmFinishReason {
                    reason: "stop".into(),
                    raw: None,
                },
                token_usage: TokenUsage {
                    prompt: 1,
                    completion: 2,
                    total: Some(3),
                },
                usage_details: None,
                warnings_ref: None,
                rate_limit_ref: None,
                cost_cents: Some(7),
                provider_id: "adapter.llm.test".into(),
            },
        )
        .unwrap();
    assert_eq!(llm.adapter_id, "adapter.llm.test");
    assert_eq!(
        serde_cbor::from_slice::<LlmGenerateReceipt>(&llm.payload_cbor)
            .unwrap()
            .cost_cents,
        Some(7)
    );

    let timeout = harness
        .core()
        .receipt_timeout(
            intent_hash,
            "adapter.test",
            &TimerSetReceipt {
                delivered_at_ns: 42,
                key: None,
            },
        )
        .unwrap();
    assert_eq!(timeout.status, ReceiptStatus::Timeout);
    assert_eq!(timeout.adapter_id, "adapter.test");

    let failure = harness
        .core()
        .receipt_error(
            intent_hash,
            "adapter.test",
            &TimerSetReceipt {
                delivered_at_ns: 13,
                key: Some("retry".into()),
            },
        )
        .unwrap();
    assert_eq!(failure.status, ReceiptStatus::Error);
}
