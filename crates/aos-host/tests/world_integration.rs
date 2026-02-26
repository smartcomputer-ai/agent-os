#![cfg(feature = "e2e-tests")]

use aos_air_types::{DefSchema, ReducerAbi, TypeExpr, TypeRecord};
use aos_kernel::error::KernelError;
use aos_wasm_abi::{DomainEvent, ReducerOutput};
use helpers::fixtures::{self, TestWorld, START_SCHEMA};
use indexmap::IndexMap;

mod helpers;
use helpers::{
    def_text_record_schema, insert_test_schemas, int_type, simple_state_manifest, text_type,
};

#[test]
fn rejects_event_payload_that_violates_schema() {
    let store = fixtures::new_mem_store();
    let loaded = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store, loaded).unwrap();

    let bad_payload = aos_air_exec::Value::Record(IndexMap::new());
    let err = world
        .submit_event_value_result(START_SCHEMA, &bad_payload)
        .expect_err("event should fail schema validation");
    match err {
        KernelError::Manifest(msg) => {
            assert!(msg.contains("payload failed validation"));
            assert!(msg.contains("record missing field 'id'"));
        }
        other => panic!("unexpected error: {:?}", other),
    }
}

#[test]
fn raised_events_are_routed_to_reducers() {
    let store = fixtures::new_mem_store();

    let mut emitter = fixtures::stub_reducer_module(
        &store,
        "com.acme/RaisedEmitter@1",
        &ReducerOutput {
            state: Some(vec![0x01]),
            domain_events: vec![DomainEvent::new(
                "com.acme/Raised@1".to_string(),
                serde_cbor::to_vec(&serde_json::json!({ "value": 9 })).unwrap(),
            )],
            effects: vec![],
            ann: None,
        },
    );
    emitter.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/RaisedEmitterState@1"),
        event: fixtures::schema(START_SCHEMA),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    let mut consumer = fixtures::stub_reducer_module(
        &store,
        "com.acme/RaisedConsumer@1",
        &ReducerOutput {
            state: Some(vec![0xEE]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    consumer.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema("com.acme/RaisedConsumerState@1"),
        event: fixtures::schema("com.acme/Raised@1"),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    let mut loaded = fixtures::build_loaded_manifest(
        vec![],
        vec![],
        vec![emitter, consumer],
        vec![
            fixtures::routing_event(START_SCHEMA, "com.acme/RaisedEmitter@1"),
            fixtures::routing_event("com.acme/Raised@1", "com.acme/RaisedConsumer@1"),
        ],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/Raised@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::from([("value".into(), int_type())]),
                }),
            },
            DefSchema {
                name: "com.acme/RaisedEmitterState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
            DefSchema {
                name: "com.acme/RaisedConsumerState@1".into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::new(),
                }),
            },
        ],
    );

    let mut world = TestWorld::with_store(store, loaded).unwrap();
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("raise"))
        .expect("submit start event");
    world.kernel.tick_until_idle().unwrap();

    assert_eq!(
        world.kernel.reducer_state("com.acme/RaisedConsumer@1"),
        Some(vec![0xEE])
    );
}
