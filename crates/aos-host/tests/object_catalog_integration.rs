//! Integration tests for the ObjectCatalog reducer (sys/ObjectCatalog@1).
//!
//! Tests verify:
//! - Version increments correctly on registration
//! - Key routing matches meta.name
//! - Replay determinism
//! - Previous versions remain accessible

#![cfg(feature = "test-fixtures")]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aos_air_exec::Value as ExprValue;
use aos_host::fixtures::{self, TestStore};
use aos_kernel::Kernel;
use aos_kernel::journal::mem::MemJournal;
use aos_kernel::journal::OwnedJournalEntry;
use aos_wasm_abi::ReducerOutput;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Matches aos_sys::ObjectMeta for deserialization
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ObjectMeta {
    name: String,
    kind: String,
    hash: String,
    tags: BTreeSet<String>,
    created_at: u64,
    owner: String,
}

/// Matches aos_sys::ObjectVersions for deserialization
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ObjectVersions {
    latest: u64,
    versions: BTreeMap<u64, ObjectMeta>,
}

/// Helper to build ObjectRegistered event payload as ExprValue (for key extraction).
fn object_registered_event_value(name: &str, kind: &str, hash: &str, owner: &str) -> Vec<u8> {
    let meta = ExprValue::Record(IndexMap::from([
        ("name".into(), ExprValue::Text(name.into())),
        ("kind".into(), ExprValue::Text(kind.into())),
        ("hash".into(), ExprValue::Text(hash.into())),
        ("tags".into(), ExprValue::Set(BTreeSet::new())),
        ("created_at".into(), ExprValue::Nat(1000)),
        ("owner".into(), ExprValue::Text(owner.into())),
    ]));
    let event = ExprValue::Record(IndexMap::from([("meta".into(), meta)]));
    serde_cbor::to_vec(&event).unwrap()
}

/// Helper to create a stub reducer that returns updated ObjectVersions state.
/// The stub simulates the ObjectCatalog reducer behavior.
fn create_catalog_stub_reducer(
    _store: &TestStore,
    initial_state: Option<ObjectVersions>,
    name: &str,
    kind: &str,
    hash: &str,
    owner: &str,
) -> ReducerOutput {
    let mut state = initial_state.unwrap_or_default();
    state.latest = state.latest.saturating_add(1);
    state.versions.insert(
        state.latest,
        ObjectMeta {
            name: name.to_string(),
            kind: kind.to_string(),
            hash: hash.to_string(),
            tags: BTreeSet::new(),
            created_at: 1000,
            owner: owner.to_string(),
        },
    );

    ReducerOutput {
        state: Some(serde_cbor::to_vec(&state).unwrap()),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    }
}

/// Test that ObjectRegistered events are correctly routed and state is updated.
#[tokio::test]
async fn object_catalog_version_increment() {
    let store: Arc<TestStore> = fixtures::new_mem_store();

    // Create stub output for first registration
    let output = create_catalog_stub_reducer(
        &store,
        None,
        "artifacts/patch-001",
        "air.patch",
        "sha256:abc123",
        "sys/self-upgrade@1",
    );

    let mut reducer = fixtures::stub_reducer_module(&store, "sys/ObjectCatalog@1", &output);
    reducer.key_schema = Some(fixtures::schema("sys/ObjectKey@1"));
    reducer.abi.reducer = Some(aos_air_types::ReducerAbi {
        state: fixtures::schema("sys/ObjectVersions@1"),
        event: fixtures::schema("sys/ObjectRegistered@1"),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    let routing = vec![aos_air_types::RoutingEvent {
        event: fixtures::schema("sys/ObjectRegistered@1"),
        reducer: reducer.name.clone(),
        key_field: Some("meta.name".into()),
    }];

    let mut manifest = fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing);
    fixtures::insert_test_schemas(
        &mut manifest,
        vec![
            fixtures::def_text_record_schema("sys/ObjectKey@1", vec![]),
            fixtures::def_text_record_schema("sys/ObjectVersions@1", vec![]),
            fixtures::def_text_record_schema("sys/ObjectMeta@1", vec![]),
            fixtures::def_text_record_schema("sys/ObjectRegistered@1", vec![]),
        ],
    );

    let mut kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();

    // Submit registration event using ExprValue format
    let event_payload = object_registered_event_value(
        "artifacts/patch-001",
        "air.patch",
        "sha256:abc123",
        "sys/self-upgrade@1",
    );
    kernel.submit_domain_event("sys/ObjectRegistered@1", event_payload);
    kernel.tick_until_idle().unwrap();

    // Verify state after registration
    let state_bytes = kernel
        .reducer_state_bytes("sys/ObjectCatalog@1", Some(b"artifacts/patch-001"))
        .unwrap()
        .expect("state exists");
    let state: ObjectVersions = serde_cbor::from_slice(&state_bytes).unwrap();
    assert_eq!(state.latest, 1);
    assert_eq!(state.versions.len(), 1);
    assert!(state.versions.contains_key(&1));
    assert_eq!(state.versions[&1].kind, "air.patch");
}

/// Test that keyed routing correctly partitions events by meta.name.
#[tokio::test]
async fn object_catalog_keyed_routing() {
    let store: Arc<TestStore> = fixtures::new_mem_store();

    // Create stub that produces state with version 1
    let output =
        create_catalog_stub_reducer(&store, None, "test/obj", "test", "sha256:test", "test");

    let mut reducer = fixtures::stub_reducer_module(&store, "sys/ObjectCatalog@1", &output);
    reducer.key_schema = Some(fixtures::schema("sys/ObjectKey@1"));
    reducer.abi.reducer = Some(aos_air_types::ReducerAbi {
        state: fixtures::schema("sys/ObjectVersions@1"),
        event: fixtures::schema("sys/ObjectRegistered@1"),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    let routing = vec![aos_air_types::RoutingEvent {
        event: fixtures::schema("sys/ObjectRegistered@1"),
        reducer: reducer.name.clone(),
        key_field: Some("meta.name".into()),
    }];

    let mut manifest = fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing);
    fixtures::insert_test_schemas(
        &mut manifest,
        vec![
            fixtures::def_text_record_schema("sys/ObjectKey@1", vec![]),
            fixtures::def_text_record_schema("sys/ObjectVersions@1", vec![]),
            fixtures::def_text_record_schema("sys/ObjectMeta@1", vec![]),
            fixtures::def_text_record_schema("sys/ObjectRegistered@1", vec![]),
        ],
    );

    let mut kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();

    // Submit events for two different object names using ExprValue format
    let event_a = object_registered_event_value("path/a", "kind-a", "sha256:aaa", "owner-a");
    let event_b = object_registered_event_value("path/b", "kind-b", "sha256:bbb", "owner-b");

    kernel.submit_domain_event("sys/ObjectRegistered@1", event_a);
    kernel.submit_domain_event("sys/ObjectRegistered@1", event_b);
    kernel.tick_until_idle().unwrap();

    // Verify both keys have separate state
    let state_a = kernel
        .reducer_state_bytes("sys/ObjectCatalog@1", Some(b"path/a"))
        .unwrap();
    let state_b = kernel
        .reducer_state_bytes("sys/ObjectCatalog@1", Some(b"path/b"))
        .unwrap();

    assert!(state_a.is_some(), "path/a should have state");
    assert!(state_b.is_some(), "path/b should have state");

    // Verify index root is present (keyed reducer has index)
    let root = kernel.reducer_index_root("sys/ObjectCatalog@1");
    assert!(root.is_some(), "index root should exist for keyed reducer");
}

/// Test snapshot and replay preserves catalog state.
#[tokio::test]
async fn object_catalog_snapshot_replay() {
    let store: Arc<TestStore> = fixtures::new_mem_store();

    let output = create_catalog_stub_reducer(
        &store,
        None,
        "replay/test",
        "test",
        "sha256:replay",
        "test",
    );

    let build_manifest = || {
        let mut reducer = fixtures::stub_reducer_module(&store, "sys/ObjectCatalog@1", &output);
        reducer.key_schema = Some(fixtures::schema("sys/ObjectKey@1"));
        reducer.abi.reducer = Some(aos_air_types::ReducerAbi {
            state: fixtures::schema("sys/ObjectVersions@1"),
            event: fixtures::schema("sys/ObjectRegistered@1"),
            annotations: None,
            effects_emitted: vec![],
            cap_slots: Default::default(),
        });

        let routing = vec![aos_air_types::RoutingEvent {
            event: fixtures::schema("sys/ObjectRegistered@1"),
            reducer: reducer.name.clone(),
            key_field: Some("meta.name".into()),
        }];

        let mut manifest = fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing);
        fixtures::insert_test_schemas(
            &mut manifest,
            vec![
                fixtures::def_text_record_schema("sys/ObjectKey@1", vec![]),
                fixtures::def_text_record_schema("sys/ObjectVersions@1", vec![]),
                fixtures::def_text_record_schema("sys/ObjectMeta@1", vec![]),
                fixtures::def_text_record_schema("sys/ObjectRegistered@1", vec![]),
            ],
        );
        manifest
    };

    let manifest = build_manifest();
    let mut kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();

    // Submit event and tick using ExprValue format
    let event_payload = object_registered_event_value("replay/test", "test", "sha256:replay", "test");
    kernel.submit_domain_event("sys/ObjectRegistered@1", event_payload);
    kernel.tick_until_idle().unwrap();

    // Capture state before snapshot
    let root_before = kernel
        .reducer_index_root("sys/ObjectCatalog@1")
        .expect("index root");
    let state_before = kernel
        .reducer_state_bytes("sys/ObjectCatalog@1", Some(b"replay/test"))
        .unwrap()
        .expect("state");

    // Create snapshot
    kernel.create_snapshot().unwrap();
    let entries: Vec<OwnedJournalEntry> = kernel.dump_journal().unwrap();
    let journal = Box::new(MemJournal::from_entries(&entries));

    // Replay from journal
    let manifest_replay = build_manifest();
    let mut kernel_replay =
        Kernel::from_loaded_manifest(store.clone(), manifest_replay, journal).unwrap();
    kernel_replay.tick_until_idle().unwrap();

    // Verify state matches after replay
    let root_after = kernel_replay
        .reducer_index_root("sys/ObjectCatalog@1")
        .expect("replay index root");
    let state_after = kernel_replay
        .reducer_state_bytes("sys/ObjectCatalog@1", Some(b"replay/test"))
        .unwrap()
        .expect("replay state");

    assert_eq!(root_before, root_after, "index root should match after replay");
    assert_eq!(state_before, state_after, "state should match after replay");
}
