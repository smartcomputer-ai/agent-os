//! Introspection integration tests against TestWorld/Kernel (no daemon).

#[path = "support/helpers.rs"]
mod helpers;
use helpers::fixtures::{self, WorkflowAbi};

use aos_effect_types::{
    IntrospectJournalHeadReceipt, IntrospectListCellsReceipt, IntrospectManifestReceipt,
    IntrospectWorkflowStateReceipt,
};
use aos_effects::{IntentBuilder, ReceiptStatus, effect_ops};
use aos_kernel::StateReader;
use aos_wasm_abi::WorkflowOutput;
use serde_json::json;

/// Build a simple world with a monolithic workflow that sets state on the first event.
fn world_with_state(bytes: &[u8]) -> helpers::fixtures::TestWorld {
    let store = fixtures::new_mem_store();
    let mut workflow = fixtures::stub_workflow_module(
        &store,
        "com.acme/Store@1",
        &WorkflowOutput {
            state: Some(bytes.to_vec()),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/StoreState@1"),
        event: fixtures::schema(fixtures::START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
    });
    let routing = vec![fixtures::routing_event(
        fixtures::START_SCHEMA,
        &workflow.name,
    )];
    let mut loaded = fixtures::build_loaded_manifest(vec![workflow], routing);
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            fixtures::def_text_record_schema(
                fixtures::START_SCHEMA,
                vec![("id", fixtures::text_type())],
            ),
            aos_air_types::DefSchema {
                name: "com.acme/StoreState@1".into(),
                ty: fixtures::text_type(),
            },
        ],
    );
    helpers::fixtures::TestWorld::with_store(store, loaded).expect("world")
}

#[test]
fn introspect_manifest_matches_kernel_manifest() {
    let mut world = world_with_state(b"hello");
    world
        .submit_event_result(fixtures::START_SCHEMA, &json!({ "id": "start" }))
        .expect("submit");
    world.tick_n(1).unwrap();

    let kernel = &mut world.kernel;
    let intent = IntentBuilder::new(
        effect_ops::INTROSPECT_MANIFEST,
        &json!({ "consistency": "head" }),
    )
    .build()
    .unwrap();

    let receipt = kernel
        .handle_internal_intent(&intent)
        .unwrap()
        .expect("handled");
    assert_eq!(receipt.status, ReceiptStatus::Ok);

    let decoded: IntrospectManifestReceipt = receipt.payload().unwrap();
    let manifest: aos_air_types::Manifest = serde_cbor::from_slice(&decoded.manifest).unwrap();
    let head_manifest = kernel
        .get_manifest(aos_kernel::Consistency::Head)
        .unwrap()
        .value;
    assert_eq!(manifest.air_version, head_manifest.air_version);
    assert_eq!(
        decoded.meta.manifest_hash.to_string(),
        kernel.get_journal_head().manifest_hash.to_hex()
    );
}

#[test]
fn introspect_workflow_state_returns_value_and_meta() {
    let mut world = world_with_state(b"payload");
    world
        .submit_event_result(fixtures::START_SCHEMA, &json!({ "id": "start" }))
        .expect("submit");
    world.tick_n(1).unwrap();

    let kernel = &mut world.kernel;
    let intent = IntentBuilder::new(
        effect_ops::INTROSPECT_WORKFLOW_STATE,
        &json!({
            "workflow": "com.acme/Store@1",
            "consistency": "head"
        }),
    )
    .build()
    .unwrap();

    let receipt = kernel
        .handle_internal_intent(&intent)
        .unwrap()
        .expect("handled");
    assert_eq!(receipt.status, ReceiptStatus::Ok);

    let decoded: IntrospectWorkflowStateReceipt = receipt.payload().unwrap();
    assert_eq!(decoded.state.as_deref(), Some("payload".as_bytes()));
    assert_eq!(
        decoded.meta.manifest_hash.to_string(),
        kernel.get_journal_head().manifest_hash.to_hex()
    );
}

#[test]
fn introspect_list_cells_returns_sentinel_for_non_keyed() {
    let mut world = world_with_state(b"payload");
    world
        .submit_event_result(fixtures::START_SCHEMA, &json!({ "id": "start" }))
        .expect("submit");
    world.tick_n(1).unwrap();
    let kernel = &mut world.kernel;

    let intent = IntentBuilder::new(
        effect_ops::INTROSPECT_LIST_CELLS,
        &json!({ "workflow": "com.acme/Store@1" }),
    )
    .build()
    .unwrap();

    let receipt = kernel
        .handle_internal_intent(&intent)
        .unwrap()
        .expect("handled");
    assert_eq!(receipt.status, ReceiptStatus::Ok);

    let decoded: IntrospectListCellsReceipt = receipt.payload().unwrap();
    assert_eq!(
        decoded.cells.len(),
        1,
        "expected sentinel cell for non-keyed workflow"
    );
    assert_eq!(
        decoded.cells[0].key.len(),
        0,
        "sentinel key should be empty bytes"
    );
}

#[test]
fn introspect_journal_head_matches_state_reader() {
    let mut world = world_with_state(b"payload");
    world
        .submit_event_result(fixtures::START_SCHEMA, &json!({ "id": "start" }))
        .expect("submit");
    world.tick_n(1).unwrap();

    let kernel = &mut world.kernel;
    let intent = IntentBuilder::new(effect_ops::INTROSPECT_JOURNAL_HEAD, &json!({}))
        .build()
        .unwrap();

    let receipt = kernel
        .handle_internal_intent(&intent)
        .unwrap()
        .expect("handled");
    assert_eq!(receipt.status, ReceiptStatus::Ok);
    let decoded: IntrospectJournalHeadReceipt = receipt.payload().unwrap();
    let meta = kernel.get_journal_head();
    assert_eq!(decoded.meta.journal_height, meta.journal_height);
    assert_eq!(
        decoded.meta.manifest_hash.to_string(),
        meta.manifest_hash.to_hex()
    );
}
