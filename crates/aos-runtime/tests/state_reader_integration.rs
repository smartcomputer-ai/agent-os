mod helpers;
use aos_air_types::{
    DefSchema, WorkflowAbi, schema_index::SchemaIndex, value_normalize::normalize_cbor_by_name,
};
use aos_kernel::{Consistency, StateReader};
use aos_wasm_abi::WorkflowOutput;
use helpers::fixtures;
use std::collections::HashMap;

/// Build a test world with a single stub workflow whose state is set to `payload` on first event.
fn test_world_with_state(payload: &[u8]) -> fixtures::TestWorld {
    let store = fixtures::new_mem_store();

    // Stub workflow that always returns the provided state.
    let mut module = fixtures::stub_workflow_module(
        &store,
        "com.acme/Store@1",
        &WorkflowOutput {
            state: Some(payload.to_vec()),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    module.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/StoreState@1"),
        event: fixtures::schema(fixtures::START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    // Simple schema for start event routed to the workflow.
    let start_schema = fixtures::def_text_record_schema(
        fixtures::START_SCHEMA,
        vec![("id", fixtures::text_type())],
    );

    let routing = vec![fixtures::routing_event(
        fixtures::START_SCHEMA,
        &module.name,
    )];
    let mut loaded = fixtures::build_loaded_manifest(vec![module], routing);
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            start_schema,
            DefSchema {
                name: "com.acme/StoreState@1".into(),
                ty: fixtures::text_type(),
            },
        ],
    );

    fixtures::TestWorld::with_store(store, loaded).expect("build world")
}

/// Build a keyed workflow world: workflow expects key and stores payload per key.
fn test_world_keyed(payload: &[u8], key_field: &str) -> fixtures::TestWorld {
    let store = fixtures::new_mem_store();

    // Match the keyed manifest setup from keyed_workflow_integration.
    let mut workflow = fixtures::stub_workflow_module(
        &store,
        "com.acme/Keyed@1",
        &WorkflowOutput {
            state: Some(payload.to_vec()),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    workflow.key_schema = Some(fixtures::schema("com.acme/Key@1"));
    workflow.abi.workflow = Some(aos_air_types::WorkflowAbi {
        state: fixtures::schema("com.acme/State@1"),
        event: fixtures::schema("com.acme/Event@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    let routing = vec![aos_air_types::RoutingEvent {
        event: fixtures::schema("com.acme/Event@1"),
        module: workflow.name.clone(),
        key_field: Some(key_field.to_string()),
    }];

    let mut loaded = fixtures::build_loaded_manifest(vec![workflow], routing);
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            fixtures::def_text_record_schema("com.acme/State@1", vec![]),
            // Key schema must be Text to match the key_field extraction and CBOR encoding.
            aos_air_types::DefSchema {
                name: "com.acme/Key@1".into(),
                ty: fixtures::text_type(),
            },
            fixtures::def_text_record_schema(
                "com.acme/Event@1",
                vec![(key_field, fixtures::text_type())],
            ),
            fixtures::def_text_record_schema(
                fixtures::START_SCHEMA,
                vec![("id", fixtures::text_type())],
            ),
        ],
    );

    fixtures::TestWorld::with_store(store, loaded).expect("build keyed world")
}

fn canonical_key_bytes(key: &str) -> Vec<u8> {
    let mut schemas = HashMap::new();
    schemas.insert("com.acme/Key@1".to_string(), fixtures::text_type());
    let idx = SchemaIndex::new(schemas);
    normalize_cbor_by_name(
        &idx,
        "com.acme/Key@1",
        &serde_cbor::to_vec(&key.to_string()).unwrap(),
    )
    .unwrap()
    .bytes
}

#[test]
fn head_reads_state_and_meta() {
    let mut world = test_world_with_state(b"hello");
    world
        .submit_event_result(
            fixtures::START_SCHEMA,
            &serde_json::json!({ "id": "start" }),
        )
        .expect("submit start event");
    world.tick_n(2).unwrap();

    let read = world
        .kernel
        .get_workflow_state("com.acme/Store@1", None, Consistency::Head)
        .expect("head read");

    assert_eq!(read.value.as_deref(), Some("hello".as_bytes()));
    assert!(read.meta.journal_height > 0);
    assert_eq!(read.meta.manifest_hash.to_hex().len() > 0, true);
}

#[test]
fn exact_errors_without_snapshot() {
    let mut world = test_world_with_state(b"hi");
    world
        .submit_event_result(
            fixtures::START_SCHEMA,
            &serde_json::json!({ "id": "start" }),
        )
        .expect("submit start event");
    world.tick_n(1).unwrap();

    let err = world
        .kernel
        .get_workflow_state("com.acme/Store@1", None, Consistency::Exact(999))
        .unwrap_err();
    assert!(format!("{err:?}").contains("SnapshotUnavailable"));
}

#[test]
fn exact_uses_snapshot_when_available() {
    let mut world = test_world_with_state(b"snap");
    world
        .submit_event_result(
            fixtures::START_SCHEMA,
            &serde_json::json!({ "id": "start" }),
        )
        .expect("submit start event");
    world.tick_n(1).unwrap();

    world.kernel.create_snapshot().unwrap();
    let snap_hash = world.kernel.snapshot_hash();
    let snap_height = world.kernel.heights().snapshot.unwrap();

    let read = world
        .kernel
        .get_workflow_state("com.acme/Store@1", None, Consistency::Exact(snap_height))
        .expect("exact read");

    assert_eq!(read.value.as_deref(), Some("snap".as_bytes()));
    assert_eq!(read.meta.journal_height, snap_height);
    assert_eq!(read.meta.snapshot_hash, snap_hash);
    assert_eq!(read.meta.manifest_hash.to_hex().len() > 0, true);
}

#[test]
fn keyed_head_and_exact_reads_state() {
    let key_field = "id";
    let key_val = "k1";
    let mut world = test_world_keyed(b"cell", key_field);

    // Submit an event with key field so it routes to the keyed workflow instance.
    let payload = serde_cbor::to_vec(&serde_json::json!({ key_field: key_val })).unwrap();
    world
        .kernel
        .submit_domain_event("com.acme/Event@1".to_string(), payload)
        .expect("submit domain event");
    world.kernel.tick_until_idle().unwrap();

    let cells = world.kernel.list_cells("com.acme/Keyed@1").unwrap();
    assert_eq!(cells.len(), 1, "expected one keyed cell, got {cells:?}");

    // Head read
    let key_bytes = canonical_key_bytes(key_val);
    let head_read = world
        .kernel
        .get_workflow_state("com.acme/Keyed@1", Some(&key_bytes), Consistency::Head)
        .expect("head read keyed");
    assert_eq!(head_read.value.as_deref(), Some("cell".as_bytes()));

    // Snapshot for Exact
    world.kernel.create_snapshot().unwrap();
    let snap_height = world.kernel.heights().snapshot.unwrap();
    let exact_read = world
        .kernel
        .get_workflow_state(
            "com.acme/Keyed@1",
            Some(&key_bytes),
            Consistency::Exact(snap_height),
        )
        .expect("exact read keyed");
    assert_eq!(exact_read.value.as_deref(), Some("cell".as_bytes()));
    assert_eq!(exact_read.meta.journal_height, snap_height);
    assert!(exact_read.meta.snapshot_hash.is_some());
}
