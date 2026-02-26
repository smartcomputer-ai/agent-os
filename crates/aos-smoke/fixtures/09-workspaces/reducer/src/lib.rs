#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::{collections::BTreeMap, format, string::String, vec::Vec};
use aos_wasm_sdk::{
    EffectReceiptEnvelope, ReduceError, Reducer, ReducerCtx, aos_reducer, aos_variant,
};
use serde::{Deserialize, Serialize};

const WORKSPACE_RESOLVE_EFFECT: &str = "workspace.resolve";
const WORKSPACE_EMPTY_ROOT_EFFECT: &str = "workspace.empty_root";
const WORKSPACE_WRITE_BYTES_EFFECT: &str = "workspace.write_bytes";
const WORKSPACE_LIST_EFFECT: &str = "workspace.list";
const WORKSPACE_DIFF_EFFECT: &str = "workspace.diff";
const WORKSPACE_COMMIT_SCHEMA: &str = "sys/WorkspaceCommit@1";

aos_reducer!(WorkspaceDemo);

#[derive(Default)]
struct WorkspaceDemo;

impl Reducer for WorkspaceDemo {
    type State = WorkspaceState;
    type Event = WorkspaceEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        match event {
            WorkspaceEvent::Start(start) => handle_start(ctx, start)?,
            WorkspaceEvent::Receipt(envelope) => handle_receipt(ctx, envelope)?,
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct WorkspaceState {
    workspaces: BTreeMap<String, WorkspaceSummary>,
    pending_workspaces: Vec<String>,
    active_workspace: Option<String>,
    active_owner: Option<String>,
    active_step: Option<WorkspaceStep>,
    active_expected_head: Option<u64>,
    active_base_root: Option<String>,
    active_current_root: Option<String>,
    active_entry_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceSummary {
    version: Option<u64>,
    root_hash: String,
    entry_count: u64,
    diff_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceStart {
    workspaces: Vec<String>,
    owner: String,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum WorkspaceStep {
        Resolving,
        CreatingEmptyRoot,
        WritingMarker,
        WritingReadme,
        WritingData,
        Listing,
        WritingReadmeUpdate,
        Diffing,
    }
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum WorkspaceEvent {
        Start(WorkspaceStart),
        Receipt(EffectReceiptEnvelope),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceResolveParams {
    workspace: String,
    version: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceResolveReceipt {
    exists: bool,
    resolved_version: Option<u64>,
    head: Option<u64>,
    root_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceEmptyRootParams {
    workspace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceEmptyRootReceipt {
    root_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceWriteBytesParams {
    root_hash: String,
    path: String,
    #[serde(with = "serde_bytes")]
    bytes: Vec<u8>,
    mode: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceWriteBytesReceipt {
    new_root_hash: String,
    blob_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceListParams {
    root_hash: String,
    path: Option<String>,
    scope: Option<String>,
    cursor: Option<String>,
    limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceListEntry {
    path: String,
    kind: String,
    hash: Option<String>,
    size: Option<u64>,
    mode: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceListReceipt {
    entries: Vec<WorkspaceListEntry>,
    next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceDiffParams {
    root_a: String,
    root_b: String,
    prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceDiffChange {
    path: String,
    kind: String,
    old_hash: Option<String>,
    new_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceDiffReceipt {
    changes: Vec<WorkspaceDiffChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceCommitMeta {
    root_hash: String,
    owner: String,
    created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceCommit {
    workspace: String,
    expected_head: Option<u64>,
    meta: WorkspaceCommitMeta,
}

fn handle_start(ctx: &mut ReducerCtx<WorkspaceState, ()>, start: WorkspaceStart) -> Result<(), ReduceError> {
    if ctx.state.active_step.is_some() {
        return Ok(());
    }
    ctx.state.pending_workspaces = start.workspaces;
    ctx.state.active_owner = Some(start.owner);
    maybe_begin_next_workspace(ctx)
}

fn handle_receipt(
    ctx: &mut ReducerCtx<WorkspaceState, ()>,
    envelope: EffectReceiptEnvelope,
) -> Result<(), ReduceError> {
    let Some(step) = ctx.state.active_step.clone() else {
        return Ok(());
    };
    if envelope.effect_kind != effect_kind_for_step(&step) {
        return Ok(());
    }

    match step {
        WorkspaceStep::Resolving => {
            let receipt: WorkspaceResolveReceipt = envelope
                .decode_receipt_payload()
                .map_err(|_| ReduceError::new("invalid workspace.resolve receipt payload"))?;
            ctx.state.active_expected_head = receipt.head;
            if receipt.exists {
                let root = receipt
                    .root_hash
                    .ok_or_else(|| ReduceError::new("workspace.resolve missing root_hash"))?;
                ctx.state.active_base_root = Some(root.clone());
                ctx.state.active_current_root = Some(root.clone());
                emit_write_marker(ctx, root)?;
            } else {
                emit_empty_root(ctx)?;
            }
        }
        WorkspaceStep::CreatingEmptyRoot => {
            let receipt: WorkspaceEmptyRootReceipt = envelope
                .decode_receipt_payload()
                .map_err(|_| ReduceError::new("invalid workspace.empty_root receipt payload"))?;
            ctx.state.active_base_root = Some(receipt.root_hash.clone());
            ctx.state.active_current_root = Some(receipt.root_hash.clone());
            emit_write_marker(ctx, receipt.root_hash)?;
        }
        WorkspaceStep::WritingMarker => {
            let receipt: WorkspaceWriteBytesReceipt = envelope
                .decode_receipt_payload()
                .map_err(|_| ReduceError::new("invalid workspace.write_bytes marker receipt"))?;
            ctx.state.active_current_root = Some(receipt.new_root_hash.clone());
            emit_write_readme(ctx, receipt.new_root_hash)?;
        }
        WorkspaceStep::WritingReadme => {
            let receipt: WorkspaceWriteBytesReceipt = envelope
                .decode_receipt_payload()
                .map_err(|_| ReduceError::new("invalid workspace.write_bytes readme receipt"))?;
            ctx.state.active_current_root = Some(receipt.new_root_hash.clone());
            emit_write_data(ctx, receipt.new_root_hash)?;
        }
        WorkspaceStep::WritingData => {
            let receipt: WorkspaceWriteBytesReceipt = envelope
                .decode_receipt_payload()
                .map_err(|_| ReduceError::new("invalid workspace.write_bytes data receipt"))?;
            ctx.state.active_base_root = Some(receipt.new_root_hash.clone());
            ctx.state.active_current_root = Some(receipt.new_root_hash.clone());
            emit_list(ctx, receipt.new_root_hash)?;
        }
        WorkspaceStep::Listing => {
            let receipt: WorkspaceListReceipt = envelope
                .decode_receipt_payload()
                .map_err(|_| ReduceError::new("invalid workspace.list receipt payload"))?;
            ctx.state.active_entry_count = Some(receipt.entries.len() as u64);
            let root = ctx
                .state
                .active_current_root
                .clone()
                .ok_or_else(|| ReduceError::new("missing active root after list"))?;
            emit_write_readme_update(ctx, root)?;
        }
        WorkspaceStep::WritingReadmeUpdate => {
            let receipt: WorkspaceWriteBytesReceipt = envelope
                .decode_receipt_payload()
                .map_err(|_| ReduceError::new("invalid workspace.write_bytes update receipt"))?;
            ctx.state.active_current_root = Some(receipt.new_root_hash.clone());
            let base = ctx
                .state
                .active_base_root
                .clone()
                .ok_or_else(|| ReduceError::new("missing base root for diff"))?;
            emit_diff(ctx, base, receipt.new_root_hash)?;
        }
        WorkspaceStep::Diffing => {
            let receipt: WorkspaceDiffReceipt = envelope
                .decode_receipt_payload()
                .map_err(|_| ReduceError::new("invalid workspace.diff receipt payload"))?;
            finalize_active_workspace(ctx, receipt.changes.len() as u64)?;
            maybe_begin_next_workspace(ctx)?;
        }
    }
    Ok(())
}

fn maybe_begin_next_workspace(ctx: &mut ReducerCtx<WorkspaceState, ()>) -> Result<(), ReduceError> {
    if ctx.state.active_step.is_some() {
        return Ok(());
    }
    if ctx.state.pending_workspaces.is_empty() {
        ctx.state.active_owner = None;
        return Ok(());
    }

    let workspace = ctx.state.pending_workspaces.remove(0);
    ctx.state.active_workspace = Some(workspace.clone());
    ctx.state.active_expected_head = None;
    ctx.state.active_base_root = None;
    ctx.state.active_current_root = None;
    ctx.state.active_entry_count = None;

    let params = WorkspaceResolveParams {
        workspace,
        version: None,
    };
    ctx.effects()
        .emit_raw(WORKSPACE_RESOLVE_EFFECT, &params, Some("default"));
    ctx.state.active_step = Some(WorkspaceStep::Resolving);
    Ok(())
}

fn emit_empty_root(ctx: &mut ReducerCtx<WorkspaceState, ()>) -> Result<(), ReduceError> {
    let workspace = ctx
        .state
        .active_workspace
        .clone()
        .ok_or_else(|| ReduceError::new("missing active workspace"))?;
    let params = WorkspaceEmptyRootParams { workspace };
    ctx.effects()
        .emit_raw(WORKSPACE_EMPTY_ROOT_EFFECT, &params, Some("default"));
    ctx.state.active_step = Some(WorkspaceStep::CreatingEmptyRoot);
    Ok(())
}

fn emit_write_marker(
    ctx: &mut ReducerCtx<WorkspaceState, ()>,
    root_hash: String,
) -> Result<(), ReduceError> {
    let workspace = ctx
        .state
        .active_workspace
        .clone()
        .ok_or_else(|| ReduceError::new("missing active workspace"))?;
    let params = WorkspaceWriteBytesParams {
        root_hash,
        path: format!("seed/{workspace}.txt"),
        bytes: format!("seed marker for {workspace}\n").into_bytes(),
        mode: None,
    };
    ctx.effects()
        .emit_raw(WORKSPACE_WRITE_BYTES_EFFECT, &params, Some("default"));
    ctx.state.active_step = Some(WorkspaceStep::WritingMarker);
    Ok(())
}

fn emit_write_readme(
    ctx: &mut ReducerCtx<WorkspaceState, ()>,
    root_hash: String,
) -> Result<(), ReduceError> {
    let params = WorkspaceWriteBytesParams {
        root_hash,
        path: "README.txt".into(),
        bytes: b"seeded by workflow\n".to_vec(),
        mode: None,
    };
    ctx.effects()
        .emit_raw(WORKSPACE_WRITE_BYTES_EFFECT, &params, Some("default"));
    ctx.state.active_step = Some(WorkspaceStep::WritingReadme);
    Ok(())
}

fn emit_write_data(
    ctx: &mut ReducerCtx<WorkspaceState, ()>,
    root_hash: String,
) -> Result<(), ReduceError> {
    let params = WorkspaceWriteBytesParams {
        root_hash,
        path: "data.json".into(),
        bytes: b"{\"demo\":true,\"items\":[1,2,3]}\n".to_vec(),
        mode: None,
    };
    ctx.effects()
        .emit_raw(WORKSPACE_WRITE_BYTES_EFFECT, &params, Some("default"));
    ctx.state.active_step = Some(WorkspaceStep::WritingData);
    Ok(())
}

fn emit_list(ctx: &mut ReducerCtx<WorkspaceState, ()>, root_hash: String) -> Result<(), ReduceError> {
    let params = WorkspaceListParams {
        root_hash,
        path: None,
        scope: Some("subtree".into()),
        cursor: None,
        limit: 200,
    };
    ctx.effects()
        .emit_raw(WORKSPACE_LIST_EFFECT, &params, Some("default"));
    ctx.state.active_step = Some(WorkspaceStep::Listing);
    Ok(())
}

fn emit_write_readme_update(
    ctx: &mut ReducerCtx<WorkspaceState, ()>,
    root_hash: String,
) -> Result<(), ReduceError> {
    let params = WorkspaceWriteBytesParams {
        root_hash,
        path: "README.txt".into(),
        bytes: b"updated by workflow\n".to_vec(),
        mode: None,
    };
    ctx.effects()
        .emit_raw(WORKSPACE_WRITE_BYTES_EFFECT, &params, Some("default"));
    ctx.state.active_step = Some(WorkspaceStep::WritingReadmeUpdate);
    Ok(())
}

fn emit_diff(
    ctx: &mut ReducerCtx<WorkspaceState, ()>,
    root_a: String,
    root_b: String,
) -> Result<(), ReduceError> {
    let params = WorkspaceDiffParams {
        root_a,
        root_b,
        prefix: None,
    };
    ctx.effects()
        .emit_raw(WORKSPACE_DIFF_EFFECT, &params, Some("default"));
    ctx.state.active_step = Some(WorkspaceStep::Diffing);
    Ok(())
}

fn finalize_active_workspace(
    ctx: &mut ReducerCtx<WorkspaceState, ()>,
    diff_count: u64,
) -> Result<(), ReduceError> {
    let workspace = ctx
        .state
        .active_workspace
        .clone()
        .ok_or_else(|| ReduceError::new("missing active workspace during finalize"))?;
    let owner = ctx
        .state
        .active_owner
        .clone()
        .ok_or_else(|| ReduceError::new("missing active owner during finalize"))?;
    let root_hash = ctx
        .state
        .active_current_root
        .clone()
        .ok_or_else(|| ReduceError::new("missing active root during finalize"))?;
    let entry_count = ctx.state.active_entry_count.unwrap_or(0);
    let expected_head = ctx.state.active_expected_head;

    let version = match expected_head {
        Some(v) => Some(v.saturating_add(1)),
        None => ctx
            .state
            .workspaces
            .get(&workspace)
            .and_then(|summary| summary.version)
            .map(|v| v.saturating_add(1))
            .or(Some(1)),
    };

    ctx.state.workspaces.insert(
        workspace.clone(),
        WorkspaceSummary {
            version,
            root_hash: root_hash.clone(),
            entry_count,
            diff_count,
        },
    );

    let commit = WorkspaceCommit {
        workspace: workspace.clone(),
        expected_head,
        meta: WorkspaceCommitMeta {
            root_hash,
            owner,
            created_at: ctx.now_ns().unwrap_or(0),
        },
    };
    ctx.intent(WORKSPACE_COMMIT_SCHEMA).payload(&commit).send();

    ctx.state.active_workspace = None;
    ctx.state.active_step = None;
    ctx.state.active_expected_head = None;
    ctx.state.active_base_root = None;
    ctx.state.active_current_root = None;
    ctx.state.active_entry_count = None;
    Ok(())
}

fn effect_kind_for_step(step: &WorkspaceStep) -> &'static str {
    match step {
        WorkspaceStep::Resolving => WORKSPACE_RESOLVE_EFFECT,
        WorkspaceStep::CreatingEmptyRoot => WORKSPACE_EMPTY_ROOT_EFFECT,
        WorkspaceStep::WritingMarker
        | WorkspaceStep::WritingReadme
        | WorkspaceStep::WritingData
        | WorkspaceStep::WritingReadmeUpdate => WORKSPACE_WRITE_BYTES_EFFECT,
        WorkspaceStep::Listing => WORKSPACE_LIST_EFFECT,
        WorkspaceStep::Diffing => WORKSPACE_DIFF_EFFECT,
    }
}
