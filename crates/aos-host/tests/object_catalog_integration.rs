#![cfg(feature = "test-fixtures")]

//! Integration tests for the ObjectCatalog reducer (sys/ObjectCatalog@1).
//!
//! Tests verify:
//! - Version increments correctly on registration
//! - Key routing matches meta.name
//! - Replay determinism
//! - Previous versions remain accessible
//!
//! These tests load the actual reducer WASM built in `crates/aos-sys` from
//! `target/wasm32-unknown-unknown/debug/object_catalog.wasm`. Build it first with:
//! `cargo build -p aos-sys --target wasm32-unknown-unknown`.

#![cfg(feature = "test-fixtures")]

#[path = "helpers.rs"]
mod helpers;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aos_air_types::{
    builtins, plan_literals::SchemaIndex, value_normalize::normalize_cbor_by_name,
};
use helpers::fixtures::{self, TestStore};
use aos_kernel::Kernel;
use aos_kernel::journal::OwnedJournalEntry;
use aos_kernel::journal::mem::MemJournal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ObjectRegistered {
    meta: ObjectMeta,
}

/// Helper to build ObjectRegistered event payload using canonical schema shape.
fn object_registered_event_value(name: &str, kind: &str, hash: &str, owner: &str) -> Vec<u8> {
    let meta = ObjectMeta {
        name: name.into(),
        kind: kind.into(),
        hash: hash.into(),
        tags: BTreeSet::new(),
        created_at: 1000,
        owner: owner.into(),
    };
    let event = ObjectRegistered { meta };
    serde_cbor::to_vec(&event).unwrap()
}

fn canonical_key_bytes(name: &str) -> Vec<u8> {
    let mut schemas = HashMap::new();
    for builtin in builtins::builtin_schemas() {
        schemas.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
    }
    let idx = SchemaIndex::new(schemas);
    normalize_cbor_by_name(
        &idx,
        "sys/ObjectKey@1",
        &serde_cbor::to_vec(&name.to_string()).unwrap(),
    )
    .unwrap()
    .bytes
}

/// Test that ObjectRegistered events are correctly routed and state is updated.
#[tokio::test]
async fn object_catalog_version_increment() {
    let store: Arc<TestStore> = fixtures::new_mem_store();
    let reducer = fixtures::reducer_module_from_target(
        &store,
        "sys/ObjectCatalog@1",
        "object_catalog.wasm",
        Some("sys/ObjectKey@1"),
        "sys/ObjectVersions@1",
        "sys/ObjectRegistered@1",
    );

    let routing = vec![aos_air_types::RoutingEvent {
        event: fixtures::schema("sys/ObjectRegistered@1"),
        reducer: reducer.name.clone(),
        key_field: Some("meta.name".into()),
    }];

    let mut manifest = fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing);
    fixtures::insert_test_schemas(
        &mut manifest,
        vec![
            aos_air_types::builtins::find_builtin_schema("sys/ObjectKey@1")
                .unwrap()
                .schema
                .clone(),
            aos_air_types::builtins::find_builtin_schema("sys/ObjectVersions@1")
                .unwrap()
                .schema
                .clone(),
            aos_air_types::builtins::find_builtin_schema("sys/ObjectMeta@1")
                .unwrap()
                .schema
                .clone(),
            aos_air_types::builtins::find_builtin_schema("sys/ObjectRegistered@1")
                .unwrap()
                .schema
                .clone(),
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
    kernel
        .submit_domain_event_result("sys/ObjectRegistered@1", event_payload)
        .expect("route event");
    let res = kernel.tick_until_idle();
    eprintln!("tick result: {:?}", res);
    res.unwrap();

    // Verify state after registration
    let key_bytes = canonical_key_bytes("artifacts/patch-001");
    let state_bytes = kernel
        .reducer_state_bytes("sys/ObjectCatalog@1", Some(&key_bytes))
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

    let reducer = fixtures::reducer_module_from_target(
        &store,
        "sys/ObjectCatalog@1",
        "object_catalog.wasm",
        Some("sys/ObjectKey@1"),
        "sys/ObjectVersions@1",
        "sys/ObjectRegistered@1",
    );

    let routing = vec![aos_air_types::RoutingEvent {
        event: fixtures::schema("sys/ObjectRegistered@1"),
        reducer: reducer.name.clone(),
        key_field: Some("meta.name".into()),
    }];

    let mut manifest = fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing);
    fixtures::insert_test_schemas(
        &mut manifest,
        vec![
            aos_air_types::builtins::find_builtin_schema("sys/ObjectKey@1")
                .unwrap()
                .schema
                .clone(),
            aos_air_types::builtins::find_builtin_schema("sys/ObjectVersions@1")
                .unwrap()
                .schema
                .clone(),
            aos_air_types::builtins::find_builtin_schema("sys/ObjectMeta@1")
                .unwrap()
                .schema
                .clone(),
            aos_air_types::builtins::find_builtin_schema("sys/ObjectRegistered@1")
                .unwrap()
                .schema
                .clone(),
        ],
    );

    let mut kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();

    // Submit events for two different object names using ExprValue format
    let event_a = object_registered_event_value("path/a", "kind-a", "sha256:aaa", "owner-a");
    let event_b = object_registered_event_value("path/b", "kind-b", "sha256:bbb", "owner-b");

    kernel
        .submit_domain_event_result("sys/ObjectRegistered@1", event_a)
        .expect("route a");
    kernel
        .submit_domain_event_result("sys/ObjectRegistered@1", event_b)
        .expect("route b");
    kernel.tick_until_idle().unwrap();

    // Verify both keys have separate state
    let state_a = kernel
        .reducer_state_bytes("sys/ObjectCatalog@1", Some(&canonical_key_bytes("path/a")))
        .unwrap();
    let state_b = kernel
        .reducer_state_bytes("sys/ObjectCatalog@1", Some(&canonical_key_bytes("path/b")))
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

    let build_manifest = || {
        let reducer = fixtures::reducer_module_from_target(
            &store,
            "sys/ObjectCatalog@1",
            "object_catalog.wasm",
            Some("sys/ObjectKey@1"),
            "sys/ObjectVersions@1",
            "sys/ObjectRegistered@1",
        );

        let routing = vec![aos_air_types::RoutingEvent {
            event: fixtures::schema("sys/ObjectRegistered@1"),
            reducer: reducer.name.clone(),
            key_field: Some("meta.name".into()),
        }];

        let mut manifest = fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing);
        fixtures::insert_test_schemas(
            &mut manifest,
            vec![
                aos_air_types::builtins::find_builtin_schema("sys/ObjectKey@1")
                    .unwrap()
                    .schema
                    .clone(),
                aos_air_types::builtins::find_builtin_schema("sys/ObjectVersions@1")
                    .unwrap()
                    .schema
                    .clone(),
                aos_air_types::builtins::find_builtin_schema("sys/ObjectMeta@1")
                    .unwrap()
                    .schema
                    .clone(),
                aos_air_types::builtins::find_builtin_schema("sys/ObjectRegistered@1")
                    .unwrap()
                    .schema
                    .clone(),
            ],
        );
        manifest
    };

    let manifest = build_manifest();
    let mut kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();

    // Submit event and tick using ExprValue format
    let event_payload =
        object_registered_event_value("replay/test", "test", "sha256:replay", "test");
    kernel
        .submit_domain_event_result("sys/ObjectRegistered@1", event_payload)
        .expect("route event");
    kernel.tick_until_idle().unwrap();

    // Capture state before snapshot
    let root_before = kernel
        .reducer_index_root("sys/ObjectCatalog@1")
        .expect("index root");
    let replay_key = canonical_key_bytes("replay/test");
    let state_before = kernel
        .reducer_state_bytes("sys/ObjectCatalog@1", Some(&replay_key))
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
        .reducer_state_bytes("sys/ObjectCatalog@1", Some(&replay_key))
        .unwrap()
        .expect("replay state");

    assert_eq!(
        root_before, root_after,
        "index root should match after replay"
    );
    assert_eq!(state_before, state_after, "state should match after replay");
}
