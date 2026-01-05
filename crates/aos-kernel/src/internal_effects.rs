use aos_air_types::HashRef;
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

use crate::cell_index::CellMeta;
use crate::governance_effects::{
    GovApplyParams, GovApplyReceipt, GovApprovalDecision, GovApproveParams, GovApproveReceipt,
    GovPatchInput, GovProposeParams, GovProposeReceipt, GovShadowParams, GovShadowReceipt,
    GovPredictedEffect, GovPendingReceipt, GovPlanResultPreview, GovLedgerDelta, GovLedgerKind,
    GovDeltaKind,
};
use crate::query::{Consistency, ReadMeta, StateReader};
use crate::{Kernel, KernelError};

const INTROSPECT_ADAPTER_ID: &str = "kernel.introspect";
const GOVERNANCE_ADAPTER_ID: &str = "kernel.governance";

/// Kinds handled entirely inside the kernel (no host adapter).
pub(crate) static INTERNAL_EFFECT_KINDS: &[&str] = &[
    "introspect.manifest",
    "introspect.reducer_state",
    "introspect.journal_head",
    "introspect.list_cells",
    "workspace.resolve",
    "workspace.empty_root",
    "workspace.list",
    "workspace.read_ref",
    "workspace.read_bytes",
    "workspace.write_bytes",
    "workspace.remove",
    "workspace.diff",
    "workspace.annotations_get",
    "workspace.annotations_set",
    "governance.propose",
    "governance.shadow",
    "governance.approve",
    "governance.apply",
];

#[derive(Debug, Serialize, Deserialize)]
struct ManifestParams {
    consistency: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestReceipt {
    #[serde(with = "serde_bytes")]
    manifest: Vec<u8>,
    meta: MetaSer,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReducerStateParams {
    reducer: String,
    #[serde(default)]
    key: Option<Vec<u8>>, // bytes
    consistency: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReducerStateReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state: Option<Vec<u8>>,
    meta: MetaSer,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListCellsParams {
    reducer: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListCellsReceipt {
    cells: Vec<CellEntry>,
    meta: MetaSer,
}

#[derive(Debug, Serialize, Deserialize)]
struct CellEntry {
    #[serde(with = "serde_bytes")]
    key: Vec<u8>,
    state_hash: [u8; 32],
    size: u64,
    last_active_ns: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct JournalHeadReceipt {
    meta: MetaSer,
}

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
struct WorkspaceAnnotations(BTreeMap<HashRef, HashRef>);

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(transparent)]
struct WorkspaceAnnotationsPatch(BTreeMap<HashRef, Option<HashRef>>);

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

#[derive(Debug, Serialize, Deserialize)]
struct MetaSer {
    journal_height: u64,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "serde_bytes")]
    snapshot_hash: Option<Vec<u8>>,
    #[serde(with = "serde_bytes")]
    manifest_hash: Vec<u8>,
}

/// Map textual consistency param to enum.
fn parse_consistency(value: &str) -> Result<Consistency, KernelError> {
    let v = value.trim().to_lowercase();
    if v == "head" {
        return Ok(Consistency::Head);
    }
    if let Some(rest) = v.strip_prefix("exact:") {
        let h = rest
            .parse::<u64>()
            .map_err(|e| KernelError::Query(format!("invalid exact height '{rest}': {e}")))?;
        return Ok(Consistency::Exact(h));
    }
    if let Some(rest) = v.strip_prefix("at_least:") {
        let h = rest
            .parse::<u64>()
            .map_err(|e| KernelError::Query(format!("invalid at_least height '{rest}': {e}")))?;
        return Ok(Consistency::AtLeast(h));
    }
    Err(KernelError::Query(format!("unknown consistency '{value}'")))
}

impl<S> Kernel<S>
where
    S: aos_store::Store + 'static,
{
    /// Handle an internal effect intent and return its receipt if the kind is supported.
    pub fn handle_internal_intent(
        &mut self,
        intent: &EffectIntent,
    ) -> Result<Option<EffectReceipt>, KernelError> {
        if !INTERNAL_EFFECT_KINDS.contains(&intent.kind.as_str()) {
            return Ok(None);
        }

        let receipt_result = match intent.kind.as_str() {
            EffectKind::INTROSPECT_MANIFEST => self.handle_manifest(intent),
            EffectKind::INTROSPECT_REDUCER_STATE => self.handle_reducer_state(intent),
            EffectKind::INTROSPECT_JOURNAL_HEAD => self.handle_journal_head(intent),
            EffectKind::INTROSPECT_LIST_CELLS => self.handle_list_cells(intent),
            "workspace.resolve" => self.handle_workspace_resolve(intent),
            "workspace.empty_root" => self.handle_workspace_empty_root(intent),
            "workspace.list" => self.handle_workspace_list(intent),
            "workspace.read_ref" => self.handle_workspace_read_ref(intent),
            "workspace.read_bytes" => self.handle_workspace_read_bytes(intent),
            "workspace.write_bytes" => self.handle_workspace_write_bytes(intent),
            "workspace.remove" => self.handle_workspace_remove(intent),
            "workspace.diff" => self.handle_workspace_diff(intent),
            "workspace.annotations_get" => self.handle_workspace_annotations_get(intent),
            "workspace.annotations_set" => self.handle_workspace_annotations_set(intent),
            "governance.propose" => self.handle_governance_propose(intent),
            "governance.shadow" => self.handle_governance_shadow(intent),
            "governance.approve" => self.handle_governance_approve(intent),
            "governance.apply" => self.handle_governance_apply(intent),
            _ => unreachable!("guard ensures only internal kinds reach here"),
        };

        let adapter_id = if intent.kind.as_str().starts_with("governance.") {
            GOVERNANCE_ADAPTER_ID
        } else {
            INTROSPECT_ADAPTER_ID
        };
        let receipt = match receipt_result {
            Ok(payload_cbor) => EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: adapter_id.to_string(),
                status: ReceiptStatus::Ok,
                payload_cbor,
                cost_cents: Some(0),
                signature: vec![0; 64],
            },
            Err(err) => EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: adapter_id.to_string(),
                status: ReceiptStatus::Error,
                payload_cbor: Vec::new(),
                cost_cents: Some(0),
                signature: vec![0; 64],
            },
        };

        Ok(Some(receipt))
    }

    fn handle_manifest(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let params: ManifestParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let consistency = parse_consistency(&params.consistency)?;
        let read = self.get_manifest(consistency)?;
        let manifest_bytes = to_canonical_cbor(&read.value)
            .map_err(|e| KernelError::Manifest(format!("encode manifest: {e}")))?;
        let receipt = ManifestReceipt {
            manifest: manifest_bytes,
            meta: to_meta(&read.meta),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_reducer_state(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let params: ReducerStateParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let consistency = parse_consistency(&params.consistency)?;
        let state_read =
            self.get_reducer_state(&params.reducer, params.key.as_deref(), consistency)?;
        let receipt = ReducerStateReceipt {
            state: state_read.value,
            meta: to_meta(&state_read.meta),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_journal_head(&self, _intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let meta = self.get_journal_head();
        let receipt = JournalHeadReceipt {
            meta: to_meta(&meta),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_list_cells(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let params: ListCellsParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let cells_meta = self.list_cells(&params.reducer)?;
        let cells: Vec<CellEntry> = cells_meta
            .into_iter()
            .map(|meta| CellEntry {
                key: meta.key_bytes,
                state_hash: meta.state_hash,
                size: meta.size,
                last_active_ns: meta.last_active_ns,
            })
            .collect();
        let receipt = ListCellsReceipt {
            cells,
            meta: to_meta(&self.read_meta()),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_workspace_resolve(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
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
            return Ok(to_canonical_cbor(&receipt)
                .map_err(|e| KernelError::Manifest(e.to_string()))?);
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
            return Ok(to_canonical_cbor(&receipt)
                .map_err(|e| KernelError::Manifest(e.to_string()))?);
        };
        let receipt = WorkspaceResolveReceipt {
            exists: true,
            resolved_version: Some(target),
            head: Some(head),
            root_hash: Some(meta.root_hash.clone()),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_workspace_empty_root(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
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

    fn handle_workspace_list(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
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
            _ => {
                return Err(KernelError::Query(format!(
                    "invalid scope '{}'",
                    scope
                )))
            }
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

    fn handle_workspace_read_ref(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
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

    fn handle_workspace_read_bytes(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let params: WorkspaceReadBytesParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let root_hash = parse_hash_str(&params.root_hash)?;
        let path_segments = validate_path(&params.path)?;
        let store = self.store();
        let Some(entry) = resolve_entry(store.as_ref(), &root_hash, &path_segments)? else {
            return Err(KernelError::Query("path not found".into()));
        };
        if entry.kind != "file" {
            return Err(KernelError::Query("path is not a file".into()));
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
        Ok(to_canonical_cbor(&bytes).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_workspace_write_bytes(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
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

    fn handle_workspace_remove(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let params: WorkspaceRemoveParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let root_hash = parse_hash_str(&params.root_hash)?;
        let path_segments = validate_path(&params.path)?;
        let store = self.store();
        let new_root = remove_entry_at_path(store.as_ref(), &root_hash, &path_segments)?;
        let receipt = WorkspaceRemoveReceipt {
            new_root_hash: hash_ref_from_hash(&new_root)?,
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_workspace_diff(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
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
        let root_a_hash = resolve_dir_hash(store.as_ref(), &root_a, &prefix_segments)?;
        let root_b_hash = resolve_dir_hash(store.as_ref(), &root_b, &prefix_segments)?;
        let mut entries_a = Vec::new();
        let mut entries_b = Vec::new();
        collect_subtree_entries(store.as_ref(), &root_a_hash, &prefix, &mut entries_a)?;
        collect_subtree_entries(store.as_ref(), &root_b_hash, &prefix, &mut entries_b)?;
        let mut map_a = HashMap::new();
        let mut map_b = HashMap::new();
        for (path, entry) in entries_a {
            map_a.insert(path, entry);
        }
        for (path, entry) in entries_b {
            map_b.insert(path, entry);
        }
        let mut paths: Vec<String> = map_a
            .keys()
            .chain(map_b.keys())
            .cloned()
            .collect();
        paths.sort();
        paths.dedup();
        let mut changes = Vec::new();
        for path in paths {
            let old = map_a.get(&path);
            let new = map_b.get(&path);
            if old
                .zip(new)
                .map(|(a, b)| {
                    a.kind == b.kind
                        && a.hash == b.hash
                        && a.annotations_hash == b.annotations_hash
                })
                .unwrap_or(false)
            {
                continue;
            }
            let kind = new
                .map(|e| e.kind.clone())
                .or_else(|| old.map(|e| e.kind.clone()))
                .unwrap_or_else(|| "file".into());
            let change = WorkspaceDiffChange {
                path,
                kind,
                old_hash: old.map(|e| e.hash.clone()),
                new_hash: new.map(|e| e.hash.clone()),
            };
            changes.push(change);
        }
        let receipt = WorkspaceDiffReceipt { changes };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_workspace_annotations_get(
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

    fn handle_workspace_annotations_set(
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
        let (new_root, annotations_hash) =
            set_annotations_at_path(store.as_ref(), &root_hash, &path_segments, &params.annotations_patch)?;
        let receipt = WorkspaceAnnotationsSetReceipt {
            new_root_hash: hash_ref_from_hash(&new_root)?,
            annotations_hash,
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_governance_propose(&mut self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let params: GovProposeParams = intent
            .params()
            .map_err(|e| KernelError::Manifest(format!("decode gov.propose params: {e}")))?;
        let patch_hash = match &params.patch {
            GovPatchInput::Hash(hash) => hash.clone(),
            _ => {
                return Err(KernelError::Manifest(
                    "gov.propose params must use patch hash input after normalization".into(),
                ));
            }
        };
        let patch =
            crate::governance_effects::load_patch_by_hash(self.store().as_ref(), &patch_hash)?;
        let proposal_id = self.submit_proposal(patch, params.description.clone())?;
        let receipt = GovProposeReceipt {
            proposal_id,
            patch_hash,
            manifest_base: params.manifest_base.clone(),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_governance_shadow(&mut self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let params: GovShadowParams = intent
            .params()
            .map_err(|e| KernelError::Manifest(format!("decode gov.shadow params: {e}")))?;
        let summary = self.run_shadow(params.proposal_id, None)?;
        let receipt = GovShadowReceipt {
            proposal_id: params.proposal_id,
            manifest_hash: HashRef::new(summary.manifest_hash)
                .map_err(|e| KernelError::Manifest(format!("invalid manifest hash: {e}")))?,
            predicted_effects: summary
                .predicted_effects
                .into_iter()
                .map(|effect| {
                    let intent_hash = hash_ref_from_hex(&effect.intent_hash)?;
                    let params_json = match effect.params_json {
                        Some(value) => Some(
                            serde_json::to_string(&value).map_err(|err| {
                                KernelError::Manifest(format!("encode params_json: {err}"))
                            })?,
                        ),
                        None => None,
                    };
                    Ok(GovPredictedEffect {
                        kind: effect.kind,
                        cap: effect.cap,
                        intent_hash,
                        params_json,
                    })
                })
                .collect::<Result<Vec<_>, KernelError>>()?,
            pending_receipts: summary
                .pending_receipts
                .into_iter()
                .map(|pending| {
                    Ok(GovPendingReceipt {
                        plan_id: pending.plan_id,
                        plan: pending.plan,
                        intent_hash: hash_ref_from_hex(&pending.intent_hash)?,
                    })
                })
                .collect::<Result<Vec<_>, KernelError>>()?,
            plan_results: summary
                .plan_results
                .into_iter()
                .map(|result| GovPlanResultPreview {
                    plan: result.plan,
                    plan_id: result.plan_id,
                    output_schema: result.output_schema,
                })
                .collect(),
            ledger_deltas: summary
                .ledger_deltas
                .into_iter()
                .map(|delta| GovLedgerDelta {
                    ledger: match delta.ledger {
                        crate::shadow::LedgerKind::Capability => GovLedgerKind::Capability,
                        crate::shadow::LedgerKind::Policy => GovLedgerKind::Policy,
                    },
                    name: delta.name,
                    change: match delta.change {
                        crate::shadow::DeltaKind::Added => GovDeltaKind::Added,
                        crate::shadow::DeltaKind::Removed => GovDeltaKind::Removed,
                        crate::shadow::DeltaKind::Changed => GovDeltaKind::Changed,
                    },
                })
                .collect(),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_governance_approve(&mut self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let params: GovApproveParams = intent
            .params()
            .map_err(|e| KernelError::Manifest(format!("decode gov.approve params: {e}")))?;
        let proposal = self
            .governance()
            .proposals()
            .get(&params.proposal_id)
            .ok_or(KernelError::ProposalNotFound(params.proposal_id))?;
        let patch_hash = HashRef::new(proposal.patch_hash.clone())
            .map_err(|e| KernelError::Manifest(format!("invalid patch hash: {e}")))?;
        match params.decision {
            GovApprovalDecision::Approve => self.approve_proposal(params.proposal_id, params.approver.clone())?,
            GovApprovalDecision::Reject => self.reject_proposal(params.proposal_id, params.approver.clone())?,
        }
        let receipt = GovApproveReceipt {
            proposal_id: params.proposal_id,
            decision: params.decision,
            patch_hash,
            approver: params.approver,
            reason: params.reason,
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_governance_apply(&mut self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let params: GovApplyParams = intent
            .params()
            .map_err(|e| KernelError::Manifest(format!("decode gov.apply params: {e}")))?;
        let proposal = self
            .governance()
            .proposals()
            .get(&params.proposal_id)
            .ok_or(KernelError::ProposalNotFound(params.proposal_id))?;
        let patch_hash = HashRef::new(proposal.patch_hash.clone())
            .map_err(|e| KernelError::Manifest(format!("invalid patch hash: {e}")))?;
        self.apply_proposal(params.proposal_id)?;
        let manifest_hash_new = HashRef::new(self.manifest_hash().to_hex())
            .map_err(|e| KernelError::Manifest(format!("invalid manifest hash: {e}")))?;
        let receipt = GovApplyReceipt {
            proposal_id: params.proposal_id,
            manifest_hash_new,
            patch_hash,
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }
}

fn hash_ref_from_hex(hex: &str) -> Result<HashRef, KernelError> {
    let value = format!("sha256:{hex}");
    HashRef::new(value).map_err(|e| KernelError::Manifest(format!("invalid hash: {e}")))
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
    Hash::from_hex_str(hash.as_str())
        .map_err(|e| KernelError::Query(format!("invalid hash: {e}")))
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
        if let Some(prev) = prev {
            if entry.name.as_str() <= prev {
                return Err(KernelError::Query("workspace tree not sorted".into()));
            }
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

fn load_tree<S: aos_store::Store>(
    store: &S,
    hash: &Hash,
) -> Result<WorkspaceTree, KernelError> {
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

fn remove_entry(entries: &mut Vec<WorkspaceEntry>, name: &str) -> Result<WorkspaceEntry, KernelError> {
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
        let entry = find_entry(&tree, segment).ok_or_else(|| {
            KernelError::Query(format!("missing path segment '{segment}'"))
        })?;
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
    if let Some(entry) = existing {
        if entry.kind == "file" {
            return Ok(entry.mode);
        }
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
        Some(hash) => annotations_from_hash(store, Some(hash))?
            .unwrap_or_default()
            .0,
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
        let annotations_hash = apply_annotations_patch(store, tree.annotations_hash.as_ref(), patch)?;
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
    let entry = find_entry(&tree, dir_name)
        .ok_or_else(|| KernelError::Query("path not found".into()))?;
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

fn to_meta(meta: &ReadMeta) -> MetaSer {
    MetaSer {
        journal_height: meta.journal_height,
        snapshot_hash: meta.snapshot_hash.as_ref().map(|h| h.as_bytes().to_vec()),
        manifest_hash: meta.manifest_hash.as_bytes().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KernelBuilder, KernelConfig};
    use aos_effects::IntentBuilder;
    use aos_store::{MemStore, Store};
    use serde_json::json;
    use std::fs::File;
    use std::io::Write;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn write_minimal_manifest(path: &std::path::Path) {
        let manifest = json!({
            "air_version": "1",
            "schemas": [],
            "modules": [],
            "plans": [],
            "effects": [],
            "caps": [],
            "policies": [],
            "triggers": []
        });
        let bytes = serde_cbor::to_vec(&manifest).expect("cbor encode");
        let mut file = File::create(path).expect("create manifest");
        file.write_all(&bytes).expect("write manifest");
    }

    fn open_kernel() -> Kernel<MemStore> {
        let tmp = TempDir::new().unwrap();
        let manifest_path = tmp.path().join("manifest.cbor");
        write_minimal_manifest(&manifest_path);
        let store = Arc::new(MemStore::new());
        KernelBuilder::new(store)
            .from_manifest_path(&manifest_path)
            .expect("kernel")
    }

    #[test]
    fn manifest_intent_produces_receipt() {
        let mut kernel = open_kernel();
        let params = ManifestParams {
            consistency: "head".into(),
        };
        let intent = IntentBuilder::new(EffectKind::introspect_manifest(), "sys/query@1", &params)
            .build()
            .unwrap();

        let receipt = kernel
            .handle_internal_intent(&intent)
            .expect("ok")
            .expect("handled");
        assert_eq!(receipt.status, ReceiptStatus::Ok);
        let decoded: ManifestReceipt = receipt.payload().unwrap();
        assert!(decoded.manifest.len() > 0);
        assert_eq!(decoded.meta.journal_height, 0);
    }

    #[test]
    fn parse_consistency_variants() {
        assert!(matches!(parse_consistency("head"), Ok(Consistency::Head)));
        assert_eq!(parse_consistency("exact:5").unwrap(), Consistency::Exact(5));
        assert_eq!(
            parse_consistency("at_least:10").unwrap(),
            Consistency::AtLeast(10)
        );
        assert!(parse_consistency("bogus").is_err());
    }

    #[test]
    fn invalid_params_returns_error_receipt() {
        let mut kernel = open_kernel();
        // bogus CBOR payload
        let intent = EffectIntent {
            kind: EffectKind::introspect_manifest(),
            cap_name: "sys/query@1".into(),
            params_cbor: b"\x01\x02\x03".to_vec(),
            idempotency_key: [0; 32],
            intent_hash: [9; 32],
        };
        let receipt = kernel
            .handle_internal_intent(&intent)
            .unwrap()
            .expect("handled");
        assert_eq!(receipt.status, ReceiptStatus::Error);
        assert_eq!(receipt.adapter_id, INTROSPECT_ADAPTER_ID);
    }

    #[test]
    fn list_cells_empty_for_non_keyed() {
        let mut kernel = open_kernel();
        let params = ListCellsParams {
            reducer: "missing/Reducer@1".into(),
        };
        let intent =
            IntentBuilder::new(EffectKind::introspect_list_cells(), "sys/query@1", &params)
                .build()
                .unwrap();

        let receipt = kernel
            .handle_internal_intent(&intent)
            .unwrap()
            .expect("handled");
        assert_eq!(receipt.status, ReceiptStatus::Ok);
        let decoded: ListCellsReceipt = receipt.payload().unwrap();
        assert!(decoded.cells.is_empty());
    }

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
        let entry = resolve_entry(
            &store,
            &new_root,
            &vec!["dir".into(), "file.txt".into()],
        )
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
