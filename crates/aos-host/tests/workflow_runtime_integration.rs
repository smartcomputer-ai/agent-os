#![cfg(feature = "e2e-tests")]

use std::sync::Arc;

use aos_air_types::{
    DefModule, DefSchema, HashRef, ModuleAbi, ModuleKind, WorkflowAbi, TypeExpr, TypeRecord,
    TypeRef, TypeVariant,
};
use aos_effects::builtins::{
    BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt, HttpRequestParams,
    HttpRequestReceipt, RequestTimings, TimerSetParams, TimerSetReceipt,
};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_kernel::error::KernelError;
use aos_kernel::snapshot::WorkflowStatusSnapshot;
use aos_store::Store;
use aos_wasm_abi::{DomainEvent, WorkflowEffect, WorkflowOutput};
use helpers::fixtures::{self, effect_params_text, fake_hash, TestStore, TestWorld, START_SCHEMA};
use indexmap::IndexMap;
use wat::parse_str;

mod helpers;
use helpers::{def_text_record_schema, insert_test_schemas, text_type};

#[test]
fn workflow_orchestration_completes_after_receipt() {
    let store = fixtures::new_mem_store();
    let mut world =
        TestWorld::with_store(store.clone(), workflow_receipt_manifest(&store)).unwrap();

    let start_event = serde_json::json!({
        "$tag": "Start",
        "$value": fixtures::start_event("wf-1")
    });
    world
        .submit_event_result("com.acme/WorkflowEvent@1", &start_event)
        .expect("submit workflow start");
    world.tick_n(1).unwrap();

    let mut effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 1);
    let effect = effects.remove(0);
    assert_eq!(effect.kind.as_str(), aos_effects::EffectKind::HTTP_REQUEST);

    let receipt = EffectReceipt {
        intent_hash: effect.intent_hash,
        adapter_id: "adapter.http".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&HttpRequestReceipt {
            status: 200,
            headers: IndexMap::new(),
            body_ref: None,
            timings: RequestTimings {
                start_ns: 1,
                end_ns: 2,
            },
            adapter_id: "adapter.http".into(),
        })
        .unwrap(),
        cost_cents: None,
        signature: vec![],
    };
    world.kernel.handle_receipt(receipt).unwrap();
    world.kernel.tick_until_idle().unwrap();

    assert_eq!(
        world.kernel.workflow_state("com.acme/ResultWorkflow@1"),
        Some(vec![0xEE])
    );
}

#[test]
fn workflow_effects_share_outbox_without_interference() {
    let store = fixtures::new_mem_store();

    let mut timer_module = fixtures::stub_workflow_module(
        &store,
        "com.acme/TimerEmitter@1",
        &WorkflowOutput {
            state: Some(vec![0xA1]),
            domain_events: vec![],
            effects: vec![WorkflowEffect::new(
                aos_effects::EffectKind::TIMER_SET,
                serde_cbor::to_vec(&TimerSetParams {
                    deliver_at_ns: 5,
                    key: Some("shared".into()),
                })
                .unwrap(),
            )],
            ann: None,
        },
    );
    timer_module.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/TimerState@1"),
        event: fixtures::schema(START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::TIMER_SET.into()],
        cap_slots: Default::default(),
    });

    let mut http_module = fixtures::stub_workflow_module(
        &store,
        "com.acme/HttpEmitter@1",
        &WorkflowOutput {
            state: Some(vec![0xB2]),
            domain_events: vec![],
            effects: vec![WorkflowEffect::with_cap_slot(
                aos_effects::EffectKind::HTTP_REQUEST,
                serde_cbor::to_vec(&HttpRequestParams {
                    method: "GET".into(),
                    url: "https://example.com/shared".into(),
                    headers: IndexMap::new(),
                    body_ref: None,
                })
                .unwrap(),
                "http",
            )],
            ann: None,
        },
    );
    http_module.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/HttpState@1"),
        event: fixtures::schema(START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::HTTP_REQUEST.into()],
        cap_slots: Default::default(),
    });

    let mut loaded = build_loaded_manifest_with_http_enforcer(
        &store,
        vec![timer_module, http_module],
        vec![
            fixtures::routing_event(START_SCHEMA, "com.acme/TimerEmitter@1"),
            fixtures::routing_event(START_SCHEMA, "com.acme/HttpEmitter@1"),
        ],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            def_text_record_schema("com.acme/TimerState@1", vec![]),
            def_text_record_schema("com.acme/HttpState@1", vec![]),
        ],
    );
    if let Some(binding) = loaded
        .manifest
        .module_bindings
        .get_mut("com.acme/HttpEmitter@1")
    {
        binding.slots.insert("http".into(), "cap_http".into());
    }

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("shared"))
        .expect("submit start");
    world.kernel.tick_until_idle().unwrap();

    let effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 2);
    let kinds: Vec<_> = effects.iter().map(|e| e.kind.as_str()).collect();
    assert!(kinds.contains(&aos_effects::EffectKind::TIMER_SET));
    assert!(kinds.contains(&aos_effects::EffectKind::HTTP_REQUEST));

    assert_eq!(
        world.kernel.workflow_state("com.acme/TimerEmitter@1"),
        Some(vec![0xA1])
    );
    assert_eq!(
        world.kernel.workflow_state("com.acme/HttpEmitter@1"),
        Some(vec![0xB2])
    );
}

#[test]
fn workflow_timer_receipt_routes_event_to_handler() {
    let store = fixtures::new_mem_store();
    let manifest = timer_receipt_workflow_manifest(&store);
    let mut world = TestWorld::with_store(store, manifest).unwrap();
    let start_event = serde_json::json!({
        "$tag": "Start",
        "$value": fixtures::start_event("timer")
    });
    world
        .submit_event_result("com.acme/TimerWorkflowEvent@1", &start_event)
        .expect("submit start event");
    world.tick_n(1).unwrap();

    let mut effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 1);
    let effect = effects.remove(0);

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

    assert_eq!(
        world.kernel.workflow_state("com.acme/TimerWorkflow@1"),
        Some(vec![0xCC])
    );

    let duplicate = EffectReceipt {
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
    world.kernel.handle_receipt(duplicate).unwrap();
    world.tick_n(1).unwrap();
    assert!(world.drain_effects().expect("drain effects").is_empty());

    let unknown = EffectReceipt {
        intent_hash: [9u8; 32],
        adapter_id: "adapter.timer".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
            delivered_at_ns: 42,
            key: None,
        })
        .unwrap(),
        cost_cents: None,
        signature: vec![],
    };
    let err = world.kernel.handle_receipt(unknown).unwrap_err();
    assert!(matches!(err, KernelError::UnknownReceipt(_)));
}

#[test]
fn blob_put_receipt_routes_event_to_handler() {
    let store = fixtures::new_mem_store();

    let mut emitter = fixtures::stub_workflow_module(
        &store,
        "com.acme/BlobPutEmitter@1",
        &WorkflowOutput {
            state: None,
            domain_events: vec![],
            effects: vec![WorkflowEffect::with_cap_slot(
                aos_effects::EffectKind::BLOB_PUT,
                serde_cbor::to_vec(&BlobPutParams {
                    bytes: Vec::new(),
                    blob_ref: None,
                    refs: None,
                })
                .unwrap(),
                "blob",
            )],
            ann: None,
        },
    );

    let mut handler = fixtures::stub_workflow_module(
        &store,
        "com.acme/BlobPutHandler@1",
        &WorkflowOutput {
            state: Some(vec![0xDD]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );

    let event_schema = "com.acme/BlobPutEvent@1";
    emitter.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema(START_SCHEMA),
        event: fixtures::schema(event_schema),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::BLOB_PUT.into()],
        cap_slots: Default::default(),
    });
    handler.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema(START_SCHEMA),
        event: fixtures::schema(event_schema),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    let mut loaded = build_loaded_manifest_with_http_enforcer(
        &store,
        vec![emitter, handler],
        vec![
            fixtures::routing_event(event_schema, "com.acme/BlobPutEmitter@1"),
            fixtures::routing_event(event_schema, "com.acme/BlobPutHandler@1"),
        ],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: event_schema.into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(START_SCHEMA),
                            }),
                        ),
                        (
                            "PutResult".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("sys/BlobPutResult@1"),
                            }),
                        ),
                    ]),
                }),
            },
        ],
    );
    if let Some(binding) = loaded
        .manifest
        .module_bindings
        .get_mut("com.acme/BlobPutEmitter@1")
    {
        binding.slots.insert("blob".into(), "blob_cap".into());
    }

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    let start_event = serde_json::json!({
        "$tag": "Start",
        "$value": fixtures::start_event("blob-put"),
    });
    world
        .submit_event_result(event_schema, &start_event)
        .expect("submit start event");
    world.tick_n(1).unwrap();

    let mut effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 1);
    let intent = effects.remove(0);
    assert_eq!(intent.kind.as_str(), aos_effects::EffectKind::BLOB_PUT);

    let receipt = EffectReceipt {
        intent_hash: intent.intent_hash,
        adapter_id: "adapter.blob".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&BlobPutReceipt {
            blob_ref: fake_hash(0x21),
            edge_ref: fake_hash(0x22),
            size: 64,
        })
        .unwrap(),
        cost_cents: Some(2),
        signature: vec![7, 7],
    };
    world.kernel.handle_receipt(receipt).unwrap();
    world.tick_n(1).unwrap();

    assert_eq!(
        world.kernel.workflow_state("com.acme/BlobPutHandler@1"),
        Some(vec![0xDD])
    );
}

#[test]
fn blob_get_receipt_routes_event_to_handler() {
    let store = fixtures::new_mem_store();

    let mut emitter = fixtures::stub_workflow_module(
        &store,
        "com.acme/BlobGetEmitter@1",
        &WorkflowOutput {
            state: None,
            domain_events: vec![],
            effects: vec![WorkflowEffect::with_cap_slot(
                aos_effects::EffectKind::BLOB_GET,
                serde_cbor::to_vec(&BlobGetParams {
                    blob_ref: fake_hash(0x10),
                })
                .unwrap(),
                "blob",
            )],
            ann: None,
        },
    );

    let mut handler = fixtures::stub_workflow_module(
        &store,
        "com.acme/BlobGetHandler@1",
        &WorkflowOutput {
            state: Some(vec![0xEE]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );

    let event_schema = "com.acme/BlobGetEvent@1";
    emitter.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema(START_SCHEMA),
        event: fixtures::schema(event_schema),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::BLOB_GET.into()],
        cap_slots: Default::default(),
    });
    handler.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema(START_SCHEMA),
        event: fixtures::schema(event_schema),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    let mut loaded = build_loaded_manifest_with_http_enforcer(
        &store,
        vec![emitter, handler],
        vec![
            fixtures::routing_event(event_schema, "com.acme/BlobGetEmitter@1"),
            fixtures::routing_event(event_schema, "com.acme/BlobGetHandler@1"),
        ],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: event_schema.into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(START_SCHEMA),
                            }),
                        ),
                        (
                            "GetResult".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema("sys/BlobGetResult@1"),
                            }),
                        ),
                    ]),
                }),
            },
        ],
    );
    if let Some(binding) = loaded
        .manifest
        .module_bindings
        .get_mut("com.acme/BlobGetEmitter@1")
    {
        binding.slots.insert("blob".into(), "blob_cap".into());
    }

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    let start_event = serde_json::json!({
        "$tag": "Start",
        "$value": fixtures::start_event("blob-get"),
    });
    world
        .submit_event_result(event_schema, &start_event)
        .expect("submit start event");
    world.tick_n(1).unwrap();

    let mut effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 1);
    let intent = effects.remove(0);
    assert_eq!(intent.kind.as_str(), aos_effects::EffectKind::BLOB_GET);

    let receipt = EffectReceipt {
        intent_hash: intent.intent_hash,
        adapter_id: "adapter.blob".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&BlobGetReceipt {
            blob_ref: fake_hash(0x22),
            size: 128,
            bytes: vec![0; 128],
        })
        .unwrap(),
        cost_cents: None,
        signature: vec![8, 8],
    };
    world.kernel.handle_receipt(receipt).unwrap();
    world.tick_n(1).unwrap();

    assert_eq!(
        world.kernel.workflow_state("com.acme/BlobGetHandler@1"),
        Some(vec![0xEE])
    );
}

#[test]
fn workflow_receipt_and_event_progression_emit_followups_in_order() {
    let store = fixtures::new_mem_store();

    let start_output = WorkflowOutput {
        state: Some(vec![0x10]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::HTTP_REQUEST,
            serde_cbor::to_vec(&HttpRequestParams {
                method: "GET".into(),
                url: "https://example.com/first".into(),
                headers: IndexMap::new(),
                body_ref: None,
            })
            .unwrap(),
            "http",
        )],
        ann: None,
    };
    let receipt_output = WorkflowOutput {
        state: Some(vec![0x11]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::HTTP_REQUEST,
            serde_cbor::to_vec(&HttpRequestParams {
                method: "GET".into(),
                url: "https://example.com/after-receipt".into(),
                headers: IndexMap::new(),
                body_ref: None,
            })
            .unwrap(),
            "http",
        )],
        ann: None,
    };
    let mut staged = sequenced_workflow_module(
        &store,
        "com.acme/StagedWorkflow@1",
        &start_output,
        &receipt_output,
    );
    staged.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/StagedState@1"),
        event: fixtures::schema("com.acme/StagedWorkflowEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::HTTP_REQUEST.into()],
        cap_slots: Default::default(),
    });

    let mut pulse = fixtures::stub_workflow_module(
        &store,
        "com.acme/PulseWorkflow@1",
        &WorkflowOutput {
            state: Some(vec![0x12]),
            domain_events: vec![],
            effects: vec![WorkflowEffect::with_cap_slot(
                aos_effects::EffectKind::HTTP_REQUEST,
                serde_cbor::to_vec(&HttpRequestParams {
                    method: "GET".into(),
                    url: "https://example.com/after-event".into(),
                    headers: IndexMap::new(),
                    body_ref: None,
                })
                .unwrap(),
                "http",
            )],
            ann: None,
        },
    );
    pulse.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/PulseState@1"),
        event: fixtures::schema("com.acme/PulseNext@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::HTTP_REQUEST.into()],
        cap_slots: Default::default(),
    });

    let mut loaded = build_loaded_manifest_with_http_enforcer(
        &store,
        vec![staged, pulse],
        vec![
            fixtures::routing_event(
                "com.acme/StagedWorkflowEvent@1",
                "com.acme/StagedWorkflow@1",
            ),
            fixtures::routing_event("com.acme/PulseNext@1", "com.acme/PulseWorkflow@1"),
        ],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/StagedWorkflowEvent@1".into(),
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
                name: "com.acme/PulseNext@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            DefSchema {
                name: "com.acme/StagedState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            DefSchema {
                name: "com.acme/PulseState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );
    for module in ["com.acme/StagedWorkflow@1", "com.acme/PulseWorkflow@1"] {
        loaded
            .manifest
            .module_bindings
            .get_mut(module)
            .expect("module binding")
            .slots
            .insert("http".into(), "cap_http".into());
    }

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    let start_event = serde_json::json!({
        "$tag": "Start",
        "$value": fixtures::start_event("staged"),
    });
    world
        .submit_event_result("com.acme/StagedWorkflowEvent@1", &start_event)
        .expect("submit start");
    world.tick_n(1).unwrap();

    let mut effects = world.drain_effects().expect("drain first");
    assert_eq!(effects.len(), 1);
    let first = effects.remove(0);
    assert_eq!(
        effect_params_text(&first),
        "https://example.com/first".to_string()
    );

    let receipt = EffectReceipt {
        intent_hash: first.intent_hash,
        adapter_id: "adapter.http".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&HttpRequestReceipt {
            status: 200,
            headers: IndexMap::new(),
            body_ref: None,
            timings: RequestTimings {
                start_ns: 1,
                end_ns: 2,
            },
            adapter_id: "adapter.http".into(),
        })
        .unwrap(),
        cost_cents: None,
        signature: vec![],
    };
    world.kernel.handle_receipt(receipt).unwrap();
    world.tick_n(1).unwrap();

    let mut effects = world.drain_effects().expect("drain second");
    assert_eq!(effects.len(), 1);
    assert_eq!(
        effect_params_text(&effects.remove(0)),
        "https://example.com/after-receipt".to_string()
    );

    world
        .submit_event_result("com.acme/PulseNext@1", &serde_json::json!({}))
        .expect("submit pulse");
    world.tick_n(1).unwrap();

    let mut effects = world.drain_effects().expect("drain third");
    assert_eq!(effects.len(), 1);
    assert_eq!(
        effect_params_text(&effects.remove(0)),
        "https://example.com/after-event".to_string()
    );
}

#[test]
fn workflow_event_routing_only_matches_subscribed_schema() {
    let store = fixtures::new_mem_store();

    let mut ready = fixtures::stub_workflow_module(
        &store,
        "com.acme/ReadyWorkflow@1",
        &WorkflowOutput {
            state: Some(vec![0x21]),
            domain_events: vec![],
            effects: vec![WorkflowEffect::with_cap_slot(
                aos_effects::EffectKind::HTTP_REQUEST,
                serde_cbor::to_vec(&HttpRequestParams {
                    method: "GET".into(),
                    url: "https://example.com/ready".into(),
                    headers: IndexMap::new(),
                    body_ref: None,
                })
                .unwrap(),
                "http",
            )],
            ann: None,
        },
    );
    ready.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/ReadyState@1"),
        event: fixtures::schema("com.acme/Ready@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::HTTP_REQUEST.into()],
        cap_slots: Default::default(),
    });

    let mut other = fixtures::stub_workflow_module(
        &store,
        "com.acme/OtherWorkflow@1",
        &WorkflowOutput {
            state: Some(vec![0x22]),
            domain_events: vec![],
            effects: vec![WorkflowEffect::with_cap_slot(
                aos_effects::EffectKind::HTTP_REQUEST,
                serde_cbor::to_vec(&HttpRequestParams {
                    method: "GET".into(),
                    url: "https://example.com/other".into(),
                    headers: IndexMap::new(),
                    body_ref: None,
                })
                .unwrap(),
                "http",
            )],
            ann: None,
        },
    );
    other.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/OtherState@1"),
        event: fixtures::schema("com.acme/Other@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::HTTP_REQUEST.into()],
        cap_slots: Default::default(),
    });

    let mut loaded = build_loaded_manifest_with_http_enforcer(
        &store,
        vec![ready, other],
        vec![
            fixtures::routing_event("com.acme/Ready@1", "com.acme/ReadyWorkflow@1"),
            fixtures::routing_event("com.acme/Other@1", "com.acme/OtherWorkflow@1"),
        ],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            DefSchema {
                name: "com.acme/Ready@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            DefSchema {
                name: "com.acme/Other@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            DefSchema {
                name: "com.acme/ReadyState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            DefSchema {
                name: "com.acme/OtherState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );
    for module in ["com.acme/ReadyWorkflow@1", "com.acme/OtherWorkflow@1"] {
        loaded
            .manifest
            .module_bindings
            .get_mut(module)
            .expect("module binding")
            .slots
            .insert("http".into(), "cap_http".into());
    }

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_result("com.acme/Ready@1", &serde_json::json!({}))
        .expect("submit ready");
    world.tick_n(1).unwrap();
    let mut effects = world.drain_effects().expect("drain ready");
    assert_eq!(effects.len(), 1);
    assert_eq!(
        effect_params_text(&effects.remove(0)),
        "https://example.com/ready".to_string()
    );

    world
        .submit_event_result("com.acme/Other@1", &serde_json::json!({}))
        .expect("submit other");
    world.tick_n(1).unwrap();
    let mut effects = world.drain_effects().expect("drain other");
    assert_eq!(effects.len(), 1);
    assert_eq!(
        effect_params_text(&effects.remove(0)),
        "https://example.com/other".to_string()
    );
}

#[test]
fn keyed_workflow_receipt_routing_is_instance_isolated() {
    let store = fixtures::new_mem_store();

    let start_output = WorkflowOutput {
        state: Some(vec![0x31]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::HTTP_REQUEST,
            serde_cbor::to_vec(&HttpRequestParams {
                method: "GET".into(),
                url: "https://example.com/keyed".into(),
                headers: IndexMap::new(),
                body_ref: None,
            })
            .unwrap(),
            "http",
        )],
        ann: None,
    };
    let receipt_output = WorkflowOutput {
        state: Some(vec![0x32]),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };
    let mut keyed = sequenced_workflow_module(
        &store,
        "com.acme/KeyedWorkflow@1",
        &start_output,
        &receipt_output,
    );
    keyed.key_schema = Some(fixtures::schema("com.acme/WorkflowKey@1"));
    keyed.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/KeyedState@1"),
        event: fixtures::schema("com.acme/KeyedWorkflowEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::HTTP_REQUEST.into()],
        cap_slots: Default::default(),
    });

    let mut keyed_route =
        fixtures::routing_event("com.acme/KeyedWorkflowEvent@1", "com.acme/KeyedWorkflow@1");
    keyed_route.key_field = Some("$value.id".into());

    let mut loaded =
        build_loaded_manifest_with_http_enforcer(&store, vec![keyed], vec![keyed_route]);
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/WorkflowKey@1".into(),
                ty: text_type(),
            },
            DefSchema {
                name: "com.acme/KeyedWorkflowEvent@1".into(),
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
                name: "com.acme/KeyedState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );
    loaded
        .manifest
        .module_bindings
        .get_mut("com.acme/KeyedWorkflow@1")
        .expect("module binding")
        .slots
        .insert("http".into(), "cap_http".into());

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    for id in ["a", "b"] {
        let event = serde_json::json!({
            "$tag": "Start",
            "$value": fixtures::start_event(id),
        });
        world
            .submit_event_result("com.acme/KeyedWorkflowEvent@1", &event)
            .expect("submit keyed start");
    }
    world.kernel.tick_until_idle().unwrap();

    let effects = world.drain_effects().expect("drain effects");
    assert_eq!(effects.len(), 2);

    let pending = world.kernel.pending_workflow_receipts_snapshot();
    assert_eq!(pending.len(), 2);
    let keys_by_hash: std::collections::HashMap<[u8; 32], String> = pending
        .iter()
        .map(|entry| {
            let key_bytes = entry
                .origin_instance_key
                .as_ref()
                .expect("keyed instance should have key");
            let key: String = serde_cbor::from_slice(key_bytes).expect("decode key");
            (entry.intent_hash, key)
        })
        .collect();

    let b_hash = keys_by_hash
        .iter()
        .find_map(|(hash, key)| if key == "b" { Some(*hash) } else { None })
        .expect("missing b hash");

    let receipt = EffectReceipt {
        intent_hash: b_hash,
        adapter_id: "adapter.http".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&HttpRequestReceipt {
            status: 200,
            headers: IndexMap::new(),
            body_ref: None,
            timings: RequestTimings {
                start_ns: 10,
                end_ns: 12,
            },
            adapter_id: "adapter.http".into(),
        })
        .unwrap(),
        cost_cents: None,
        signature: vec![],
    };
    world.kernel.handle_receipt(receipt).unwrap();
    world.kernel.tick_until_idle().unwrap();

    let remaining = world.kernel.pending_workflow_receipts_snapshot();
    assert_eq!(remaining.len(), 1);
    let remaining_key: String = serde_cbor::from_slice(
        remaining[0]
            .origin_instance_key
            .as_ref()
            .expect("remaining keyed pending intent should keep key"),
    )
    .expect("decode remaining key");
    assert_eq!(remaining_key, "a");

    let instances: Vec<_> = world
        .kernel
        .workflow_instances_snapshot()
        .into_iter()
        .filter(|instance| {
            instance
                .instance_id
                .starts_with("com.acme/KeyedWorkflow@1::")
        })
        .collect();
    assert_eq!(instances.len(), 2);
    assert_eq!(
        instances
            .iter()
            .filter(|instance| instance.status == WorkflowStatusSnapshot::Waiting)
            .count(),
        1
    );
    assert_eq!(
        instances
            .iter()
            .filter(|instance| instance.inflight_intents.is_empty())
            .count(),
        1
    );
}

fn allow_http_enforcer(store: &Arc<TestStore>) -> DefModule {
    let allow_output = aos_kernel::cap_enforcer::CapCheckOutput {
        constraints_ok: true,
        deny: None,
    };
    let output_bytes = serde_cbor::to_vec(&allow_output).expect("encode cap output");
    let pure_output = aos_wasm_abi::PureOutput {
        output: output_bytes,
    };
    fixtures::stub_pure_module(
        store,
        "sys/CapEnforceHttpOut@1",
        &pure_output,
        "sys/CapCheckInput@1",
        "sys/CapCheckOutput@1",
    )
}

fn build_loaded_manifest_with_http_enforcer(
    store: &Arc<TestStore>,
    mut modules: Vec<DefModule>,
    routing: Vec<aos_air_types::RoutingEvent>,
) -> aos_kernel::manifest::LoadedManifest {
    if !modules
        .iter()
        .any(|module| module.name == "sys/CapEnforceHttpOut@1")
    {
        modules.push(allow_http_enforcer(store));
    }
    fixtures::build_loaded_manifest(modules, routing)
}

fn workflow_receipt_manifest(store: &Arc<TestStore>) -> aos_kernel::manifest::LoadedManifest {
    let start_output = WorkflowOutput {
        state: Some(vec![0x01]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::HTTP_REQUEST,
            serde_cbor::to_vec(&HttpRequestParams {
                method: "GET".into(),
                url: "https://example.com/workflow".into(),
                headers: IndexMap::new(),
                body_ref: None,
            })
            .unwrap(),
            "http",
        )],
        ann: None,
    };
    let receipt_output = WorkflowOutput {
        state: Some(vec![0x02]),
        domain_events: vec![DomainEvent::new(
            "com.acme/WorkflowDone@1".to_string(),
            serde_cbor::to_vec(&serde_json::json!({ "id": "wf-1" })).unwrap(),
        )],
        effects: vec![],
        ann: None,
    };

    let mut workflow =
        sequenced_workflow_module(store, "com.acme/Workflow@1", &start_output, &receipt_output);
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/WorkflowState@1"),
        event: fixtures::schema("com.acme/WorkflowEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::HTTP_REQUEST.into()],
        cap_slots: Default::default(),
    });

    let mut result_module = fixtures::stub_workflow_module(
        store,
        "com.acme/ResultWorkflow@1",
        &WorkflowOutput {
            state: Some(vec![0xEE]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    result_module.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/ResultState@1"),
        event: fixtures::schema("com.acme/WorkflowDone@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: IndexMap::new(),
    });

    let mut loaded = build_loaded_manifest_with_http_enforcer(
        store,
        vec![workflow, result_module],
        vec![
            fixtures::routing_event("com.acme/WorkflowEvent@1", "com.acme/Workflow@1"),
            fixtures::routing_event("com.acme/WorkflowDone@1", "com.acme/ResultWorkflow@1"),
        ],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/WorkflowEvent@1".into(),
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
            def_text_record_schema("com.acme/WorkflowDone@1", vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/WorkflowState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            DefSchema {
                name: "com.acme/ResultState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );
    if let Some(binding) = loaded
        .manifest
        .module_bindings
        .get_mut("com.acme/Workflow@1")
    {
        binding.slots.insert("http".into(), "cap_http".into());
    }
    loaded
}

fn timer_receipt_workflow_manifest(store: &Arc<TestStore>) -> aos_kernel::manifest::LoadedManifest {
    let start_output = WorkflowOutput {
        state: Some(vec![0x01]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::new(
            aos_effects::EffectKind::TIMER_SET,
            serde_cbor::to_vec(&TimerSetParams {
                deliver_at_ns: 10,
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
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
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
