mod helpers;
use helpers::fixtures;
use aos_kernel::{Consistency, StateReader};
use aos_wasm_abi::ReducerOutput;
use indexmap::IndexMap;

/// Build a test world with a single stub reducer whose state is set to `payload` on first event.
fn test_world_with_state(payload: &[u8]) -> fixtures::TestWorld {
    let store = fixtures::new_mem_store();

    // Stub reducer that always returns the provided state.
    let module = fixtures::stub_reducer_module(
        &store,
        "com.acme/Store@1",
        &ReducerOutput {
            state: Some(payload.to_vec()),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );

    // Simple schema for start event routed to the reducer.
    let start_schema = fixtures::def_text_record_schema(fixtures::START_SCHEMA, vec![]);

    let routing = vec![fixtures::routing_event(fixtures::START_SCHEMA, &module.name)];
    let mut loaded = fixtures::build_loaded_manifest(vec![], vec![], vec![module], routing);
    fixtures::insert_test_schemas(&mut loaded, vec![start_schema]);

    fixtures::TestWorld::with_store(store, loaded).expect("build world")
}

#[test]
fn head_reads_state_and_meta() {
    let mut world = test_world_with_state(b"hello");
    world.submit_event(fixtures::START_SCHEMA, &fixtures::empty_value_literal());
    world.tick_n(1).unwrap();

    let read = world
        .kernel
        .get_reducer_state("com.acme/Store@1", None, Consistency::Head)
        .expect("head read");

    assert_eq!(read.value.as_deref(), Some("hello".as_bytes()));
    assert!(read.meta.journal_height > 0);
    assert_eq!(read.meta.manifest_hash.to_hex().len() > 0, true);
}

#[test]
fn exact_errors_without_snapshot() {
    let mut world = test_world_with_state(b"hi");
    world.submit_event(fixtures::START_SCHEMA, &fixtures::empty_value_literal());
    world.tick_n(1).unwrap();

    let err = world
        .kernel
        .get_reducer_state("com.acme/Store@1", None, Consistency::Exact(999))
        .unwrap_err();
    assert!(format!("{err:?}").contains("SnapshotUnavailable"));
}

#[test]
fn exact_uses_snapshot_when_available() {
    let mut world = test_world_with_state(b"snap");
    world.submit_event(fixtures::START_SCHEMA, &fixtures::empty_value_literal());
    world.tick_n(1).unwrap();

    world.kernel.create_snapshot().unwrap();
    let snap_hash = world.kernel.snapshot_hash();
    let snap_height = world.kernel.heights().snapshot.unwrap();

    let read = world
        .kernel
        .get_reducer_state("com.acme/Store@1", None, Consistency::Exact(snap_height))
        .expect("exact read");

    assert_eq!(read.value.as_deref(), Some("snap".as_bytes()));
    assert_eq!(read.meta.journal_height, snap_height);
    assert_eq!(read.meta.snapshot_hash, snap_hash);
    assert_eq!(read.meta.manifest_hash.to_hex().len() > 0, true);
}
