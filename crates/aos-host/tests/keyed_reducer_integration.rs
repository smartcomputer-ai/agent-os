#![cfg(feature = "test-fixtures")]

#[path = "helpers.rs"]
mod helpers;

use std::sync::Arc;

use aos_cbor::Hash;
use aos_kernel::Kernel;
use aos_kernel::cell_index::CellIndex;
use aos_kernel::journal::mem::MemJournal;
use aos_kernel::journal::{JournalKind, OwnedJournalEntry};
use aos_kernel::snapshot::KernelSnapshot;
use aos_store::Store;
use aos_wasm_abi::ReducerOutput;
use helpers::fixtures::{self, TestStore};

/// Full-path integration for keyed reducers:
/// - routes keyed events,
/// - writes cell states into the CAS-backed index,
/// - snapshots and replays,
/// - keeps both keys accessible after replay.
#[tokio::test]
async fn keyed_reducer_integration_flow() {
    let store: Arc<TestStore> = fixtures::new_mem_store();

    // Helper to build the manifest so we can reuse it for replay.
    let build_manifest = || {
        let mut reducer = fixtures::stub_reducer_module(
            &store,
            "com.acme/Keyed@1",
            &ReducerOutput {
                state: Some(vec![0xAA]),
                domain_events: vec![],
                effects: vec![],
                ann: None,
            },
        );
        reducer.key_schema = Some(fixtures::schema("com.acme/Key@1"));
        reducer.abi.reducer = Some(aos_air_types::ReducerAbi {
            state: fixtures::schema("com.acme/State@1"),
            event: fixtures::schema("com.acme/Event@1"),
            context: Some(fixtures::schema("sys/ReducerContext@1")),
            annotations: None,
            effects_emitted: vec![],
            cap_slots: Default::default(),
        });

        let routing = vec![aos_air_types::RoutingEvent {
            event: fixtures::schema("com.acme/Event@1"),
            reducer: reducer.name.clone(),
            key_field: Some("id".into()),
        }];

        let mut manifest = fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing);
        fixtures::insert_test_schemas(
            &mut manifest,
            vec![
                fixtures::def_text_record_schema("com.acme/State@1", vec![]),
                aos_air_types::DefSchema {
                    name: "com.acme/Key@1".into(),
                    ty: fixtures::text_type(),
                },
                fixtures::def_text_record_schema(
                    "com.acme/Event@1",
                    vec![("id", fixtures::text_type())],
                ),
            ],
        );
        manifest
    };

    let manifest = build_manifest();
    let mut kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();

    // Two keyed events.
    let evt_payload = |id: &str| serde_cbor::to_vec(&serde_json::json!({ "id": id })).unwrap();
    kernel
        .submit_domain_event_result("com.acme/Event@1", evt_payload("k1"))
        .expect("submit k1");
    kernel
        .submit_domain_event_result("com.acme/Event@1", evt_payload("k2"))
        .expect("submit k2");
    kernel.tick_until_idle().unwrap();

    // Index root present and both cells accessible.
    let root = kernel
        .reducer_index_root("com.acme/Keyed@1")
        .expect("index root present");
    let index = CellIndex::new(store.as_ref());
    let metas: Vec<_> = index.iter(root).map(|m| m.unwrap()).collect();
    assert_eq!(metas.len(), 2);
    let key_strings: Vec<String> = metas
        .iter()
        .map(|m| serde_cbor::from_slice(&m.key_bytes).unwrap())
        .collect();
    assert!(key_strings.contains(&"k1".to_string()));
    assert!(key_strings.contains(&"k2".to_string()));
    for meta in &metas {
        let state = kernel
            .reducer_state_bytes("com.acme/Keyed@1", Some(&meta.key_bytes))
            .unwrap()
            .expect("cell state");
        assert_eq!(state, vec![0xAA]);
    }

    // Snapshot to pin index root.
    kernel.create_snapshot().unwrap();
    let entries: Vec<OwnedJournalEntry> = kernel.dump_journal().unwrap();
    let snap_record = entries
        .iter()
        .rev()
        .find(|e| matches!(e.kind, JournalKind::Snapshot))
        .expect("snapshot record");
    let journal = Box::new(MemJournal::from_entries(&entries));

    // Rehydrate a fresh kernel from snapshot + shared store.
    let manifest_replay = build_manifest();
    let mut kernel_replay =
        Kernel::from_loaded_manifest(store.clone(), manifest_replay, journal).unwrap();
    kernel_replay.tick_until_idle().unwrap();

    let root_replay = kernel_replay
        .reducer_index_root("com.acme/Keyed@1")
        .expect("replay root");
    assert_eq!(root, root_replay, "index root should persist across replay");

    // Verify cells via API and direct index iteration.
    let index = CellIndex::new(store.as_ref());
    let metas: Vec<_> = index.iter(root_replay).map(|m| m.unwrap()).collect();
    assert_eq!(metas.len(), 2);
    let keys: Vec<String> = metas
        .iter()
        .map(|m| serde_cbor::from_slice(&m.key_bytes).unwrap())
        .collect();
    assert!(keys.contains(&"k1".to_string()));
    assert!(keys.contains(&"k2".to_string()));
    for meta in metas {
        let state_hash = Hash::from_bytes(&meta.state_hash).unwrap();
        let state = store.get_blob(state_hash).unwrap();
        assert_eq!(state, vec![0xAA], "state bytes should match stored blob");
    }

    // Ensure snapshot encoded the root we used (paranoia check).
    let snap_record_decoded: aos_kernel::journal::JournalRecord =
        serde_cbor::from_slice(&snap_record.payload).unwrap();
    let snapshot_hash = match snap_record_decoded {
        aos_kernel::journal::JournalRecord::Snapshot(s) => s.snapshot_ref,
        _ => panic!("expected snapshot record"),
    };
    let snapshot_bytes = store
        .get_blob(aos_cbor::Hash::from_hex_str(&snapshot_hash).unwrap())
        .unwrap();
    let snapshot: KernelSnapshot = serde_cbor::from_slice(&snapshot_bytes).unwrap();
    let root_in_snapshot = snapshot
        .reducer_index_roots()
        .iter()
        .find(|(name, _)| name == "com.acme/Keyed@1")
        .map(|(_, h)| *h)
        .expect("root in snapshot");
    assert_eq!(root_in_snapshot, *root.as_bytes());
}
