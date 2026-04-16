use std::collections::{BTreeMap, HashMap};

use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effect_types::{
    WorkspaceAnnotations, WorkspaceAnnotationsGetReceipt, WorkspaceAnnotationsPatch,
    WorkspaceAnnotationsSetReceipt, WorkspaceDiffChange, WorkspaceDiffReceipt, WorkspaceListEntry,
    WorkspaceListReceipt, WorkspaceReadRefReceipt, WorkspaceRefEntry, WorkspaceRemoveReceipt,
    WorkspaceWriteBytesReceipt, WorkspaceWriteRefReceipt,
};
use aos_kernel::KernelError;
use aos_kernel::Store;
use serde::{Deserialize, Serialize};

const MODE_FILE_DEFAULT: u64 = 0o100644;
const MODE_FILE_EXEC: u64 = 0o100755;
const MODE_DIR: u64 = 0o040000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceEntry {
    name: String,
    kind: String,
    hash: HashRef,
    size: u64,
    mode: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    annotations_hash: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceTree {
    entries: Vec<WorkspaceEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    annotations_hash: Option<HashRef>,
}

pub fn empty_root<S: Store>(store: &S) -> Result<HashRef, KernelError> {
    let hash = store.put_node(&WorkspaceTree {
        entries: Vec::new(),
        annotations_hash: None,
    })?;
    hash_ref_from_hash(&hash)
}

pub fn list<S: Store>(
    store: &S,
    root_hash: &HashRef,
    path: Option<&str>,
    scope: Option<&str>,
    cursor: Option<&str>,
    limit: u64,
) -> Result<WorkspaceListReceipt, KernelError> {
    let root_hash = parse_hash_ref(root_hash)?;
    let path_segments = path.map(validate_path).transpose()?.unwrap_or_default();
    let dir_hash = resolve_dir_hash(store, &root_hash, &path_segments)?;
    let base = path_segments.join("/");
    let mut entries = Vec::new();
    match scope.unwrap_or("dir") {
        "dir" => {
            let tree = load_tree(store, &dir_hash)?;
            for entry in tree.entries {
                entries.push(list_entry_from_tree(join_path(&base, &entry.name), &entry));
            }
        }
        "subtree" => {
            let mut tree_entries = Vec::new();
            collect_subtree_entries(store, &dir_hash, &base, &mut tree_entries)?;
            for (path, entry) in tree_entries {
                entries.push(list_entry_from_tree(path, &entry));
            }
        }
        other => return Err(KernelError::Query(format!("invalid scope '{other}'"))),
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let start = cursor
        .map(|cursor| {
            entries
                .iter()
                .position(|entry| entry.path.as_str() > cursor)
                .unwrap_or(entries.len())
        })
        .unwrap_or(0);
    let end = if limit == 0 {
        entries.len()
    } else {
        (start + limit as usize).min(entries.len())
    };
    let next_cursor = if end < entries.len() && end > start {
        Some(entries[end - 1].path.clone())
    } else {
        None
    };
    let entries = if start < end {
        entries[start..end].to_vec()
    } else {
        Vec::new()
    };
    Ok(WorkspaceListReceipt {
        entries,
        next_cursor,
    })
}

pub fn read_ref<S: Store>(
    store: &S,
    root_hash: &HashRef,
    path: &str,
) -> Result<WorkspaceReadRefReceipt, KernelError> {
    let root_hash = parse_hash_ref(root_hash)?;
    let path_segments = validate_path(path)?;
    let entry = resolve_entry(store, &root_hash, &path_segments)?;
    Ok(entry.map(|entry| WorkspaceRefEntry {
        kind: entry.kind,
        hash: entry.hash,
        size: entry.size,
        mode: entry.mode,
    }))
}

pub fn read_bytes<S: Store>(
    store: &S,
    root_hash: &HashRef,
    path: &str,
    range: Option<(u64, u64)>,
) -> Result<Vec<u8>, KernelError> {
    let root_hash = parse_hash_ref(root_hash)?;
    let path_segments = validate_path(path)?;
    let Some(entry) = resolve_entry(store, &root_hash, &path_segments)? else {
        return Err(KernelError::Query(format!("path not found: {path}")));
    };
    if entry.kind != "file" {
        return Err(KernelError::Query(format!("path is not a file: {path}")));
    }
    let blob_hash = parse_hash_ref(&entry.hash)?;
    let mut bytes = store.get_blob(blob_hash)?;
    if let Some((start, end)) = range {
        if start > end {
            return Err(KernelError::Query("invalid range".into()));
        }
        let len = bytes.len() as u64;
        if end > len {
            return Err(KernelError::Query("range exceeds file size".into()));
        }
        bytes = bytes[start as usize..end as usize].to_vec();
    }
    Ok(bytes)
}

pub fn write_bytes<S: Store>(
    store: &S,
    root_hash: &HashRef,
    path: &str,
    bytes: &[u8],
    mode: Option<u64>,
) -> Result<WorkspaceWriteBytesReceipt, KernelError> {
    let root_hash = parse_hash_ref(root_hash)?;
    let path_segments = validate_path(path)?;
    let existing = resolve_entry(store, &root_hash, &path_segments)?;
    if matches!(
        existing.as_ref().map(|entry| entry.kind.as_str()),
        Some("dir")
    ) {
        return Err(KernelError::Query("path is a directory".into()));
    }
    let mode = resolve_file_mode(existing.as_ref(), mode)?;
    let blob_hash = store.put_blob(bytes)?;
    let blob_ref = hash_ref_from_hash(&blob_hash)?;
    let new_root = write_file_at_path(
        store,
        &root_hash,
        &path_segments,
        &blob_ref,
        bytes.len() as u64,
        mode,
    )?;
    Ok(WorkspaceWriteBytesReceipt {
        new_root_hash: hash_ref_from_hash(&new_root)?,
        blob_hash: blob_ref,
    })
}

pub fn write_ref<S: Store>(
    store: &S,
    root_hash: &HashRef,
    path: &str,
    blob_hash: &HashRef,
    mode: Option<u64>,
) -> Result<WorkspaceWriteRefReceipt, KernelError> {
    let root_hash = parse_hash_ref(root_hash)?;
    let blob_hash = parse_hash_ref(blob_hash)?;
    let path_segments = validate_path(path)?;
    let existing = resolve_entry(store, &root_hash, &path_segments)?;
    if matches!(
        existing.as_ref().map(|entry| entry.kind.as_str()),
        Some("dir")
    ) {
        return Err(KernelError::Query("path is a directory".into()));
    }
    if !store.has_blob(blob_hash)? {
        return Err(KernelError::Query(format!(
            "blob not found: {}",
            blob_hash.to_hex()
        )));
    }
    let mode = resolve_file_mode(existing.as_ref(), mode)?;
    let size = store.get_blob(blob_hash)?.len() as u64;
    let blob_ref = hash_ref_from_hash(&blob_hash)?;
    let new_root = write_file_at_path(store, &root_hash, &path_segments, &blob_ref, size, mode)?;
    Ok(WorkspaceWriteRefReceipt {
        new_root_hash: hash_ref_from_hash(&new_root)?,
        blob_hash: blob_ref,
    })
}

pub fn remove<S: Store>(
    store: &S,
    root_hash: &HashRef,
    path: &str,
) -> Result<WorkspaceRemoveReceipt, KernelError> {
    let root_hash = parse_hash_ref(root_hash)?;
    let path_segments = validate_path(path)?;
    if resolve_entry(store, &root_hash, &path_segments)?.is_none() {
        return Err(KernelError::Query(format!("path not found: {path}")));
    }
    let new_root = remove_entry_at_path(store, &root_hash, &path_segments)?;
    Ok(WorkspaceRemoveReceipt {
        new_root_hash: hash_ref_from_hash(&new_root)?,
    })
}

pub fn diff<S: Store>(
    store: &S,
    root_a: &HashRef,
    root_b: &HashRef,
    prefix: Option<&str>,
) -> Result<WorkspaceDiffReceipt, KernelError> {
    let root_a = parse_hash_ref(root_a)?;
    let root_b = parse_hash_ref(root_b)?;
    let prefix_segments = prefix.map(validate_path).transpose()?.unwrap_or_default();
    let prefix = prefix_segments.join("/");
    let mut entries_a = Vec::new();
    let mut entries_b = Vec::new();
    if prefix_segments.is_empty() {
        collect_subtree_entries(store, &root_a, "", &mut entries_a)?;
        collect_subtree_entries(store, &root_b, "", &mut entries_b)?;
    } else {
        let entry_a = resolve_entry(store, &root_a, &prefix_segments)?;
        let entry_b = resolve_entry(store, &root_b, &prefix_segments)?;
        if entry_a.is_none() && entry_b.is_none() {
            return Err(KernelError::Query(format!("path not found: {prefix}")));
        }
        let file_diff = entry_a
            .as_ref()
            .map(|entry| entry.kind == "file")
            .unwrap_or(false)
            || entry_b
                .as_ref()
                .map(|entry| entry.kind == "file")
                .unwrap_or(false);
        if file_diff {
            if let Some(entry) = entry_a {
                entries_a.push((prefix.clone(), entry));
            }
            if let Some(entry) = entry_b {
                entries_b.push((prefix.clone(), entry));
            }
        } else {
            if let Some(entry) = entry_a {
                let root_a_hash = parse_hash_ref(&entry.hash)?;
                collect_subtree_entries(store, &root_a_hash, &prefix, &mut entries_a)?;
            }
            if let Some(entry) = entry_b {
                let root_b_hash = parse_hash_ref(&entry.hash)?;
                collect_subtree_entries(store, &root_b_hash, &prefix, &mut entries_b)?;
            }
        }
    }

    let mut map_a = HashMap::new();
    let mut map_b = HashMap::new();
    for (path, entry) in entries_a {
        map_a.insert(path, entry);
    }
    for (path, entry) in entries_b {
        map_b.insert(path, entry);
    }
    let mut paths: Vec<String> = map_a.keys().chain(map_b.keys()).cloned().collect();
    paths.sort();
    paths.dedup();

    let mut changes = Vec::new();
    for path in paths {
        let old = map_a.get(&path);
        let new = map_b.get(&path);
        if old
            .zip(new)
            .map(|(a, b)| {
                a.kind == b.kind && a.hash == b.hash && a.annotations_hash == b.annotations_hash
            })
            .unwrap_or(false)
        {
            continue;
        }
        let kind = new
            .map(|entry| entry.kind.clone())
            .or_else(|| old.map(|entry| entry.kind.clone()))
            .unwrap_or_else(|| "file".into());
        changes.push(WorkspaceDiffChange {
            path,
            kind,
            old_hash: old.map(|entry| entry.hash.clone()),
            new_hash: new.map(|entry| entry.hash.clone()),
        });
    }
    Ok(WorkspaceDiffReceipt { changes })
}

pub fn annotations_get<S: Store>(
    store: &S,
    root_hash: &HashRef,
    path: Option<&str>,
) -> Result<WorkspaceAnnotationsGetReceipt, KernelError> {
    let root_hash = parse_hash_ref(root_hash)?;
    let path_segments = path.map(validate_path).transpose()?.unwrap_or_default();
    if !path_segments.is_empty() && resolve_entry(store, &root_hash, &path_segments)?.is_none() {
        return Err(KernelError::Query(format!(
            "path not found: {}",
            path.unwrap_or_default()
        )));
    }
    let annotations = if path_segments.is_empty() {
        let tree = load_tree(store, &root_hash)?;
        annotations_from_hash(store, tree.annotations_hash.as_ref())?
    } else {
        let (name, parent) = path_segments
            .split_last()
            .ok_or_else(|| KernelError::Query("path required".into()))?;
        let parent_hash = resolve_dir_hash(store, &root_hash, parent)?;
        let tree = load_tree(store, &parent_hash)?;
        let entry = find_entry(&tree, name)
            .cloned()
            .ok_or_else(|| KernelError::Query("path not found".into()))?;
        if entry.kind == "file" {
            annotations_from_hash(store, entry.annotations_hash.as_ref())?
        } else if entry.kind == "dir" {
            let child_hash = parse_hash_ref(&entry.hash)?;
            let child = load_tree(store, &child_hash)?;
            annotations_from_hash(store, child.annotations_hash.as_ref())?
        } else {
            return Err(KernelError::Query("invalid entry kind".into()));
        }
    };
    Ok(WorkspaceAnnotationsGetReceipt { annotations })
}

pub fn annotations_set<S: Store>(
    store: &S,
    root_hash: &HashRef,
    path: Option<&str>,
    patch: &WorkspaceAnnotationsPatch,
) -> Result<WorkspaceAnnotationsSetReceipt, KernelError> {
    let root_hash = parse_hash_ref(root_hash)?;
    let path_segments = path.map(validate_path).transpose()?.unwrap_or_default();
    if !path_segments.is_empty() && resolve_entry(store, &root_hash, &path_segments)?.is_none() {
        return Err(KernelError::Query(format!(
            "path not found: {}",
            path.unwrap_or_default()
        )));
    }
    let (new_root, annotations_hash) =
        set_annotations_at_path(store, &root_hash, &path_segments, patch)?;
    Ok(WorkspaceAnnotationsSetReceipt {
        new_root_hash: hash_ref_from_hash(&new_root)?,
        annotations_hash,
    })
}

fn parse_hash_ref(hash: &HashRef) -> Result<Hash, KernelError> {
    Hash::from_hex_str(hash.as_str())
        .map_err(|err| KernelError::Query(format!("invalid hash ref '{}': {err}", hash.as_str())))
}

fn hash_ref_from_hash(hash: &Hash) -> Result<HashRef, KernelError> {
    HashRef::new(hash.to_hex()).map_err(|err| KernelError::Query(err.to_string()))
}

fn validate_path(path: &str) -> Result<Vec<String>, KernelError> {
    if path.is_empty() {
        return Err(KernelError::Query("path required".into()));
    }
    if path.starts_with('/') || path.ends_with('/') {
        return Err(KernelError::Query("invalid path".into()));
    }
    let mut segments = Vec::new();
    for segment in path.split('/') {
        validate_path_segment(segment)?;
        segments.push(segment.to_string());
    }
    Ok(segments)
}

fn validate_path_segment(segment: &str) -> Result<(), KernelError> {
    if segment.is_empty() || segment == "." || segment == ".." {
        return Err(KernelError::Query("invalid path segment".into()));
    }
    if !segment.chars().all(is_url_safe_char) {
        return Err(KernelError::Query("invalid path segment".into()));
    }
    Ok(())
}

fn is_url_safe_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '~' | '-')
}

fn validate_tree(tree: &WorkspaceTree) -> Result<(), KernelError> {
    let mut prev: Option<&str> = None;
    for entry in &tree.entries {
        validate_path_segment(&entry.name)?;
        if let Some(prev) = prev
            && entry.name.as_str() <= prev
        {
            return Err(KernelError::Query("workspace tree not sorted".into()));
        }
        prev = Some(entry.name.as_str());
        match entry.kind.as_str() {
            "file" => {
                if entry.mode != MODE_FILE_DEFAULT && entry.mode != MODE_FILE_EXEC {
                    return Err(KernelError::Query("invalid file mode".into()));
                }
            }
            "dir" => {
                if entry.mode != MODE_DIR || entry.size != 0 {
                    return Err(KernelError::Query("invalid dir entry".into()));
                }
            }
            _ => return Err(KernelError::Query("invalid entry kind".into())),
        }
    }
    Ok(())
}

fn load_tree<S: Store>(store: &S, hash: &Hash) -> Result<WorkspaceTree, KernelError> {
    let tree: WorkspaceTree = store.get_node(*hash)?;
    validate_tree(&tree)?;
    Ok(tree)
}

fn find_entry<'a>(tree: &'a WorkspaceTree, name: &str) -> Option<&'a WorkspaceEntry> {
    tree.entries
        .binary_search_by(|entry| entry.name.as_str().cmp(name))
        .ok()
        .map(|idx| &tree.entries[idx])
}

fn upsert_entry(entries: &mut Vec<WorkspaceEntry>, entry: WorkspaceEntry) {
    match entries.binary_search_by(|candidate| candidate.name.as_str().cmp(entry.name.as_str())) {
        Ok(idx) => entries[idx] = entry,
        Err(idx) => entries.insert(idx, entry),
    }
}

fn remove_entry(
    entries: &mut Vec<WorkspaceEntry>,
    name: &str,
) -> Result<WorkspaceEntry, KernelError> {
    match entries.binary_search_by(|entry| entry.name.as_str().cmp(name)) {
        Ok(idx) => Ok(entries.remove(idx)),
        Err(_) => Err(KernelError::Query("path not found".into())),
    }
}

fn resolve_dir_hash<S: Store>(
    store: &S,
    root_hash: &Hash,
    path: &[String],
) -> Result<Hash, KernelError> {
    let mut current = *root_hash;
    for segment in path {
        let tree = load_tree(store, &current)?;
        let entry = find_entry(&tree, segment)
            .ok_or_else(|| KernelError::Query(format!("missing path segment '{segment}'")))?;
        if entry.kind != "dir" {
            return Err(KernelError::Query("path is not a directory".into()));
        }
        current = parse_hash_ref(&entry.hash)?;
    }
    Ok(current)
}

fn resolve_entry<S: Store>(
    store: &S,
    root_hash: &Hash,
    path: &[String],
) -> Result<Option<WorkspaceEntry>, KernelError> {
    if path.is_empty() {
        return Err(KernelError::Query("path required".into()));
    }
    let mut current = *root_hash;
    for (idx, segment) in path.iter().enumerate() {
        let tree = load_tree(store, &current)?;
        let Some(entry) = find_entry(&tree, segment) else {
            return Ok(None);
        };
        if idx == path.len() - 1 {
            return Ok(Some(entry.clone()));
        }
        if entry.kind != "dir" {
            return Err(KernelError::Query("path is not a directory".into()));
        }
        current = parse_hash_ref(&entry.hash)?;
    }
    Ok(None)
}

fn join_path(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else {
        format!("{base}/{name}")
    }
}

fn list_entry_from_tree(path: String, entry: &WorkspaceEntry) -> WorkspaceListEntry {
    WorkspaceListEntry {
        path,
        kind: entry.kind.clone(),
        hash: Some(entry.hash.clone()),
        size: Some(entry.size),
        mode: Some(entry.mode),
    }
}

fn collect_subtree_entries<S: Store>(
    store: &S,
    tree_hash: &Hash,
    base: &str,
    out: &mut Vec<(String, WorkspaceEntry)>,
) -> Result<(), KernelError> {
    let tree = load_tree(store, tree_hash)?;
    for entry in &tree.entries {
        let path = join_path(base, &entry.name);
        out.push((path.clone(), entry.clone()));
        if entry.kind == "dir" {
            let child_hash = parse_hash_ref(&entry.hash)?;
            collect_subtree_entries(store, &child_hash, &path, out)?;
        }
    }
    Ok(())
}

fn resolve_file_mode(
    existing: Option<&WorkspaceEntry>,
    requested: Option<u64>,
) -> Result<u64, KernelError> {
    if let Some(mode) = requested {
        return normalize_file_mode(mode);
    }
    if let Some(entry) = existing
        && entry.kind == "file"
    {
        return normalize_file_mode(entry.mode);
    }
    Ok(MODE_FILE_DEFAULT)
}

fn normalize_file_mode(mode: u64) -> Result<u64, KernelError> {
    match mode {
        0o644 | MODE_FILE_DEFAULT => Ok(MODE_FILE_DEFAULT),
        0o755 | MODE_FILE_EXEC => Ok(MODE_FILE_EXEC),
        _ => Err(KernelError::Query(
            "invalid file mode: expected 0o644/0o755 or 0o100644/0o100755".into(),
        )),
    }
}

fn annotations_from_hash<S: Store>(
    store: &S,
    hash: Option<&HashRef>,
) -> Result<Option<WorkspaceAnnotations>, KernelError> {
    let Some(hash) = hash else {
        return Ok(None);
    };
    let hash = parse_hash_ref(hash)?;
    let annotations: WorkspaceAnnotations = store.get_node(hash)?;
    Ok(Some(annotations))
}

fn apply_annotations_patch<S: Store>(
    store: &S,
    current: Option<&HashRef>,
    patch: &WorkspaceAnnotationsPatch,
) -> Result<HashRef, KernelError> {
    let mut annotations = match current {
        Some(hash) => annotations_from_hash(store, Some(hash))?.unwrap_or_default(),
        None => BTreeMap::new(),
    };
    for (key, value) in patch {
        match value {
            Some(hash) => {
                annotations.insert(key.clone(), hash.clone());
            }
            None => {
                annotations.remove(key);
            }
        }
    }
    let new_hash = store.put_node(&annotations)?;
    hash_ref_from_hash(&new_hash)
}

fn set_annotations_at_path<S: Store>(
    store: &S,
    tree_hash: &Hash,
    path: &[String],
    patch: &WorkspaceAnnotationsPatch,
) -> Result<(Hash, HashRef), KernelError> {
    let mut tree = load_tree(store, tree_hash)?;
    if path.is_empty() {
        let annotations_hash =
            apply_annotations_patch(store, tree.annotations_hash.as_ref(), patch)?;
        tree.annotations_hash = Some(annotations_hash.clone());
        let new_root = store.put_node(&tree)?;
        return Ok((new_root, annotations_hash));
    }
    if path.len() == 1 {
        let name = &path[0];
        let entry = find_entry(&tree, name)
            .cloned()
            .ok_or_else(|| KernelError::Query("path not found".into()))?;
        if entry.kind == "file" {
            let annotations_hash =
                apply_annotations_patch(store, entry.annotations_hash.as_ref(), patch)?;
            let updated = WorkspaceEntry {
                annotations_hash: Some(annotations_hash.clone()),
                ..entry
            };
            upsert_entry(&mut tree.entries, updated);
            let new_root = store.put_node(&tree)?;
            return Ok((new_root, annotations_hash));
        }
        if entry.kind == "dir" {
            let child_hash = parse_hash_ref(&entry.hash)?;
            let (new_child, annotations_hash) =
                set_annotations_at_path(store, &child_hash, &[], patch)?;
            let updated = WorkspaceEntry {
                hash: hash_ref_from_hash(&new_child)?,
                annotations_hash: entry.annotations_hash.clone(),
                ..entry
            };
            upsert_entry(&mut tree.entries, updated);
            let new_root = store.put_node(&tree)?;
            return Ok((new_root, annotations_hash));
        }
        return Err(KernelError::Query("invalid entry kind".into()));
    }
    let dir_name = &path[0];
    let entry = find_entry(&tree, dir_name)
        .cloned()
        .ok_or_else(|| KernelError::Query("path not found".into()))?;
    if entry.kind != "dir" {
        return Err(KernelError::Query("path is not a directory".into()));
    }
    let child_hash = parse_hash_ref(&entry.hash)?;
    let (new_child, annotations_hash) =
        set_annotations_at_path(store, &child_hash, &path[1..], patch)?;
    let updated = WorkspaceEntry {
        hash: hash_ref_from_hash(&new_child)?,
        annotations_hash: entry.annotations_hash.clone(),
        ..entry
    };
    upsert_entry(&mut tree.entries, updated);
    let new_root = store.put_node(&tree)?;
    Ok((new_root, annotations_hash))
}

fn write_file_at_path<S: Store>(
    store: &S,
    tree_hash: &Hash,
    path: &[String],
    blob_hash: &HashRef,
    size: u64,
    mode: u64,
) -> Result<Hash, KernelError> {
    let mut tree = load_tree(store, tree_hash)?;
    if path.len() == 1 {
        let name = path[0].clone();
        let existing = find_entry(&tree, &name).cloned();
        let entry = WorkspaceEntry {
            name,
            kind: "file".into(),
            hash: blob_hash.clone(),
            size,
            mode,
            annotations_hash: existing.and_then(|entry| {
                if entry.kind == "file" {
                    entry.annotations_hash
                } else {
                    None
                }
            }),
        };
        upsert_entry(&mut tree.entries, entry);
        return Ok(store.put_node(&tree)?);
    }
    let dir_name = &path[0];
    let existing = find_entry(&tree, dir_name).cloned();
    let child_hash = if let Some(entry) = &existing {
        if entry.kind != "dir" {
            return Err(KernelError::Query("path is not a directory".into()));
        }
        parse_hash_ref(&entry.hash)?
    } else {
        store.put_node(&WorkspaceTree {
            entries: Vec::new(),
            annotations_hash: None,
        })?
    };
    let new_child = write_file_at_path(store, &child_hash, &path[1..], blob_hash, size, mode)?;
    let entry = WorkspaceEntry {
        name: dir_name.clone(),
        kind: "dir".into(),
        hash: hash_ref_from_hash(&new_child)?,
        size: 0,
        mode: MODE_DIR,
        annotations_hash: existing.and_then(|entry| entry.annotations_hash),
    };
    upsert_entry(&mut tree.entries, entry);
    Ok(store.put_node(&tree)?)
}

fn remove_entry_at_path<S: Store>(
    store: &S,
    tree_hash: &Hash,
    path: &[String],
) -> Result<Hash, KernelError> {
    let mut tree = load_tree(store, tree_hash)?;
    if path.len() == 1 {
        let entry = remove_entry(&mut tree.entries, &path[0])?;
        if entry.kind == "dir" {
            let child_hash = parse_hash_ref(&entry.hash)?;
            let child = load_tree(store, &child_hash)?;
            if !child.entries.is_empty() {
                return Err(KernelError::Query("directory not empty".into()));
            }
        }
        return Ok(store.put_node(&tree)?);
    }
    let dir_name = &path[0];
    let entry = find_entry(&tree, dir_name)
        .cloned()
        .ok_or_else(|| KernelError::Query("path not found".into()))?;
    if entry.kind != "dir" {
        return Err(KernelError::Query("path is not a directory".into()));
    }
    let child_hash = parse_hash_ref(&entry.hash)?;
    let new_child = remove_entry_at_path(store, &child_hash, &path[1..])?;
    let updated = WorkspaceEntry {
        name: dir_name.clone(),
        kind: "dir".into(),
        hash: hash_ref_from_hash(&new_child)?,
        size: 0,
        mode: MODE_DIR,
        annotations_hash: entry.annotations_hash.clone(),
    };
    upsert_entry(&mut tree.entries, updated);
    Ok(store.put_node(&tree)?)
}
