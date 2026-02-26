//! Introspection integration tests against TestWorld/Kernel (no daemon).

mod helpers;
use helpers::fixtures;

use aos_effects::{EffectKind, IntentBuilder, ReceiptStatus};
use aos_kernel::StateReader;
use aos_wasm_abi::ReducerOutput;
use serde::Deserialize;
use serde_json::json;

/// Build a simple world with a monolithic reducer that sets state on the first event.
fn world_with_state(bytes: &[u8]) -> helpers::fixtures::TestWorld {
    let store = fixtures::new_mem_store();
    let mut reducer = fixtures::stub_reducer_module(
        &store,
        "com.acme/Store@1",
        &ReducerOutput {
            state: Some(bytes.to_vec()),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    reducer.abi.reducer = Some(aos_air_types::ReducerAbi {
        state: fixtures::schema("com.acme/StoreState@1"),
        event: fixtures::schema(fixtures::START_SCHEMA),
        context: Some(fixtures::schema("sys/ReducerContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let routing = vec![fixtures::routing_event(
        fixtures::START_SCHEMA,
        &reducer.name,
    )];
    let mut loaded = fixtures::build_loaded_manifest(vec![reducer], routing);
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
        EffectKind::introspect_manifest(),
        "sys/query@1",
        &json!({ "consistency": "head" }),
    )
    .build()
    .unwrap();

    let receipt = kernel
        .handle_internal_intent(&intent)
        .unwrap()
        .expect("handled");
    assert_eq!(receipt.status, ReceiptStatus::Ok);

    // Decode payload map and extract manifest bytes.
    let payload_val: serde_cbor::Value = serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    let manifest_bytes = match payload_val {
        serde_cbor::Value::Map(map) => map
            .into_iter()
            .find_map(|(k, v)| match (k, v) {
                (serde_cbor::Value::Text(t), serde_cbor::Value::Bytes(b)) if t == "manifest" => {
                    Some(b)
                }
                _ => None,
            })
            .expect("manifest bytes"),
        _ => panic!("unexpected payload shape"),
    };
    let manifest: aos_air_types::Manifest = serde_cbor::from_slice(&manifest_bytes).unwrap();
    let head_manifest = kernel
        .get_manifest(aos_kernel::Consistency::Head)
        .unwrap()
        .value;
    assert_eq!(manifest.air_version, head_manifest.air_version);
}

#[test]
fn introspect_reducer_state_returns_value_and_meta() {
    let mut world = world_with_state(b"payload");
    world
        .submit_event_result(fixtures::START_SCHEMA, &json!({ "id": "start" }))
        .expect("submit");
    world.tick_n(1).unwrap();

    let kernel = &mut world.kernel;
    let intent = IntentBuilder::new(
        EffectKind::introspect_reducer_state(),
        "sys/query@1",
        &json!({
            "reducer": "com.acme/Store@1",
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

    #[derive(Deserialize)]
    struct ReducerReceipt {
        #[serde(default)]
        state: Option<Vec<u8>>,
    }

    let decoded: ReducerReceipt = receipt.payload().unwrap();
    assert_eq!(decoded.state.as_deref(), Some("payload".as_bytes()));
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
        EffectKind::introspect_list_cells(),
        "sys/query@1",
        &json!({ "reducer": "com.acme/Store@1" }),
    )
    .build()
    .unwrap();

    let receipt = kernel
        .handle_internal_intent(&intent)
        .unwrap()
        .expect("handled");
    assert_eq!(receipt.status, ReceiptStatus::Ok);

    let payload: serde_cbor::Value = serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    let cells = match payload {
        serde_cbor::Value::Map(map) => map
            .into_iter()
            .find_map(|(k, v)| match (k, v) {
                (serde_cbor::Value::Text(t), serde_cbor::Value::Array(arr)) if t == "cells" => {
                    Some(arr)
                }
                _ => None,
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    assert_eq!(
        cells.len(),
        1,
        "expected sentinel cell for non-keyed reducer"
    );
    let key_len = match &cells[0] {
        serde_cbor::Value::Map(cell_map) => cell_map
            .iter()
            .find_map(|(k, v)| match (k, v) {
                (serde_cbor::Value::Text(t), serde_cbor::Value::Bytes(b)) if t == "key" => {
                    Some(b.len())
                }
                _ => None,
            })
            .unwrap_or_default(),
        _ => 0,
    };
    assert_eq!(key_len, 0, "sentinel key should be empty bytes");
}

#[test]
fn introspect_journal_head_matches_state_reader() {
    let mut world = world_with_state(b"payload");
    world
        .submit_event_result(fixtures::START_SCHEMA, &json!({ "id": "start" }))
        .expect("submit");
    world.tick_n(1).unwrap();

    let kernel = &mut world.kernel;
    let intent = IntentBuilder::new(
        EffectKind::introspect_journal_head(),
        "sys/query@1",
        &json!({}),
    )
    .build()
    .unwrap();

    let receipt = kernel
        .handle_internal_intent(&intent)
        .unwrap()
        .expect("handled");
    assert_eq!(receipt.status, ReceiptStatus::Ok);
    let payload: serde_cbor::Value = serde_cbor::from_slice(&receipt.payload_cbor).unwrap();
    let meta_map = match payload {
        serde_cbor::Value::Map(map) => map
            .into_iter()
            .find_map(|(k, v)| match (k, v) {
                (serde_cbor::Value::Text(t), m) if t == "meta" => Some(m),
                _ => None,
            })
            .expect("meta in receipt"),
        _ => panic!("unexpected payload shape"),
    };
    let meta = kernel.get_journal_head();
    let (jh, mh) = match meta_map {
        serde_cbor::Value::Map(map) => {
            let mut jh = None;
            let mut mh = None;
            for (k, v) in map {
                match (k, v) {
                    (serde_cbor::Value::Text(t), serde_cbor::Value::Integer(i))
                        if t == "journal_height" =>
                    {
                        jh = Some(i as u64);
                    }
                    (serde_cbor::Value::Text(t), serde_cbor::Value::Bytes(b))
                        if t == "manifest_hash" =>
                    {
                        mh = Some(b);
                    }
                    _ => {}
                }
            }
            (jh.expect("journal_height"), mh.expect("manifest_hash"))
        }
        _ => panic!("meta not a map"),
    };
    assert_eq!(jh, meta.journal_height);
    assert_eq!(mh, meta.manifest_hash.as_bytes());
}
