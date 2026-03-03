use std::sync::Arc;

use aos_air_types::{TypeExpr, TypeVariant, WorkflowAbi};
use aos_effects::builtins::{BlobGetReceipt, BlobPutParams};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_kernel::journal::mem::MemJournal;
use aos_kernel::journal::{JournalKind, JournalRecord};
use aos_store::Store;
use aos_wasm_abi::WorkflowEffect;
use helpers::fixtures::{self, START_SCHEMA, TestWorld};
use indexmap::IndexMap;

mod helpers;
use helpers::{def_text_record_schema, insert_test_schemas, text_type};

#[test]
fn blob_put_intent_is_externalized_in_journal() {
    let store = fixtures::new_mem_store();
    let mut world =
        TestWorld::with_store(store.clone(), blob_world_manifest(&store, "blob.put")).unwrap();
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("blob-put"))
        .expect("submit start");
    world.tick_n(1).expect("tick");

    let _intent = world
        .drain_effects()
        .expect("drain effects")
        .pop()
        .expect("blob.put intent");

    let entries = world.kernel.dump_journal().expect("dump journal");
    let record = entries
        .iter()
        .filter_map(|entry| serde_cbor::from_slice::<JournalRecord>(&entry.payload).ok())
        .find_map(|record| match record {
            JournalRecord::EffectIntent(record)
                if record.kind == aos_effects::EffectKind::BLOB_PUT =>
            {
                Some(record)
            }
            _ => None,
        })
        .expect("blob.put effect intent record");

    assert!(record.params_ref.is_some(), "params_ref should be set");
    assert!(record.params_size.is_some(), "params_size should be set");
    assert!(
        record.params_sha256.is_some(),
        "params_sha256 should be set"
    );
    assert!(
        record.params_cbor.is_empty(),
        "blob.put params should not be journaled inline"
    );

    let params_ref = record.params_ref.expect("params ref");
    let hash = aos_cbor::Hash::from_hex_str(params_ref.as_str()).expect("parse hash");
    let params_cbor = store.get_blob(hash).expect("load externalized params");
    let params: BlobPutParams = serde_cbor::from_slice(&params_cbor).expect("decode params");
    assert_eq!(params.bytes, b"journal-externalized".to_vec());
}

#[test]
fn blob_get_receipt_is_externalized_and_replay_requires_cas_dependency() {
    let store = fixtures::new_mem_store();
    let mut world =
        TestWorld::with_store(store.clone(), blob_world_manifest(&store, "blob.get")).unwrap();
    world
        .submit_event_result(START_SCHEMA, &fixtures::start_event("blob-get"))
        .expect("submit start");
    world.tick_n(1).expect("tick");

    let intent = world
        .drain_effects()
        .expect("drain effects")
        .pop()
        .expect("blob.get intent");
    let payload = b"blob-get-bytes".to_vec();
    world
        .kernel
        .handle_receipt(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "adapter.blob".into(),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&BlobGetReceipt {
                blob_ref: fixtures::fake_hash(0x44),
                size: payload.len() as u64,
                bytes: payload.clone(),
            })
            .unwrap(),
            cost_cents: None,
            signature: vec![7, 7],
        })
        .expect("handle receipt");

    let entries = world.kernel.dump_journal().expect("dump journal");
    let record = entries
        .iter()
        .filter_map(|entry| serde_cbor::from_slice::<JournalRecord>(&entry.payload).ok())
        .find_map(|record| match record {
            JournalRecord::EffectReceipt(record) if record.intent_hash == intent.intent_hash => {
                Some(record)
            }
            _ => None,
        })
        .expect("blob.get effect receipt record");

    assert!(record.payload_ref.is_some(), "payload_ref should be set");
    assert!(record.payload_size.is_some(), "payload_size should be set");
    assert!(
        record.payload_sha256.is_some(),
        "payload_sha256 should be set"
    );
    assert!(
        record.payload_cbor.is_empty(),
        "blob.get payload should not be journaled inline"
    );

    let payload_ref = record.payload_ref.expect("payload ref");
    let hash = aos_cbor::Hash::from_hex_str(payload_ref.as_str()).expect("parse hash");
    let payload_cbor = store.get_blob(hash).expect("load externalized payload");
    let receipt: BlobGetReceipt = serde_cbor::from_slice(&payload_cbor).expect("decode receipt");
    assert_eq!(receipt.bytes, payload);

    let store_missing = fixtures::new_mem_store();
    let replay_entries: Vec<_> = entries
        .iter()
        .filter(|entry| !matches!(entry.kind, JournalKind::Snapshot | JournalKind::Manifest))
        .cloned()
        .collect();
    let replay = TestWorld::with_store_and_journal(
        store_missing.clone(),
        blob_world_manifest(&store_missing, "blob.get"),
        Box::new(MemJournal::from_entries(&replay_entries)),
    );
    let err = match replay {
        Ok(_) => panic!("replay should fail without externalized CAS payload"),
        Err(err) => err,
    };
    assert!(
        format!("{err}").contains("missing_cas_dependency"),
        "unexpected error: {err}"
    );
}

fn blob_world_manifest(
    store: &Arc<fixtures::TestStore>,
    effect_kind: &str,
) -> aos_kernel::manifest::LoadedManifest {
    let effect = match effect_kind {
        "blob.put" => WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::BLOB_PUT,
            serde_cbor::to_vec(&BlobPutParams {
                bytes: b"journal-externalized".to_vec(),
                blob_ref: None,
                refs: None,
            })
            .unwrap(),
            "blob",
        ),
        "blob.get" => WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::BLOB_GET,
            serde_cbor::to_vec(&aos_effects::builtins::BlobGetParams {
                blob_ref: fixtures::fake_hash(0x10),
            })
            .unwrap(),
            "blob",
        ),
        other => panic!("unexpected effect kind {other}"),
    };
    let mut workflow = fixtures::stub_workflow_module(
        store,
        "com.acme/BlobWorkflow@1",
        &aos_wasm_abi::WorkflowOutput {
            state: Some(vec![0x01]),
            domain_events: vec![],
            effects: vec![effect],
            ann: None,
        },
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/BlobState@1"),
        event: fixtures::schema("com.acme/BlobEvent@1"),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![effect_kind.into()],
        cap_slots: Default::default(),
    });

    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow],
        vec![fixtures::routing_event(
            "com.acme/BlobEvent@1",
            "com.acme/BlobWorkflow@1",
        )],
    );
    if let Some(binding) = loaded
        .manifest
        .module_bindings
        .get_mut("com.acme/BlobWorkflow@1")
    {
        binding.slots.insert("blob".into(), "blob_cap".into());
    }
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            def_text_record_schema("com.acme/BlobState@1", vec![]),
            aos_air_types::DefSchema {
                name: "com.acme/BlobEvent@1".into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([(
                        "Start".into(),
                        TypeExpr::Ref(aos_air_types::TypeRef {
                            reference: fixtures::schema(START_SCHEMA),
                        }),
                    )]),
                }),
            },
        ],
    );
    loaded
}
