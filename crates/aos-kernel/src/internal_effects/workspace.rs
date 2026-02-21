use aos_air_types::HashRef;
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effects::EffectIntent;
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use std::collections::{BTreeMap, HashMap};

use crate::query::{Consistency, StateReader};
use crate::{Kernel, KernelError};

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceCommitMeta {
    root_hash: HashRef,
    owner: String,
    created_at: u64,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct WorkspaceHistory {
    latest: u64,
    versions: BTreeMap<u64, WorkspaceCommitMeta>,
}

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

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceTree {
    entries: Vec<WorkspaceEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    annotations_hash: Option<HashRef>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceResolveParams {
    workspace: String,
    #[serde(default)]
    version: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceResolveReceipt {
    exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    head: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_hash: Option<HashRef>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceEmptyRootParams {
    workspace: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceEmptyRootReceipt {
    root_hash: HashRef,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceListParams {
    root_hash: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceListEntry {
    path: String,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hash: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceListReceipt {
    entries: Vec<WorkspaceListEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    hash: HashRef,
    size: u64,
    mode: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceReadBytesParams {
    root_hash: String,
    path: String,
    #[serde(default)]
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
    #[serde(default)]
    mode: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceWriteBytesReceipt {
    new_root_hash: HashRef,
    blob_hash: HashRef,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceRemoveParams {
    root_hash: String,
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceRemoveReceipt {
    new_root_hash: HashRef,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceDiffParams {
    root_a: String,
    root_b: String,
    #[serde(default)]
    prefix: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceDiffChange {
    path: String,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    old_hash: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    new_hash: Option<HashRef>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceDiffReceipt {
    changes: Vec<WorkspaceDiffChange>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(transparent)]
struct WorkspaceAnnotations(BTreeMap<String, HashRef>);

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(transparent)]
struct WorkspaceAnnotationsPatch(BTreeMap<String, Option<HashRef>>);

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsGetParams {
    root_hash: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsGetReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    annotations: Option<WorkspaceAnnotations>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsSetParams {
    root_hash: String,
    #[serde(default)]
    path: Option<String>,
    annotations_patch: WorkspaceAnnotationsPatch,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceAnnotationsSetReceipt {
    new_root_hash: HashRef,
    annotations_hash: HashRef,
}

const MODE_FILE_DEFAULT: u64 = 0o644;
const MODE_FILE_EXEC: u64 = 0o755;
const MODE_DIR: u64 = 0o755;

fn hash_ref_from_hash(hash: &Hash) -> Result<HashRef, KernelError> {
    HashRef::new(hash.to_hex()).map_err(|e| KernelError::Manifest(format!("invalid hash: {e}")))
}

fn parse_hash_str(value: &str) -> Result<Hash, KernelError> {
    let hash_ref = HashRef::new(value.to_string())
        .map_err(|e| KernelError::Query(format!("invalid hash: {e}")))?;
    Hash::from_hex_str(hash_ref.as_str())
        .map_err(|e| KernelError::Query(format!("invalid hash: {e}")))
}

fn hash_from_ref(hash: &HashRef) -> Result<Hash, KernelError> {
    Hash::from_hex_str(hash.as_str()).map_err(|e| KernelError::Query(format!("invalid hash: {e}")))
}

fn validate_workspace_name(name: &str) -> Result<(), KernelError> {
    if name.is_empty() || name.contains('/') {
        return Err(KernelError::Query("invalid workspace name".into()));
    }
    if !name.chars().all(is_url_safe_char) {
        return Err(KernelError::Query("invalid workspace name".into()));
    }
    Ok(())
}

fn validate_path(path: &str) -> Result<Vec<String>, KernelError> {
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') {
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

fn load_tree<S: aos_store::Store>(store: &S, hash: &Hash) -> Result<WorkspaceTree, KernelError> {
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
    match entries.binary_search_by(|e| e.name.as_str().cmp(entry.name.as_str())) {
        Ok(idx) => entries[idx] = entry,
        Err(idx) => entries.insert(idx, entry),
    }
}

fn remove_entry(
    entries: &mut Vec<WorkspaceEntry>,
    name: &str,
) -> Result<WorkspaceEntry, KernelError> {
    match entries.binary_search_by(|e| e.name.as_str().cmp(name)) {
        Ok(idx) => Ok(entries.remove(idx)),
        Err(_) => Err(KernelError::Query("path not found".into())),
    }
}

fn resolve_dir_hash<S: aos_store::Store>(
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
        current = hash_from_ref(&entry.hash)?;
    }
    Ok(current)
}

fn resolve_entry<S: aos_store::Store>(
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
        current = hash_from_ref(&entry.hash)?;
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

fn collect_subtree_entries<S: aos_store::Store>(
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
            let child_hash = hash_from_ref(&entry.hash)?;
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
        if mode != MODE_FILE_DEFAULT && mode != MODE_FILE_EXEC {
            return Err(KernelError::Query("invalid file mode".into()));
        }
        return Ok(mode);
    }
    if let Some(entry) = existing
        && entry.kind == "file"
    {
        return Ok(entry.mode);
    }
    Ok(MODE_FILE_DEFAULT)
}

fn annotations_from_hash<S: aos_store::Store>(
    store: &S,
    hash: Option<&HashRef>,
) -> Result<Option<WorkspaceAnnotations>, KernelError> {
    let Some(hash) = hash else {
        return Ok(None);
    };
    let hash = hash_from_ref(hash)?;
    let annotations: WorkspaceAnnotations = store.get_node(hash)?;
    Ok(Some(annotations))
}

fn apply_annotations_patch<S: aos_store::Store>(
    store: &S,
    current: Option<&HashRef>,
    patch: &WorkspaceAnnotationsPatch,
) -> Result<HashRef, KernelError> {
    let mut annotations = match current {
        Some(hash) => {
            annotations_from_hash(store, Some(hash))?
                .unwrap_or_default()
                .0
        }
        None => BTreeMap::new(),
    };
    for (key, value) in &patch.0 {
        match value {
            Some(hash) => {
                annotations.insert(key.clone(), hash.clone());
            }
            None => {
                annotations.remove(key);
            }
        }
    }
    let annotations = WorkspaceAnnotations(annotations);
    let new_hash = store.put_node(&annotations)?;
    hash_ref_from_hash(&new_hash)
}

fn set_annotations_at_path<S: aos_store::Store>(
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
            let child_hash = hash_from_ref(&entry.hash)?;
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
    let child_hash = hash_from_ref(&entry.hash)?;
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

fn write_file_at_path<S: aos_store::Store>(
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
        hash_from_ref(&entry.hash)?
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

fn remove_entry_at_path<S: aos_store::Store>(
    store: &S,
    tree_hash: &Hash,
    path: &[String],
) -> Result<Hash, KernelError> {
    let mut tree = load_tree(store, tree_hash)?;
    if path.len() == 1 {
        let entry = remove_entry(&mut tree.entries, &path[0])?;
        if entry.kind == "dir" {
            let child_hash = hash_from_ref(&entry.hash)?;
            let child = load_tree(store, &child_hash)?;
            if !child.entries.is_empty() {
                return Err(KernelError::Query("directory not empty".into()));
            }
        }
        return Ok(store.put_node(&tree)?);
    }
    let dir_name = &path[0];
    let entry =
        find_entry(&tree, dir_name).ok_or_else(|| KernelError::Query("path not found".into()))?;
    if entry.kind != "dir" {
        return Err(KernelError::Query("path is not a directory".into()));
    }
    let child_hash = hash_from_ref(&entry.hash)?;
    let new_child = remove_entry_at_path(store, &child_hash, &path[1..])?;
    let entry = WorkspaceEntry {
        name: dir_name.clone(),
        kind: "dir".into(),
        hash: hash_ref_from_hash(&new_child)?,
        size: 0,
        mode: MODE_DIR,
        annotations_hash: entry.annotations_hash.clone(),
    };
    upsert_entry(&mut tree.entries, entry);
    Ok(store.put_node(&tree)?)
}

impl<S> Kernel<S>
where
    S: aos_store::Store + 'static,
{
    pub(super) fn handle_workspace_resolve(
        &self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: WorkspaceResolveParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        validate_workspace_name(&params.workspace)?;
        let key_bytes = self.canonical_key_bytes("sys/WorkspaceName@1", &params.workspace)?;
        let state_read =
            self.get_reducer_state("sys/Workspace@1", Some(&key_bytes), Consistency::Head)?;
        let Some(state_bytes) = state_read.value else {
            let receipt = WorkspaceResolveReceipt {
                exists: false,
                resolved_version: None,
                head: None,
                root_hash: None,
            };
            return Ok(
                to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?
            );
        };
        let history: WorkspaceHistory = serde_cbor::from_slice(&state_bytes)
            .map_err(|e| KernelError::Query(format!("decode workspace history: {e}")))?;
        let head = history.latest;
        let target = params.version.unwrap_or(head);
        let Some(meta) = history.versions.get(&target) else {
            let receipt = WorkspaceResolveReceipt {
                exists: false,
                resolved_version: None,
                head: None,
                root_hash: None,
            };
            return Ok(
                to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?
            );
        };
        let receipt = WorkspaceResolveReceipt {
            exists: true,
            resolved_version: Some(target),
            head: Some(head),
            root_hash: Some(meta.root_hash.clone()),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_workspace_empty_root(
        &self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: WorkspaceEmptyRootParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        validate_workspace_name(&params.workspace)?;
        let store = self.store();
        let hash = store.put_node(&WorkspaceTree {
            entries: Vec::new(),
            annotations_hash: None,
        })?;
        let receipt = WorkspaceEmptyRootReceipt {
            root_hash: hash_ref_from_hash(&hash)?,
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_workspace_list(
        &self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: WorkspaceListParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let root_hash = parse_hash_str(&params.root_hash)?;
        let path_segments = match params.path.as_deref() {
            Some(path) => validate_path(path)?,
            None => Vec::new(),
        };
        let scope = params.scope.as_deref().unwrap_or("dir");
        let store = self.store();
        let dir_hash = resolve_dir_hash(store.as_ref(), &root_hash, &path_segments)?;
        let base = path_segments.join("/");
        let mut entries = Vec::new();
        match scope {
            "dir" => {
                let tree = load_tree(store.as_ref(), &dir_hash)?;
                for entry in tree.entries {
                    let path = join_path(&base, &entry.name);
                    entries.push(list_entry_from_tree(path, &entry));
                }
            }
            "subtree" => {
                let mut tree_entries = Vec::new();
                collect_subtree_entries(store.as_ref(), &dir_hash, &base, &mut tree_entries)?;
                for (path, entry) in tree_entries {
                    entries.push(list_entry_from_tree(path, &entry));
                }
            }
            _ => return Err(KernelError::Query(format!("invalid scope '{}'", scope))),
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let start = params
            .cursor
            .as_deref()
            .map(|cursor| {
                entries
                    .iter()
                    .position(|e| e.path.as_str() > cursor)
                    .unwrap_or(entries.len())
            })
            .unwrap_or(0);
        let limit = params.limit as usize;
        let end = if limit == 0 {
            entries.len()
        } else {
            (start + limit).min(entries.len())
        };
        let next_cursor = if end < entries.len() && end > start {
            Some(entries[end - 1].path.clone())
        } else {
            None
        };
        let sliced = if start < end {
            entries[start..end].to_vec()
        } else {
            Vec::new()
        };
        let receipt = WorkspaceListReceipt {
            entries: sliced,
            next_cursor,
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_workspace_read_ref(
        &self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: WorkspaceReadRefParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let root_hash = parse_hash_str(&params.root_hash)?;
        let path_segments = validate_path(&params.path)?;
        let store = self.store();
        let entry = resolve_entry(store.as_ref(), &root_hash, &path_segments)?;
        let receipt = entry.map(|entry| WorkspaceRefEntry {
            kind: entry.kind,
            hash: entry.hash,
            size: entry.size,
            mode: entry.mode,
        });
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_workspace_read_bytes(
        &self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: WorkspaceReadBytesParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let root_hash = parse_hash_str(&params.root_hash)?;
        let path_segments = validate_path(&params.path)?;
        let store = self.store();
        let Some(entry) = resolve_entry(store.as_ref(), &root_hash, &path_segments)? else {
            return Err(KernelError::Query(format!(
                "path not found: {}",
                params.path
            )));
        };
        if entry.kind != "file" {
            return Err(KernelError::Query(format!(
                "path is not a file: {}",
                params.path
            )));
        }
        let blob_hash = hash_from_ref(&entry.hash)?;
        let mut bytes = store.get_blob(blob_hash)?;
        if let Some(range) = params.range {
            if range.start > range.end {
                return Err(KernelError::Query("invalid range".into()));
            }
            let len = bytes.len() as u64;
            if range.end > len {
                return Err(KernelError::Query("range exceeds file size".into()));
            }
            let start = range.start as usize;
            let end = range.end as usize;
            bytes = bytes[start..end].to_vec();
        }
        Ok(to_canonical_cbor(&ByteBuf::from(bytes))
            .map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_workspace_write_bytes(
        &self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: WorkspaceWriteBytesParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let root_hash = parse_hash_str(&params.root_hash)?;
        let path_segments = validate_path(&params.path)?;
        let store = self.store();
        let existing = resolve_entry(store.as_ref(), &root_hash, &path_segments)?;
        if matches!(existing.as_ref().map(|e| e.kind.as_str()), Some("dir")) {
            return Err(KernelError::Query("path is a directory".into()));
        }
        let mode = resolve_file_mode(existing.as_ref(), params.mode)?;
        let blob_hash = store.put_blob(&params.bytes)?;
        let blob_ref = hash_ref_from_hash(&blob_hash)?;
        let new_root = write_file_at_path(
            store.as_ref(),
            &root_hash,
            &path_segments,
            &blob_ref,
            params.bytes.len() as u64,
            mode,
        )?;
        let receipt = WorkspaceWriteBytesReceipt {
            new_root_hash: hash_ref_from_hash(&new_root)?,
            blob_hash: blob_ref,
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_workspace_remove(
        &self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: WorkspaceRemoveParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let root_hash = parse_hash_str(&params.root_hash)?;
        let path_segments = validate_path(&params.path)?;
        let store = self.store();
        if resolve_entry(store.as_ref(), &root_hash, &path_segments)?.is_none() {
            return Err(KernelError::Query(format!(
                "path not found: {}",
                params.path
            )));
        }
        let new_root = remove_entry_at_path(store.as_ref(), &root_hash, &path_segments)?;
        let receipt = WorkspaceRemoveReceipt {
            new_root_hash: hash_ref_from_hash(&new_root)?,
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_workspace_diff(
        &self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: WorkspaceDiffParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let root_a = parse_hash_str(&params.root_a)?;
        let root_b = parse_hash_str(&params.root_b)?;
        let prefix_segments = match params.prefix.as_deref() {
            Some(path) => validate_path(path)?,
            None => Vec::new(),
        };
        let prefix = prefix_segments.join("/");
        let store = self.store();
        let mut entries_a = Vec::new();
        let mut entries_b = Vec::new();
        if prefix_segments.is_empty() {
            collect_subtree_entries(store.as_ref(), &root_a, "", &mut entries_a)?;
            collect_subtree_entries(store.as_ref(), &root_b, "", &mut entries_b)?;
        } else {
            let entry_a = resolve_entry(store.as_ref(), &root_a, &prefix_segments)?;
            let entry_b = resolve_entry(store.as_ref(), &root_b, &prefix_segments)?;
            if entry_a.is_none() && entry_b.is_none() {
                return Err(KernelError::Query(format!("path not found: {}", prefix)));
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
                    let root_a_hash = hash_from_ref(&entry.hash)?;
                    collect_subtree_entries(store.as_ref(), &root_a_hash, &prefix, &mut entries_a)?;
                }
                if let Some(entry) = entry_b {
                    let root_b_hash = hash_from_ref(&entry.hash)?;
                    collect_subtree_entries(store.as_ref(), &root_b_hash, &prefix, &mut entries_b)?;
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
                .map(|e| e.kind.clone())
                .or_else(|| old.map(|e| e.kind.clone()))
                .unwrap_or_else(|| "file".into());
            changes.push(WorkspaceDiffChange {
                path,
                kind,
                old_hash: old.map(|e| e.hash.clone()),
                new_hash: new.map(|e| e.hash.clone()),
            });
        }
        let receipt = WorkspaceDiffReceipt { changes };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_workspace_annotations_get(
        &self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: WorkspaceAnnotationsGetParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let root_hash = parse_hash_str(&params.root_hash)?;
        let path_segments = match params.path.as_deref() {
            Some(path) => validate_path(path)?,
            None => Vec::new(),
        };
        let store = self.store();
        if !path_segments.is_empty()
            && resolve_entry(store.as_ref(), &root_hash, &path_segments)?.is_none()
        {
            return Err(KernelError::Query(format!(
                "path not found: {}",
                params.path.as_deref().unwrap_or_default()
            )));
        }
        let annotations = if path_segments.is_empty() {
            let tree = load_tree(store.as_ref(), &root_hash)?;
            annotations_from_hash(store.as_ref(), tree.annotations_hash.as_ref())?
        } else {
            let (name, parent) = path_segments
                .split_last()
                .ok_or_else(|| KernelError::Query("path required".into()))?;
            let parent_hash = resolve_dir_hash(store.as_ref(), &root_hash, parent)?;
            let tree = load_tree(store.as_ref(), &parent_hash)?;
            let entry = find_entry(&tree, name)
                .cloned()
                .ok_or_else(|| KernelError::Query("path not found".into()))?;
            if entry.kind == "file" {
                annotations_from_hash(store.as_ref(), entry.annotations_hash.as_ref())?
            } else if entry.kind == "dir" {
                let child_hash = hash_from_ref(&entry.hash)?;
                let child = load_tree(store.as_ref(), &child_hash)?;
                annotations_from_hash(store.as_ref(), child.annotations_hash.as_ref())?
            } else {
                return Err(KernelError::Query("invalid entry kind".into()));
            }
        };
        let receipt = WorkspaceAnnotationsGetReceipt { annotations };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_workspace_annotations_set(
        &self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: WorkspaceAnnotationsSetParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let root_hash = parse_hash_str(&params.root_hash)?;
        let path_segments = match params.path.as_deref() {
            Some(path) => validate_path(path)?,
            None => Vec::new(),
        };
        let store = self.store();
        if !path_segments.is_empty()
            && resolve_entry(store.as_ref(), &root_hash, &path_segments)?.is_none()
        {
            return Err(KernelError::Query(format!(
                "path not found: {}",
                params.path.as_deref().unwrap_or_default()
            )));
        }
        let (new_root, annotations_hash) = set_annotations_at_path(
            store.as_ref(),
            &root_hash,
            &path_segments,
            &params.annotations_patch,
        )?;
        let receipt = WorkspaceAnnotationsSetReceipt {
            new_root_hash: hash_ref_from_hash(&new_root)?,
            annotations_hash,
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_store::{MemStore, Store};

    #[test]
    fn validate_path_rejects_invalid() {
        assert_eq!(
            validate_path("src/lib.rs").unwrap(),
            vec!["src".to_string(), "lib.rs".to_string()]
        );
        for bad in ["", "/root", "root/", "root//file", "root/..", "root/./file"] {
            assert!(validate_path(bad).is_err(), "expected error for '{bad}'");
        }
    }

    #[test]
    fn tree_write_and_remove_roundtrip() {
        let store = MemStore::new();
        let root = store
            .put_node(&WorkspaceTree {
                entries: Vec::new(),
                annotations_hash: None,
            })
            .expect("root tree");
        let content = b"hello".to_vec();
        let blob_hash = store.put_blob(&content).expect("put blob");
        let blob_ref = hash_ref_from_hash(&blob_hash).expect("blob ref");
        let new_root = write_file_at_path(
            &store,
            &root,
            &vec!["dir".into(), "file.txt".into()],
            &blob_ref,
            content.len() as u64,
            MODE_FILE_DEFAULT,
        )
        .expect("write file");
        let entry = resolve_entry(&store, &new_root, &vec!["dir".into(), "file.txt".into()])
            .expect("resolve")
            .expect("entry");
        assert_eq!(entry.kind, "file");
        assert_eq!(entry.size, content.len() as u64);
        let err = remove_entry_at_path(&store, &new_root, &vec!["dir".into()]).unwrap_err();
        assert!(matches!(err, KernelError::Query(_)));
        let root_no_file =
            remove_entry_at_path(&store, &new_root, &vec!["dir".into(), "file.txt".into()])
                .expect("remove file");
        let missing = resolve_entry(
            &store,
            &root_no_file,
            &vec!["dir".into(), "file.txt".into()],
        )
        .expect("resolve");
        assert!(missing.is_none());
        let root_no_dir =
            remove_entry_at_path(&store, &root_no_file, &vec!["dir".into()]).expect("remove dir");
        let tree = load_tree(&store, &root_no_dir).expect("load tree");
        assert!(tree.entries.is_empty());
    }
}
