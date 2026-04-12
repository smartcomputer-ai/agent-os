mod common;

use std::collections::BTreeMap;

use aos_effect_types::HashRef;
use aos_node::api::{WorkspaceApplyOp, WorkspaceApplyRequest};
use aos_node_local::LocalControl;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

#[test]
fn local_control_exposes_workspace_root_read_mutate_and_diff_surface()
-> Result<(), Box<dyn std::error::Error>> {
    let (_temp, paths) = common::temp_state_root();
    let control = LocalControl::open_batch(paths.root())?;

    let empty = control.workspace_empty_root()?;
    let empty_entries = control.workspace_entries(&empty, None, Some("dir"), None, 100)?;
    assert!(empty_entries.entries.is_empty());

    let blob = control.put_blob(b"external-by-ref", None)?;
    let blob_hash = HashRef::new(blob.hash)?;

    let mut annotations_patch = BTreeMap::new();
    annotations_patch.insert("sha256".into(), Some(blob_hash.clone()));

    let applied = control.workspace_apply(
        empty.clone(),
        WorkspaceApplyRequest {
            operations: vec![
                WorkspaceApplyOp::WriteBytes {
                    path: "notes/todo.txt".into(),
                    bytes_b64: BASE64_STANDARD.encode("hello workspace"),
                    mode: None,
                },
                WorkspaceApplyOp::SetAnnotations {
                    path: Some("notes/todo.txt".into()),
                    annotations_patch,
                },
                WorkspaceApplyOp::WriteRef {
                    path: "notes/blob.txt".into(),
                    blob_hash: blob_hash.clone(),
                    mode: None,
                },
            ],
        },
    )?;
    assert_eq!(applied.base_root_hash, empty);

    let dir_entries = control.workspace_entries(
        &applied.new_root_hash,
        Some("notes"),
        Some("dir"),
        None,
        100,
    )?;
    assert_eq!(dir_entries.entries.len(), 2);

    let subtree_entries =
        control.workspace_entries(&applied.new_root_hash, None, Some("subtree"), None, 100)?;
    assert_eq!(subtree_entries.entries.len(), 3);

    let todo_ref = control.workspace_entry(&applied.new_root_hash, "notes/todo.txt")?;
    assert_eq!(
        todo_ref.as_ref().map(|entry| entry.kind.as_str()),
        Some("file")
    );

    let bytes = control.workspace_bytes(&applied.new_root_hash, "notes/todo.txt", None)?;
    assert_eq!(String::from_utf8(bytes)?, "hello workspace");
    let range = control.workspace_bytes(&applied.new_root_hash, "notes/todo.txt", Some((6, 15)))?;
    assert_eq!(String::from_utf8(range)?, "workspace");

    let annotations =
        control.workspace_annotations(&applied.new_root_hash, Some("notes/todo.txt"))?;
    assert_eq!(
        annotations
            .annotations
            .as_ref()
            .and_then(|map| map.get("sha256")),
        Some(&blob_hash)
    );

    let removed = control.workspace_apply(
        applied.new_root_hash.clone(),
        WorkspaceApplyRequest {
            operations: vec![WorkspaceApplyOp::Remove {
                path: "notes/blob.txt".into(),
            }],
        },
    )?;
    let diff = control.workspace_diff(
        &applied.new_root_hash,
        &removed.new_root_hash,
        Some("notes"),
    )?;
    assert!(!diff.changes.is_empty());

    Ok(())
}
