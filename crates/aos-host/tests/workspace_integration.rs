#![cfg(feature = "test-fixtures")]

//! Integration tests for the Workspace reducer and internal workspace effects.
//!
//! These tests load the actual reducer WASM built in `crates/aos-sys` from
//! `target/wasm32-unknown-unknown/debug/workspace.wasm`. Build it first with:
//! `cargo build -p aos-sys --target wasm32-unknown-unknown`.

#[path = "helpers.rs"]
mod helpers;

use std::collections::BTreeMap;
use std::sync::Arc;

use aos_effects::{EffectKind, IntentBuilder, ReceiptStatus};
use aos_kernel::Kernel;
use aos_store::Store;
use helpers::fixtures::{self, TestStore, TestWorld};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceCommitMeta {
    root_hash: String,
    owner: String,
    created_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceCommit {
    workspace: String,
    expected_head: Option<u64>,
    meta: WorkspaceCommitMeta,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceTree {
    entries: Vec<WorkspaceEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceEntry {
    name: String,
    kind: String,
    hash: String,
    size: u64,
    mode: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceResolveParams {
    workspace: String,
    version: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceResolveReceipt {
    exists: bool,
    resolved_version: Option<u64>,
    head: Option<u64>,
    root_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceListParams {
    root_hash: String,
    path: Option<String>,
    scope: Option<String>,
    cursor: Option<String>,
    limit: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceListEntry {
    path: String,
    kind: String,
    hash: Option<String>,
    size: Option<u64>,
    mode: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceListReceipt {
    entries: Vec<WorkspaceListEntry>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceReadRefParams {
    root_hash: String,
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceRefEntry {
    kind: String,
    hash: String,
    size: u64,
    mode: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceReadBytesParams {
    root_hash: String,
    path: String,
    range: Option<WorkspaceReadBytesRange>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceReadBytesRange {
    start: u64,
    end: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceWriteBytesParams {
    root_hash: String,
    path: String,
    #[serde(with = "serde_bytes")]
    bytes: Vec<u8>,
    mode: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceWriteBytesReceipt {
    new_root_hash: String,
    blob_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceRemoveParams {
    root_hash: String,
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceRemoveReceipt {
    new_root_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceDiffParams {
    root_a: String,
    root_b: String,
    prefix: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceDiffChange {
    path: String,
    kind: String,
    old_hash: Option<String>,
    new_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceDiffReceipt {
    changes: Vec<WorkspaceDiffChange>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(transparent)]
struct WorkspaceAnnotations(BTreeMap<String, String>);

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(transparent)]
struct WorkspaceAnnotationsPatch(BTreeMap<String, Option<String>>);

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsGetParams {
    root_hash: String,
    path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsGetReceipt {
    annotations: Option<WorkspaceAnnotations>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsSetParams {
    root_hash: String,
    path: Option<String>,
    annotations_patch: WorkspaceAnnotationsPatch,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsSetReceipt {
    new_root_hash: String,
    annotations_hash: String,
}

fn build_workspace_manifest(store: &Arc<TestStore>) -> aos_kernel::manifest::LoadedManifest {
    let reducer = fixtures::reducer_module_from_target(
        store,
        "sys/Workspace@1",
        "workspace.wasm",
        Some("sys/WorkspaceName@1"),
        "sys/WorkspaceHistory@1",
        "sys/WorkspaceCommit@1",
    );
    let routing = vec![aos_air_types::RoutingEvent {
        event: fixtures::schema("sys/WorkspaceCommit@1"),
        reducer: reducer.name.clone(),
        key_field: Some("workspace".into()),
    }];
    fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing)
}

fn commit_event(workspace: &str, root_hash: &str, expected_head: Option<u64>) -> Vec<u8> {
    let event = WorkspaceCommit {
        workspace: workspace.to_string(),
        expected_head,
        meta: WorkspaceCommitMeta {
            root_hash: root_hash.to_string(),
            owner: "tester".into(),
            created_at: 1234,
        },
    };
    serde_cbor::to_vec(&event).expect("encode commit event")
}

fn handle_internal<T: serde::de::DeserializeOwned>(
    kernel: &mut Kernel<TestStore>,
    intent: aos_effects::EffectIntent,
) -> T {
    let type_name = std::any::type_name::<T>();
    let receipt = kernel
        .handle_internal_intent(&intent)
        .expect("intent handled")
        .expect("receipt");
    assert_eq!(receipt.status, ReceiptStatus::Ok);
    receipt.payload().unwrap_or_else(|err| {
        panic!(
            "decode receipt for {} as {}: {:?}",
            intent.kind.as_str(),
            type_name,
            err
        )
    })
}

#[tokio::test]
async fn workspace_commit_and_resolve() {
    let store = fixtures::new_mem_store();
    let loaded = build_workspace_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), loaded).expect("world");

    let empty_root = store
        .put_node(&WorkspaceTree {
            entries: Vec::new(),
        })
        .expect("root tree");
    let empty_root_hash = empty_root.to_hex();

    world
        .kernel
        .submit_domain_event_result(
            "sys/WorkspaceCommit@1",
            commit_event("dev-workspace", &empty_root_hash, None),
        )
        .expect("submit commit");
    world.kernel.tick_until_idle().expect("tick");

    let params = WorkspaceResolveParams {
        workspace: "dev-workspace".into(),
        version: None,
    };
    let intent = IntentBuilder::new(EffectKind::workspace_resolve(), "sys/workspace@1", &params)
        .build()
        .expect("intent");
    let receipt: WorkspaceResolveReceipt = handle_internal(&mut world.kernel, intent);
    assert!(receipt.exists);
    assert_eq!(receipt.resolved_version, Some(1));
    assert_eq!(receipt.head, Some(1));
    assert_eq!(receipt.root_hash, Some(empty_root_hash.clone()));

    let missing = WorkspaceResolveParams {
        workspace: "dev-workspace".into(),
        version: Some(2),
    };
    let intent = IntentBuilder::new(EffectKind::workspace_resolve(), "sys/workspace@1", &missing)
        .build()
        .expect("intent");
    let receipt: WorkspaceResolveReceipt = handle_internal(&mut world.kernel, intent);
    assert!(!receipt.exists);
    assert_eq!(receipt.resolved_version, None);
    assert_eq!(receipt.root_hash, None);
}

#[tokio::test]
async fn workspace_tree_effects_roundtrip() {
    let store = fixtures::new_mem_store();
    let loaded = build_workspace_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), loaded).expect("world");

    let empty_root = store
        .put_node(&WorkspaceTree {
            entries: Vec::new(),
        })
        .expect("root tree");
    let empty_root_hash = empty_root.to_hex();

    let write_params = WorkspaceWriteBytesParams {
        root_hash: empty_root_hash.clone(),
        path: "src/lib.rs".into(),
        bytes: b"fn main() {}\n".to_vec(),
        mode: None,
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_write_bytes(),
        "sys/workspace@1",
        &write_params,
    )
    .build()
    .expect("intent");
    let write_receipt: WorkspaceWriteBytesReceipt = handle_internal(&mut world.kernel, intent);

    let list_params = WorkspaceListParams {
        root_hash: write_receipt.new_root_hash.clone(),
        path: None,
        scope: Some("dir".into()),
        cursor: None,
        limit: 0,
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_list(),
        "sys/workspace@1",
        &list_params,
    )
    .build()
    .expect("intent");
    let list_receipt: WorkspaceListReceipt = handle_internal(&mut world.kernel, intent);
    let paths: Vec<_> = list_receipt
        .entries
        .iter()
        .map(|e| e.path.as_str())
        .collect();
    assert_eq!(paths, vec!["src"]);

    let list_params = WorkspaceListParams {
        root_hash: write_receipt.new_root_hash.clone(),
        path: None,
        scope: Some("subtree".into()),
        cursor: None,
        limit: 0,
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_list(),
        "sys/workspace@1",
        &list_params,
    )
    .build()
    .expect("intent");
    let list_receipt: WorkspaceListReceipt = handle_internal(&mut world.kernel, intent);
    let paths: Vec<_> = list_receipt
        .entries
        .iter()
        .map(|e| e.path.as_str())
        .collect();
    assert_eq!(paths, vec!["src", "src/lib.rs"]);

    let read_ref_params = WorkspaceReadRefParams {
        root_hash: write_receipt.new_root_hash.clone(),
        path: "src/lib.rs".into(),
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_read_ref(),
        "sys/workspace@1",
        &read_ref_params,
    )
    .build()
    .expect("intent");
    let entry: Option<WorkspaceRefEntry> = handle_internal(&mut world.kernel, intent);
    let entry = entry.expect("entry");
    assert_eq!(entry.kind, "file");
    assert_eq!(entry.size, 13);

    let read_bytes_params = WorkspaceReadBytesParams {
        root_hash: write_receipt.new_root_hash.clone(),
        path: "src/lib.rs".into(),
        range: Some(WorkspaceReadBytesRange { start: 0, end: 7 }),
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_read_bytes(),
        "sys/workspace@1",
        &read_bytes_params,
    )
    .build()
    .expect("intent");
    let bytes: Vec<u8> = handle_internal(&mut world.kernel, intent);
    assert_eq!(bytes, b"fn main".to_vec());

    let write_params = WorkspaceWriteBytesParams {
        root_hash: write_receipt.new_root_hash.clone(),
        path: "README.md".into(),
        bytes: b"hello\n".to_vec(),
        mode: Some(0o644),
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_write_bytes(),
        "sys/workspace@1",
        &write_params,
    )
    .build()
    .expect("intent");
    let write_readme: WorkspaceWriteBytesReceipt = handle_internal(&mut world.kernel, intent);

    let diff_params = WorkspaceDiffParams {
        root_a: write_receipt.new_root_hash.clone(),
        root_b: write_readme.new_root_hash.clone(),
        prefix: None,
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_diff(),
        "sys/workspace@1",
        &diff_params,
    )
    .build()
    .expect("intent");
    let diff: WorkspaceDiffReceipt = handle_internal(&mut world.kernel, intent);
    assert!(
        diff.changes
            .iter()
            .any(|c| c.path == "README.md" && c.old_hash.is_none() && c.new_hash.is_some())
    );

    let diff_params = WorkspaceDiffParams {
        root_a: write_receipt.new_root_hash.clone(),
        root_b: write_readme.new_root_hash.clone(),
        prefix: Some("README.md".into()),
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_diff(),
        "sys/workspace@1",
        &diff_params,
    )
    .build()
    .expect("intent");
    let diff: WorkspaceDiffReceipt = handle_internal(&mut world.kernel, intent);
    assert_eq!(diff.changes.len(), 1);
    assert_eq!(diff.changes[0].path, "README.md");
    assert!(diff.changes[0].old_hash.is_none());
    assert!(diff.changes[0].new_hash.is_some());

    let remove_params = WorkspaceRemoveParams {
        root_hash: write_readme.new_root_hash.clone(),
        path: "README.md".into(),
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_remove(),
        "sys/workspace@1",
        &remove_params,
    )
    .build()
    .expect("intent");
    let removed: WorkspaceRemoveReceipt = handle_internal(&mut world.kernel, intent);

    let read_ref_params = WorkspaceReadRefParams {
        root_hash: removed.new_root_hash.clone(),
        path: "README.md".into(),
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_read_ref(),
        "sys/workspace@1",
        &read_ref_params,
    )
    .build()
    .expect("intent");
    let entry: Option<WorkspaceRefEntry> = handle_internal(&mut world.kernel, intent);
    assert!(entry.is_none());

    let remove_params = WorkspaceRemoveParams {
        root_hash: removed.new_root_hash.clone(),
        path: "src/lib.rs".into(),
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_remove(),
        "sys/workspace@1",
        &remove_params,
    )
    .build()
    .expect("intent");
    let removed_file: WorkspaceRemoveReceipt = handle_internal(&mut world.kernel, intent);

    let remove_params = WorkspaceRemoveParams {
        root_hash: removed_file.new_root_hash.clone(),
        path: "src".into(),
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_remove(),
        "sys/workspace@1",
        &remove_params,
    )
    .build()
    .expect("intent");
    let removed_dir: WorkspaceRemoveReceipt = handle_internal(&mut world.kernel, intent);

    let list_params = WorkspaceListParams {
        root_hash: removed_dir.new_root_hash,
        path: None,
        scope: Some("dir".into()),
        cursor: None,
        limit: 0,
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_list(),
        "sys/workspace@1",
        &list_params,
    )
    .build()
    .expect("intent");
    let list_receipt: WorkspaceListReceipt = handle_internal(&mut world.kernel, intent);
    assert!(list_receipt.entries.is_empty());
}

#[tokio::test]
async fn workspace_annotations_roundtrip_on_root() {
    let store = fixtures::new_mem_store();
    let loaded = build_workspace_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), loaded).expect("world");

    let empty_root = store
        .put_node(&WorkspaceTree {
            entries: Vec::new(),
        })
        .expect("root tree");

    let key = "sys/commit.message".to_string();
    let val_hash = aos_cbor::Hash::of_bytes(b"message body").to_hex();

    let mut patch_map = BTreeMap::new();
    patch_map.insert(key.clone(), Some(val_hash.clone()));
    let set_params = WorkspaceAnnotationsSetParams {
        root_hash: empty_root.to_hex(),
        path: None,
        annotations_patch: WorkspaceAnnotationsPatch(patch_map),
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_annotations_set(),
        "sys/workspace@1",
        &set_params,
    )
    .build()
    .expect("intent");
    let set_receipt: WorkspaceAnnotationsSetReceipt = handle_internal(&mut world.kernel, intent);
    assert!(set_receipt.annotations_hash.starts_with("sha256:"));

    let get_params = WorkspaceAnnotationsGetParams {
        root_hash: set_receipt.new_root_hash.clone(),
        path: None,
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_annotations_get(),
        "sys/workspace@1",
        &get_params,
    )
    .build()
    .expect("intent");
    let get_receipt: WorkspaceAnnotationsGetReceipt = handle_internal(&mut world.kernel, intent);
    let annotations = get_receipt.annotations.expect("annotations");
    assert_eq!(annotations.0.get(&key), Some(&val_hash));

    let mut delete_patch = BTreeMap::new();
    delete_patch.insert(key.clone(), None);
    let delete_params = WorkspaceAnnotationsSetParams {
        root_hash: set_receipt.new_root_hash,
        path: None,
        annotations_patch: WorkspaceAnnotationsPatch(delete_patch),
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_annotations_set(),
        "sys/workspace@1",
        &delete_params,
    )
    .build()
    .expect("intent");
    let delete_receipt: WorkspaceAnnotationsSetReceipt = handle_internal(&mut world.kernel, intent);

    let get_params = WorkspaceAnnotationsGetParams {
        root_hash: delete_receipt.new_root_hash,
        path: None,
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_annotations_get(),
        "sys/workspace@1",
        &get_params,
    )
    .build()
    .expect("intent");
    let get_receipt: WorkspaceAnnotationsGetReceipt = handle_internal(&mut world.kernel, intent);
    let annotations = get_receipt.annotations.expect("annotations");
    assert!(annotations.0.is_empty());
}

#[tokio::test]
async fn workspace_annotations_survive_file_write() {
    let store = fixtures::new_mem_store();
    let loaded = build_workspace_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), loaded).expect("world");

    let empty_root = store
        .put_node(&WorkspaceTree {
            entries: Vec::new(),
        })
        .expect("root tree");

    let write_params = WorkspaceWriteBytesParams {
        root_hash: empty_root.to_hex(),
        path: "notes/readme.txt".into(),
        bytes: b"hello".to_vec(),
        mode: None,
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_write_bytes(),
        "sys/workspace@1",
        &write_params,
    )
    .build()
    .expect("intent");
    let write_receipt: WorkspaceWriteBytesReceipt = handle_internal(&mut world.kernel, intent);

    let key = "doc.tag".to_string();
    let val_hash = aos_cbor::Hash::of_bytes(b"draft").to_hex();
    let mut patch_map = BTreeMap::new();
    patch_map.insert(key.clone(), Some(val_hash.clone()));
    let set_params = WorkspaceAnnotationsSetParams {
        root_hash: write_receipt.new_root_hash.clone(),
        path: Some("notes/readme.txt".into()),
        annotations_patch: WorkspaceAnnotationsPatch(patch_map),
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_annotations_set(),
        "sys/workspace@1",
        &set_params,
    )
    .build()
    .expect("intent");
    let set_receipt: WorkspaceAnnotationsSetReceipt = handle_internal(&mut world.kernel, intent);

    let write_params = WorkspaceWriteBytesParams {
        root_hash: set_receipt.new_root_hash.clone(),
        path: "notes/readme.txt".into(),
        bytes: b"hello again".to_vec(),
        mode: None,
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_write_bytes(),
        "sys/workspace@1",
        &write_params,
    )
    .build()
    .expect("intent");
    let write_receipt: WorkspaceWriteBytesReceipt = handle_internal(&mut world.kernel, intent);

    let get_params = WorkspaceAnnotationsGetParams {
        root_hash: write_receipt.new_root_hash,
        path: Some("notes/readme.txt".into()),
    };
    let intent = IntentBuilder::new(
        EffectKind::workspace_annotations_get(),
        "sys/workspace@1",
        &get_params,
    )
    .build()
    .expect("intent");
    let get_receipt: WorkspaceAnnotationsGetReceipt = handle_internal(&mut world.kernel, intent);
    let annotations = get_receipt.annotations.expect("annotations");
    assert_eq!(annotations.0.get(&key), Some(&val_hash));
}
