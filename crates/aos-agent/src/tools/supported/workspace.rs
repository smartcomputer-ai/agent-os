use super::{build_receipt, failed_receipt};
use crate::contracts::{ToolCallStatus, ToolMapper};
use crate::tools::types::{ToolEffectOp, ToolMappedReceipt, ToolRuntimeDelta};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use aos_effect_types::introspect::{IntrospectListCellsReceipt, ReadMeta};
use aos_effect_types::workspace::{
    WorkspaceDiffReceipt, WorkspaceEmptyRootReceipt, WorkspaceListEntry, WorkspaceListReceipt,
    WorkspaceReadBytesReceipt, WorkspaceReadRefReceipt, WorkspaceRefEntry, WorkspaceResolveReceipt,
    WorkspaceWriteRefReceipt,
};
use aos_effects::builtins::BlobPutReceipt;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde::{Deserialize, Serialize};
use serde_cbor::Value as CborValue;
use serde_json::{Value, json};

const WORKSPACE_WORKFLOW_NAME: &str = "sys/Workspace@1";
const WORKSPACE_COMMIT_SCHEMA: &str = "sys/WorkspaceCommit@1";

#[derive(Debug)]
pub enum WorkspaceAction {
    Emit {
        effect_op: ToolEffectOp,
        params_json: Value,
        state_json: String,
    },
    EmitEvent {
        schema: &'static str,
        payload_json: Value,
        receipt: ToolMappedReceipt,
    },
    BlobPut {
        bytes: Vec<u8>,
        state_json: String,
    },
    Complete(ToolMappedReceipt),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceRefInput {
    workspace: Option<String>,
    version: Option<u64>,
    root_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RangeInput {
    start: u64,
    end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceInspectArgs {
    workspace: Option<String>,
    version: Option<u64>,
    root_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceListArgs {
    workspace: Option<String>,
    version: Option<u64>,
    root_hash: Option<String>,
    path: Option<String>,
    scope: Option<String>,
    limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceReadArgs {
    workspace: Option<String>,
    version: Option<u64>,
    root_hash: Option<String>,
    path: String,
    range: Option<RangeInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceApplyArgs {
    workspace: Option<String>,
    version: Option<u64>,
    root_hash: Option<String>,
    operations: Vec<ApplyOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceDiffArgs {
    left: WorkspaceRefInput,
    right: WorkspaceRefInput,
    prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceCommitArgs {
    workspace: String,
    root_hash: String,
    expected_head: Option<u64>,
    owner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApplyOperation {
    op: String,
    path: String,
    text: Option<String>,
    bytes_b64: Option<String>,
    blob_hash: Option<String>,
    mode: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResolvedWorkspace {
    workspace: Option<String>,
    requested_version: Option<u64>,
    resolved_version: Option<u64>,
    head: Option<u64>,
    root_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
enum WorkspaceState {
    ListWorkspaces,
    InspectResolve {
        workspace: String,
        version: Option<u64>,
    },
    ListResolve {
        workspace: String,
        version: Option<u64>,
        path: Option<String>,
        scope: String,
        limit: u64,
    },
    ListEntries {
        resolved: ResolvedWorkspace,
        path: Option<String>,
        scope: String,
        limit: u64,
    },
    ReadResolve {
        workspace: String,
        version: Option<u64>,
        path: String,
        range: Option<RangeInput>,
    },
    ReadRef {
        resolved: ResolvedWorkspace,
        path: String,
        range: Option<RangeInput>,
    },
    ReadBytes {
        resolved: ResolvedWorkspace,
        path: String,
        range: Option<RangeInput>,
        entry: WorkspaceRefEntry,
    },
    ApplyResolve {
        workspace: String,
        version: Option<u64>,
        operations: Vec<ApplyOperation>,
    },
    ApplyEmptyRoot {
        workspace: String,
        operations: Vec<ApplyOperation>,
    },
    ApplyRun {
        resolved: ResolvedWorkspace,
        base_root_hash: String,
        current_root_hash: String,
        operations: Vec<ApplyOperation>,
        next_index: usize,
        changes: Vec<Value>,
    },
    ApplyAwaitBlobPut {
        resolved: ResolvedWorkspace,
        base_root_hash: String,
        current_root_hash: String,
        operations: Vec<ApplyOperation>,
        next_index: usize,
        changes: Vec<Value>,
        path: String,
        mode: Option<u64>,
    },
    ApplyWriteRefReady {
        resolved: ResolvedWorkspace,
        base_root_hash: String,
        current_root_hash: String,
        operations: Vec<ApplyOperation>,
        next_index: usize,
        changes: Vec<Value>,
        path: String,
        mode: Option<u64>,
        blob_hash: String,
    },
    DiffResolveLeft {
        left: WorkspaceRefInput,
        right: WorkspaceRefInput,
        prefix: Option<String>,
    },
    DiffResolveRight {
        left: ResolvedWorkspace,
        right: WorkspaceRefInput,
        prefix: Option<String>,
    },
    DiffRun {
        left: ResolvedWorkspace,
        right: ResolvedWorkspace,
        prefix: Option<String>,
    },
}

pub fn is_workspace_mapper(mapper: ToolMapper) -> bool {
    matches!(
        mapper,
        ToolMapper::WorkspaceInspect
            | ToolMapper::WorkspaceList
            | ToolMapper::WorkspaceRead
            | ToolMapper::WorkspaceApply
            | ToolMapper::WorkspaceDiff
            | ToolMapper::WorkspaceCommit
    )
}

pub fn start_tool(
    mapper: ToolMapper,
    tool_name: &str,
    arguments_json: &str,
    emitted_at_ns: u64,
) -> Result<WorkspaceAction, crate::tools::types::ToolMappingError> {
    let initial = match mapper {
        ToolMapper::WorkspaceInspect => initial_inspect_state(arguments_json, tool_name)?,
        ToolMapper::WorkspaceList => initial_list_state(arguments_json)?,
        ToolMapper::WorkspaceRead => initial_read_state(arguments_json)?,
        ToolMapper::WorkspaceApply => initial_apply_state(arguments_json)?,
        ToolMapper::WorkspaceDiff => initial_diff_state(arguments_json)?,
        ToolMapper::WorkspaceCommit => {
            return initial_commit_action(arguments_json, tool_name, emitted_at_ns);
        }
        _ => {
            return Err(crate::tools::types::ToolMappingError::unsupported(
                "not a workspace tool",
            ));
        }
    };
    match initial {
        InitialState::Immediate(receipt) => Ok(WorkspaceAction::Complete(receipt)),
        InitialState::State(state) => advance_state(tool_name, state),
    }
}

pub fn resume_tool(
    tool_name: &str,
    state_json: &str,
    status: &str,
    payload: &[u8],
) -> WorkspaceAction {
    let state: WorkspaceState = match serde_json::from_str(state_json) {
        Ok(value) => value,
        Err(err) => {
            return WorkspaceAction::Complete(failed_receipt(
                tool_name,
                status,
                "workspace_state_decode_error",
                format!("failed to decode internal workspace state: {err}"),
            ));
        }
    };

    let transition = if !status.trim().eq_ignore_ascii_case("ok") {
        Transition::Complete(decode_error_receipt(tool_name, status, payload))
    } else {
        match state {
            WorkspaceState::ListWorkspaces => finish_list_workspaces(tool_name, status, payload),
            WorkspaceState::InspectResolve { workspace, version } => {
                finish_inspect_resolve(tool_name, status, payload, workspace, version)
            }
            WorkspaceState::ListResolve {
                workspace,
                version,
                path,
                scope,
                limit,
            } => on_list_resolve(
                tool_name, status, payload, workspace, version, path, scope, limit,
            ),
            WorkspaceState::ListEntries {
                resolved,
                path,
                scope,
                limit,
            } => finish_list_entries(tool_name, status, payload, resolved, path, scope, limit),
            WorkspaceState::ReadResolve {
                workspace,
                version,
                path,
                range,
            } => on_read_resolve(tool_name, status, payload, workspace, version, path, range),
            WorkspaceState::ReadRef {
                resolved,
                path,
                range,
            } => on_read_ref(tool_name, status, payload, resolved, path, range),
            WorkspaceState::ReadBytes {
                resolved,
                path,
                range,
                entry,
            } => finish_read_bytes(tool_name, status, payload, resolved, path, range, entry),
            WorkspaceState::ApplyResolve {
                workspace,
                version,
                operations,
            } => on_apply_resolve(tool_name, status, payload, workspace, version, operations),
            WorkspaceState::ApplyEmptyRoot {
                workspace,
                operations,
            } => on_apply_empty_root(tool_name, status, payload, workspace, operations),
            WorkspaceState::ApplyRun {
                resolved,
                base_root_hash,
                current_root_hash,
                operations,
                next_index,
                changes,
            } => on_apply_effect(
                tool_name,
                status,
                payload,
                resolved,
                base_root_hash,
                current_root_hash,
                operations,
                next_index,
                changes,
            ),
            WorkspaceState::ApplyAwaitBlobPut {
                resolved,
                base_root_hash,
                current_root_hash,
                operations,
                next_index,
                changes,
                path,
                mode,
            } => on_apply_blob_put(
                tool_name,
                status,
                payload,
                resolved,
                base_root_hash,
                current_root_hash,
                operations,
                next_index,
                changes,
                path,
                mode,
            ),
            WorkspaceState::ApplyWriteRefReady { .. } => Transition::Complete(failed_receipt(
                tool_name,
                status,
                "workspace_state_invalid",
                "workspace.apply write_ref-ready state should not receive a receipt directly",
            )),
            WorkspaceState::DiffResolveLeft {
                left,
                right,
                prefix,
            } => on_diff_resolve_left(tool_name, status, payload, left, right, prefix),
            WorkspaceState::DiffResolveRight {
                left,
                right,
                prefix,
            } => on_diff_resolve_right(tool_name, status, payload, left, right, prefix),
            WorkspaceState::DiffRun {
                left,
                right,
                prefix,
            } => finish_diff(tool_name, status, payload, left, right, prefix),
        }
    };

    match transition {
        Transition::State(state) => advance_state(tool_name, state).unwrap_or_else(|err| {
            WorkspaceAction::Complete(failed_receipt(
                tool_name,
                status,
                err.code.as_str(),
                err.detail,
            ))
        }),
        Transition::Complete(receipt) => WorkspaceAction::Complete(receipt),
    }
}

pub fn continue_tool(tool_name: &str, state_json: &str) -> WorkspaceAction {
    let state: WorkspaceState = match serde_json::from_str(state_json) {
        Ok(value) => value,
        Err(err) => {
            return WorkspaceAction::Complete(failed_receipt(
                tool_name,
                "ok",
                "workspace_state_decode_error",
                format!("failed to decode internal workspace state: {err}"),
            ));
        }
    };
    advance_state(tool_name, state).unwrap_or_else(|err| {
        WorkspaceAction::Complete(failed_receipt(
            tool_name,
            "ok",
            err.code.as_str(),
            err.detail,
        ))
    })
}

enum InitialState {
    Immediate(ToolMappedReceipt),
    State(WorkspaceState),
}

enum Transition {
    State(WorkspaceState),
    Complete(ToolMappedReceipt),
}

fn initial_inspect_state(
    arguments_json: &str,
    tool_name: &str,
) -> Result<InitialState, crate::tools::types::ToolMappingError> {
    let args: WorkspaceInspectArgs = parse_args(arguments_json)?;
    ensure_single_target(args.workspace.as_ref(), args.root_hash.as_ref())?;
    if let Some(root_hash) = args.root_hash {
        return Ok(InitialState::Immediate(success_receipt(
            tool_name,
            json!({
                "workspace": Value::Null,
                "exists": true,
                "resolved_version": Value::Null,
                "head": Value::Null,
                "root_hash": root_hash,
                "source": "root_hash",
            }),
        )));
    }

    let workspace = args.workspace.ok_or_else(|| {
        crate::tools::types::ToolMappingError::invalid_args(
            "either 'workspace' or 'root_hash' is required",
        )
    })?;
    Ok(InitialState::State(WorkspaceState::InspectResolve {
        workspace,
        version: args.version,
    }))
}

fn initial_list_state(
    arguments_json: &str,
) -> Result<InitialState, crate::tools::types::ToolMappingError> {
    let args: WorkspaceListArgs = parse_args(arguments_json)?;
    ensure_single_target(args.workspace.as_ref(), args.root_hash.as_ref())?;
    let scope = args.scope.unwrap_or_else(|| "dir".into());
    if scope != "dir" && scope != "subtree" {
        return Err(crate::tools::types::ToolMappingError::invalid_args(
            "'scope' must be either 'dir' or 'subtree'",
        ));
    }
    let limit = args.limit.unwrap_or(200);

    if let Some(root_hash) = args.root_hash {
        return Ok(InitialState::State(WorkspaceState::ListEntries {
            resolved: ResolvedWorkspace {
                workspace: None,
                requested_version: None,
                resolved_version: None,
                head: None,
                root_hash,
            },
            path: args.path,
            scope,
            limit,
        }));
    }
    if let Some(workspace) = args.workspace {
        return Ok(InitialState::State(WorkspaceState::ListResolve {
            workspace,
            version: args.version,
            path: args.path,
            scope,
            limit,
        }));
    }
    Ok(InitialState::State(WorkspaceState::ListWorkspaces))
}

fn initial_read_state(
    arguments_json: &str,
) -> Result<InitialState, crate::tools::types::ToolMappingError> {
    let args: WorkspaceReadArgs = parse_args(arguments_json)?;
    ensure_single_target(args.workspace.as_ref(), args.root_hash.as_ref())?;
    if let Some(root_hash) = args.root_hash {
        return Ok(InitialState::State(WorkspaceState::ReadRef {
            resolved: ResolvedWorkspace {
                workspace: None,
                requested_version: None,
                resolved_version: None,
                head: None,
                root_hash,
            },
            path: args.path,
            range: args.range,
        }));
    }
    let workspace = args.workspace.ok_or_else(|| {
        crate::tools::types::ToolMappingError::invalid_args(
            "either 'workspace' or 'root_hash' is required",
        )
    })?;
    Ok(InitialState::State(WorkspaceState::ReadResolve {
        workspace,
        version: args.version,
        path: args.path,
        range: args.range,
    }))
}

fn initial_apply_state(
    arguments_json: &str,
) -> Result<InitialState, crate::tools::types::ToolMappingError> {
    let args: WorkspaceApplyArgs = parse_args(arguments_json)?;
    ensure_single_target(args.workspace.as_ref(), args.root_hash.as_ref())?;
    validate_operations(&args.operations)?;
    if let Some(root_hash) = args.root_hash {
        return Ok(InitialState::State(WorkspaceState::ApplyRun {
            resolved: ResolvedWorkspace {
                workspace: args.workspace,
                requested_version: args.version,
                resolved_version: args.version,
                head: None,
                root_hash: root_hash.clone(),
            },
            base_root_hash: root_hash.clone(),
            current_root_hash: root_hash,
            operations: args.operations,
            next_index: 0,
            changes: Vec::new(),
        }));
    }
    let workspace = args.workspace.ok_or_else(|| {
        crate::tools::types::ToolMappingError::invalid_args(
            "either 'workspace' or 'root_hash' is required",
        )
    })?;
    Ok(InitialState::State(WorkspaceState::ApplyResolve {
        workspace,
        version: args.version,
        operations: args.operations,
    }))
}

fn initial_diff_state(
    arguments_json: &str,
) -> Result<InitialState, crate::tools::types::ToolMappingError> {
    let args: WorkspaceDiffArgs = parse_args(arguments_json)?;
    validate_ref(&args.left)?;
    validate_ref(&args.right)?;
    Ok(InitialState::State(WorkspaceState::DiffResolveLeft {
        left: args.left,
        right: args.right,
        prefix: args.prefix,
    }))
}

fn initial_commit_action(
    arguments_json: &str,
    tool_name: &str,
    emitted_at_ns: u64,
) -> Result<WorkspaceAction, crate::tools::types::ToolMappingError> {
    let args: WorkspaceCommitArgs = parse_args(arguments_json)?;
    if args.workspace.trim().is_empty() {
        return Err(crate::tools::types::ToolMappingError::invalid_args(
            "'workspace' must be a non-empty string",
        ));
    }
    if args.root_hash.trim().is_empty() {
        return Err(crate::tools::types::ToolMappingError::invalid_args(
            "'root_hash' must be a non-empty string",
        ));
    }
    let owner = args
        .owner
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "agent".into());
    let payload_json = json!({
        "workspace": args.workspace,
        "expected_head": args.expected_head,
        "meta": {
            "root_hash": args.root_hash,
            "owner": owner,
            "created_at": emitted_at_ns,
        }
    });
    let receipt = success_receipt(
        tool_name,
        json!({
            "workspace": payload_json.get("workspace").cloned().unwrap_or(Value::Null),
            "expected_head": args.expected_head,
            "root_hash": payload_json
                .get("meta")
                .and_then(|value| value.get("root_hash"))
                .cloned()
                .unwrap_or(Value::Null),
            "owner": payload_json
                .get("meta")
                .and_then(|value| value.get("owner"))
                .cloned()
                .unwrap_or(Value::Null),
            "created_at": emitted_at_ns,
            "emitted": true,
            "schema": WORKSPACE_COMMIT_SCHEMA,
        }),
    );
    Ok(WorkspaceAction::EmitEvent {
        schema: WORKSPACE_COMMIT_SCHEMA,
        payload_json,
        receipt,
    })
}

fn advance_state(
    tool_name: &str,
    state: WorkspaceState,
) -> Result<WorkspaceAction, crate::tools::types::ToolMappingError> {
    match state {
        WorkspaceState::ListWorkspaces => emit(
            ToolEffectOp::IntrospectListCells,
            json!({ "workflow": WORKSPACE_WORKFLOW_NAME }),
            WorkspaceState::ListWorkspaces,
        ),
        WorkspaceState::InspectResolve { workspace, version } => emit(
            ToolEffectOp::WorkspaceResolve,
            resolve_params(workspace.as_str(), version),
            WorkspaceState::InspectResolve { workspace, version },
        ),
        WorkspaceState::ListResolve {
            workspace,
            version,
            path,
            scope,
            limit,
        } => emit(
            ToolEffectOp::WorkspaceResolve,
            resolve_params(workspace.as_str(), version),
            WorkspaceState::ListResolve {
                workspace,
                version,
                path,
                scope,
                limit,
            },
        ),
        WorkspaceState::ListEntries {
            resolved,
            path,
            scope,
            limit,
        } => emit(
            ToolEffectOp::WorkspaceList,
            json!({
                "root_hash": resolved.root_hash,
                "path": path,
                "scope": scope,
                "cursor": Value::Null,
                "limit": limit,
            }),
            WorkspaceState::ListEntries {
                resolved,
                path,
                scope,
                limit,
            },
        ),
        WorkspaceState::ReadResolve {
            workspace,
            version,
            path,
            range,
        } => emit(
            ToolEffectOp::WorkspaceResolve,
            resolve_params(workspace.as_str(), version),
            WorkspaceState::ReadResolve {
                workspace,
                version,
                path,
                range,
            },
        ),
        WorkspaceState::ReadRef {
            resolved,
            path,
            range,
        } => emit(
            ToolEffectOp::WorkspaceReadRef,
            json!({
                "root_hash": resolved.root_hash,
                "path": path,
            }),
            WorkspaceState::ReadRef {
                resolved,
                path,
                range,
            },
        ),
        WorkspaceState::ReadBytes {
            resolved,
            path,
            range,
            entry,
        } => emit(
            ToolEffectOp::WorkspaceReadBytes,
            json!({
                "root_hash": resolved.root_hash,
                "path": path,
                "range": range,
            }),
            WorkspaceState::ReadBytes {
                resolved,
                path,
                range,
                entry,
            },
        ),
        WorkspaceState::ApplyResolve {
            workspace,
            version,
            operations,
        } => emit(
            ToolEffectOp::WorkspaceResolve,
            resolve_params(workspace.as_str(), version),
            WorkspaceState::ApplyResolve {
                workspace,
                version,
                operations,
            },
        ),
        WorkspaceState::ApplyEmptyRoot {
            workspace,
            operations,
        } => emit(
            ToolEffectOp::WorkspaceEmptyRoot,
            json!({ "workspace": workspace }),
            WorkspaceState::ApplyEmptyRoot {
                workspace,
                operations,
            },
        ),
        WorkspaceState::ApplyRun {
            resolved,
            base_root_hash,
            current_root_hash,
            operations,
            next_index,
            changes,
        } => {
            if next_index >= operations.len() {
                Ok(WorkspaceAction::Complete(success_receipt(
                    tool_name,
                    json!({
                        "workspace": resolved.workspace,
                        "requested_version": resolved.requested_version,
                        "resolved_version": resolved.resolved_version,
                        "head": resolved.head,
                        "base_root_hash": base_root_hash,
                        "new_root_hash": current_root_hash,
                        "changes": changes,
                    }),
                )))
            } else {
                let op = operations[next_index].clone();
                match apply_effect_for_op(current_root_hash.as_str(), &op)? {
                    ApplyEffect::Effect {
                        effect_op,
                        params_json,
                    } => emit(
                        effect_op,
                        params_json,
                        WorkspaceState::ApplyRun {
                            resolved,
                            base_root_hash,
                            current_root_hash,
                            operations,
                            next_index,
                            changes,
                        },
                    ),
                    ApplyEffect::BlobPut { bytes, path, mode } => blob_put(
                        bytes,
                        WorkspaceState::ApplyAwaitBlobPut {
                            resolved,
                            base_root_hash,
                            current_root_hash,
                            operations,
                            next_index,
                            changes,
                            path,
                            mode,
                        },
                    ),
                }
            }
        }
        WorkspaceState::ApplyAwaitBlobPut { .. } => {
            Err(crate::tools::types::ToolMappingError::unsupported(
                "blob.put receipt is required before inline workspace writes can continue",
            ))
        }
        WorkspaceState::ApplyWriteRefReady {
            resolved,
            base_root_hash,
            current_root_hash,
            operations,
            next_index,
            changes,
            path,
            mode,
            blob_hash,
        } => emit(
            ToolEffectOp::WorkspaceWriteRef,
            json!({
                "root_hash": current_root_hash,
                "path": path,
                "blob_hash": blob_hash,
                "mode": mode,
            }),
            WorkspaceState::ApplyRun {
                resolved,
                base_root_hash,
                current_root_hash,
                operations,
                next_index,
                changes,
            },
        ),
        WorkspaceState::DiffResolveLeft {
            left,
            right,
            prefix,
        } => {
            if let Some(root_hash) = left.root_hash {
                advance_state(
                    tool_name,
                    WorkspaceState::DiffResolveRight {
                        left: ResolvedWorkspace {
                            workspace: left.workspace,
                            requested_version: left.version,
                            resolved_version: left.version,
                            head: None,
                            root_hash,
                        },
                        right,
                        prefix,
                    },
                )
            } else {
                let workspace = left.workspace.clone().ok_or_else(|| {
                    crate::tools::types::ToolMappingError::invalid_args(
                        "workspace ref must contain either 'workspace' or 'root_hash'",
                    )
                })?;
                emit(
                    ToolEffectOp::WorkspaceResolve,
                    resolve_params(workspace.as_str(), left.version),
                    WorkspaceState::DiffResolveLeft {
                        left,
                        right,
                        prefix,
                    },
                )
            }
        }
        WorkspaceState::DiffResolveRight {
            left,
            right,
            prefix,
        } => {
            if let Some(root_hash) = right.root_hash {
                advance_state(
                    tool_name,
                    WorkspaceState::DiffRun {
                        left,
                        right: ResolvedWorkspace {
                            workspace: right.workspace,
                            requested_version: right.version,
                            resolved_version: right.version,
                            head: None,
                            root_hash,
                        },
                        prefix,
                    },
                )
            } else {
                let workspace = right.workspace.clone().ok_or_else(|| {
                    crate::tools::types::ToolMappingError::invalid_args(
                        "workspace ref must contain either 'workspace' or 'root_hash'",
                    )
                })?;
                emit(
                    ToolEffectOp::WorkspaceResolve,
                    resolve_params(workspace.as_str(), right.version),
                    WorkspaceState::DiffResolveRight {
                        left,
                        right,
                        prefix,
                    },
                )
            }
        }
        WorkspaceState::DiffRun {
            left,
            right,
            prefix,
        } => emit(
            ToolEffectOp::WorkspaceDiff,
            json!({
                "root_a": left.root_hash,
                "root_b": right.root_hash,
                "prefix": prefix,
            }),
            WorkspaceState::DiffRun {
                left,
                right,
                prefix,
            },
        ),
    }
}

fn finish_list_workspaces(tool_name: &str, status: &str, payload: &[u8]) -> Transition {
    let receipt: IntrospectListCellsReceipt = match serde_cbor::from_slice(payload) {
        Ok(value) => value,
        Err(err) => {
            return Transition::Complete(failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                format!("failed to decode workspace cell list receipt: {err}"),
            ));
        }
    };
    let mut entries = receipt
        .cells
        .iter()
        .filter_map(|cell| decode_workspace_name(&cell.key).map(|name| (name, cell)))
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Transition::Complete(success_receipt(
        tool_name,
        json!({
            "kind": "workspaces",
            "entries": entries.into_iter().map(|(name, cell)| json!({
                "path": name,
                "kind": "workspace",
                "state_hash": cell.state_hash.to_string(),
                "size": cell.size,
                "last_active_ns": cell.last_active_ns,
            })).collect::<Vec<_>>(),
            "meta": meta_json(&receipt.meta),
        }),
    ))
}

fn finish_inspect_resolve(
    tool_name: &str,
    status: &str,
    payload: &[u8],
    workspace: String,
    version: Option<u64>,
) -> Transition {
    let receipt: WorkspaceResolveReceipt = match serde_cbor::from_slice(payload) {
        Ok(value) => value,
        Err(err) => {
            return Transition::Complete(failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                format!("failed to decode workspace.resolve receipt: {err}"),
            ));
        }
    };
    Transition::Complete(success_receipt(
        tool_name,
        json!({
            "workspace": workspace,
            "requested_version": version,
            "exists": receipt.exists,
            "resolved_version": receipt.resolved_version,
            "head": receipt.head,
            "root_hash": receipt.root_hash.map(|hash| hash.to_string()),
            "source": "workspace",
        }),
    ))
}

fn on_list_resolve(
    tool_name: &str,
    status: &str,
    payload: &[u8],
    workspace: String,
    version: Option<u64>,
    path: Option<String>,
    scope: String,
    limit: u64,
) -> Transition {
    let receipt: WorkspaceResolveReceipt = match serde_cbor::from_slice(payload) {
        Ok(value) => value,
        Err(err) => {
            return Transition::Complete(failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                format!("failed to decode workspace.resolve receipt: {err}"),
            ));
        }
    };
    let Some(root_hash) = receipt.root_hash else {
        return Transition::Complete(failed_receipt(
            tool_name,
            status,
            "workspace_not_found",
            format!("workspace '{}' was not found", workspace),
        ));
    };
    Transition::State(WorkspaceState::ListEntries {
        resolved: ResolvedWorkspace {
            workspace: Some(workspace),
            requested_version: version,
            resolved_version: receipt.resolved_version,
            head: receipt.head,
            root_hash: root_hash.to_string(),
        },
        path,
        scope,
        limit,
    })
}

fn finish_list_entries(
    tool_name: &str,
    status: &str,
    payload: &[u8],
    resolved: ResolvedWorkspace,
    path: Option<String>,
    scope: String,
    _limit: u64,
) -> Transition {
    let receipt: WorkspaceListReceipt = match serde_cbor::from_slice(payload) {
        Ok(value) => value,
        Err(err) => {
            return Transition::Complete(failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                format!("failed to decode workspace.list receipt: {err}"),
            ));
        }
    };
    Transition::Complete(success_receipt(
        tool_name,
        json!({
            "kind": "tree",
            "workspace": resolved.workspace,
            "requested_version": resolved.requested_version,
            "resolved_version": resolved.resolved_version,
            "head": resolved.head,
            "root_hash": resolved.root_hash,
            "path": path,
            "scope": scope,
            "entries": receipt.entries.iter().map(list_entry_json).collect::<Vec<_>>(),
            "next_cursor": receipt.next_cursor,
        }),
    ))
}

fn on_read_resolve(
    tool_name: &str,
    status: &str,
    payload: &[u8],
    workspace: String,
    version: Option<u64>,
    path: String,
    range: Option<RangeInput>,
) -> Transition {
    let receipt: WorkspaceResolveReceipt = match serde_cbor::from_slice(payload) {
        Ok(value) => value,
        Err(err) => {
            return Transition::Complete(failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                format!("failed to decode workspace.resolve receipt: {err}"),
            ));
        }
    };
    let Some(root_hash) = receipt.root_hash else {
        return Transition::Complete(failed_receipt(
            tool_name,
            status,
            "workspace_not_found",
            format!("workspace '{}' was not found", workspace),
        ));
    };
    Transition::State(WorkspaceState::ReadRef {
        resolved: ResolvedWorkspace {
            workspace: Some(workspace),
            requested_version: version,
            resolved_version: receipt.resolved_version,
            head: receipt.head,
            root_hash: root_hash.to_string(),
        },
        path,
        range,
    })
}

fn on_read_ref(
    tool_name: &str,
    status: &str,
    payload: &[u8],
    resolved: ResolvedWorkspace,
    path: String,
    range: Option<RangeInput>,
) -> Transition {
    let receipt: WorkspaceReadRefReceipt = match serde_cbor::from_slice(payload) {
        Ok(value) => value,
        Err(err) => {
            return Transition::Complete(failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                format!("failed to decode workspace.read_ref receipt: {err}"),
            ));
        }
    };

    let Some(entry) = receipt else {
        return Transition::Complete(success_receipt(
            tool_name,
            json!({
                "workspace": resolved.workspace,
                "requested_version": resolved.requested_version,
                "resolved_version": resolved.resolved_version,
                "head": resolved.head,
                "root_hash": resolved.root_hash,
                "path": path,
                "exists": false,
                "entry": Value::Null,
                "content": {
                    "encoding": "none"
                }
            }),
        ));
    };

    if entry.kind != "file" {
        return Transition::Complete(success_receipt(
            tool_name,
            json!({
                "workspace": resolved.workspace,
                "requested_version": resolved.requested_version,
                "resolved_version": resolved.resolved_version,
                "head": resolved.head,
                "root_hash": resolved.root_hash,
                "path": path,
                "exists": true,
                "entry": ref_entry_json(&entry),
                "content": {
                    "encoding": "none"
                }
            }),
        ));
    }

    Transition::State(WorkspaceState::ReadBytes {
        resolved,
        path,
        range,
        entry,
    })
}

fn finish_read_bytes(
    tool_name: &str,
    status: &str,
    payload: &[u8],
    resolved: ResolvedWorkspace,
    path: String,
    range: Option<RangeInput>,
    entry: WorkspaceRefEntry,
) -> Transition {
    let bytes = match decode_workspace_read_bytes_payload(payload) {
        Ok(value) => value,
        Err(detail) => {
            return Transition::Complete(failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                detail,
            ));
        }
    };
    let content = decode_content(&bytes);
    Transition::Complete(success_receipt(
        tool_name,
        json!({
            "workspace": resolved.workspace,
            "requested_version": resolved.requested_version,
            "resolved_version": resolved.resolved_version,
            "head": resolved.head,
            "root_hash": resolved.root_hash,
            "path": path,
            "range": range,
            "exists": true,
            "entry": ref_entry_json(&entry),
            "content": content,
        }),
    ))
}

fn on_apply_resolve(
    tool_name: &str,
    status: &str,
    payload: &[u8],
    workspace: String,
    version: Option<u64>,
    operations: Vec<ApplyOperation>,
) -> Transition {
    let receipt: WorkspaceResolveReceipt = match serde_cbor::from_slice(payload) {
        Ok(value) => value,
        Err(err) => {
            return Transition::Complete(failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                format!("failed to decode workspace.resolve receipt: {err}"),
            ));
        }
    };
    if let Some(root_hash) = receipt.root_hash {
        let root = root_hash.to_string();
        return Transition::State(WorkspaceState::ApplyRun {
            resolved: ResolvedWorkspace {
                workspace: Some(workspace),
                requested_version: version,
                resolved_version: receipt.resolved_version,
                head: receipt.head,
                root_hash: root.clone(),
            },
            base_root_hash: root.clone(),
            current_root_hash: root,
            operations,
            next_index: 0,
            changes: Vec::new(),
        });
    }

    Transition::State(WorkspaceState::ApplyEmptyRoot {
        workspace,
        operations,
    })
}

fn on_apply_empty_root(
    tool_name: &str,
    status: &str,
    payload: &[u8],
    workspace: String,
    operations: Vec<ApplyOperation>,
) -> Transition {
    let receipt: WorkspaceEmptyRootReceipt = match serde_cbor::from_slice(payload) {
        Ok(value) => value,
        Err(err) => {
            return Transition::Complete(failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                format!("failed to decode workspace.empty_root receipt: {err}"),
            ));
        }
    };
    let root = receipt.root_hash.to_string();
    Transition::State(WorkspaceState::ApplyRun {
        resolved: ResolvedWorkspace {
            workspace: Some(workspace),
            requested_version: None,
            resolved_version: None,
            head: None,
            root_hash: root.clone(),
        },
        base_root_hash: root.clone(),
        current_root_hash: root,
        operations,
        next_index: 0,
        changes: Vec::new(),
    })
}

#[allow(clippy::too_many_arguments)]
fn on_apply_effect(
    tool_name: &str,
    status: &str,
    payload: &[u8],
    resolved: ResolvedWorkspace,
    base_root_hash: String,
    _current_root_hash: String,
    operations: Vec<ApplyOperation>,
    next_index: usize,
    mut changes: Vec<Value>,
) -> Transition {
    let (new_root_hash, blob_hash) =
        if let Ok(receipt) = serde_cbor::from_slice::<WorkspaceWriteRefReceipt>(payload) {
            (
                receipt.new_root_hash.to_string(),
                Some(receipt.blob_hash.to_string()),
            )
        } else if let Ok(receipt) =
            serde_cbor::from_slice::<aos_effect_types::workspace::WorkspaceRemoveReceipt>(payload)
        {
            (receipt.new_root_hash.to_string(), None)
        } else {
            return Transition::Complete(failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                "failed to decode workspace apply receipt",
            ));
        };

    let op = operations[next_index].clone();
    changes.push(json!({
        "op": op.op,
        "path": op.path,
        "mode": op.mode,
        "blob_hash": blob_hash,
    }));

    Transition::State(WorkspaceState::ApplyRun {
        resolved,
        base_root_hash,
        current_root_hash: new_root_hash,
        operations,
        next_index: next_index.saturating_add(1),
        changes,
    })
}

#[allow(clippy::too_many_arguments)]
fn on_apply_blob_put(
    tool_name: &str,
    status: &str,
    payload: &[u8],
    resolved: ResolvedWorkspace,
    base_root_hash: String,
    current_root_hash: String,
    operations: Vec<ApplyOperation>,
    next_index: usize,
    changes: Vec<Value>,
    path: String,
    mode: Option<u64>,
) -> Transition {
    let receipt: BlobPutReceipt = match serde_cbor::from_slice(payload) {
        Ok(value) => value,
        Err(err) => {
            return Transition::Complete(failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                format!("failed to decode blob.put receipt: {err}"),
            ));
        }
    };
    Transition::State(WorkspaceState::ApplyWriteRefReady {
        resolved,
        base_root_hash,
        current_root_hash,
        operations,
        next_index,
        changes,
        path,
        mode,
        blob_hash: receipt.blob_ref.to_string(),
    })
}

fn on_diff_resolve_left(
    tool_name: &str,
    status: &str,
    payload: &[u8],
    left: WorkspaceRefInput,
    right: WorkspaceRefInput,
    prefix: Option<String>,
) -> Transition {
    let left = match resolve_ref_receipt(tool_name, status, payload, left) {
        Ok(value) => value,
        Err(receipt) => return Transition::Complete(receipt),
    };
    Transition::State(WorkspaceState::DiffResolveRight {
        left,
        right,
        prefix,
    })
}

fn on_diff_resolve_right(
    tool_name: &str,
    status: &str,
    payload: &[u8],
    left: ResolvedWorkspace,
    right: WorkspaceRefInput,
    prefix: Option<String>,
) -> Transition {
    let right = match resolve_ref_receipt(tool_name, status, payload, right) {
        Ok(value) => value,
        Err(receipt) => return Transition::Complete(receipt),
    };
    Transition::State(WorkspaceState::DiffRun {
        left,
        right,
        prefix,
    })
}

fn finish_diff(
    tool_name: &str,
    status: &str,
    payload: &[u8],
    left: ResolvedWorkspace,
    right: ResolvedWorkspace,
    prefix: Option<String>,
) -> Transition {
    let receipt: WorkspaceDiffReceipt = match serde_cbor::from_slice(payload) {
        Ok(value) => value,
        Err(err) => {
            return Transition::Complete(failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                format!("failed to decode workspace.diff receipt: {err}"),
            ));
        }
    };
    Transition::Complete(success_receipt(
        tool_name,
        json!({
            "left": resolved_workspace_json(&left),
            "right": resolved_workspace_json(&right),
            "prefix": prefix,
            "changes": receipt.changes.iter().map(|change| json!({
                "path": change.path,
                "kind": change.kind,
                "old_hash": change.old_hash.as_ref().map(|hash| hash.to_string()),
                "new_hash": change.new_hash.as_ref().map(|hash| hash.to_string()),
            })).collect::<Vec<_>>(),
        }),
    ))
}

fn resolve_ref_receipt(
    tool_name: &str,
    status: &str,
    payload: &[u8],
    input: WorkspaceRefInput,
) -> Result<ResolvedWorkspace, ToolMappedReceipt> {
    if let Some(root_hash) = input.root_hash {
        return Ok(ResolvedWorkspace {
            workspace: input.workspace,
            requested_version: input.version,
            resolved_version: input.version,
            head: None,
            root_hash,
        });
    }

    let receipt: WorkspaceResolveReceipt = serde_cbor::from_slice(payload).map_err(|err| {
        failed_receipt(
            tool_name,
            status,
            "receipt_decode_error",
            format!("failed to decode workspace.resolve receipt: {err}"),
        )
    })?;
    let workspace = input.workspace.unwrap_or_default();
    let root_hash = receipt.root_hash.ok_or_else(|| {
        failed_receipt(
            tool_name,
            status,
            "workspace_not_found",
            format!("workspace '{}' was not found", workspace),
        )
    })?;
    Ok(ResolvedWorkspace {
        workspace: Some(workspace),
        requested_version: input.version,
        resolved_version: receipt.resolved_version,
        head: receipt.head,
        root_hash: root_hash.to_string(),
    })
}

enum ApplyEffect {
    Effect {
        effect_op: ToolEffectOp,
        params_json: Value,
    },
    BlobPut {
        bytes: Vec<u8>,
        path: String,
        mode: Option<u64>,
    },
}

fn apply_effect_for_op(
    current_root_hash: &str,
    op: &ApplyOperation,
) -> Result<ApplyEffect, crate::tools::types::ToolMappingError> {
    match op.op.as_str() {
        "remove" => Ok(ApplyEffect::Effect {
            effect_op: ToolEffectOp::WorkspaceRemove,
            params_json: json!({
                "root_hash": current_root_hash,
                "path": op.path,
            }),
        }),
        "write" => {
            if let Some(text) = op.text.as_ref() {
                return Ok(ApplyEffect::BlobPut {
                    bytes: text.as_bytes().to_vec(),
                    path: op.path.clone(),
                    mode: op.mode,
                });
            }
            if let Some(bytes_b64) = op.bytes_b64.as_ref() {
                let bytes = BASE64_STANDARD.decode(bytes_b64).map_err(|err| {
                    crate::tools::types::ToolMappingError::invalid_args(format!(
                        "invalid base64 bytes for path '{}': {err}",
                        op.path
                    ))
                })?;
                return Ok(ApplyEffect::BlobPut {
                    bytes,
                    path: op.path.clone(),
                    mode: op.mode,
                });
            }
            if let Some(blob_hash) = op.blob_hash.as_ref() {
                return Ok(ApplyEffect::Effect {
                    effect_op: ToolEffectOp::WorkspaceWriteRef,
                    params_json: json!({
                        "root_hash": current_root_hash,
                        "path": op.path,
                        "blob_hash": blob_hash,
                        "mode": op.mode,
                    }),
                });
            }
            Err(crate::tools::types::ToolMappingError::invalid_args(
                "write operations require exactly one of 'text', 'bytes_b64', or 'blob_hash'",
            ))
        }
        _ => Err(crate::tools::types::ToolMappingError::invalid_args(
            "operation 'op' must be either 'write' or 'remove'",
        )),
    }
}

fn validate_operations(
    operations: &[ApplyOperation],
) -> Result<(), crate::tools::types::ToolMappingError> {
    if operations.is_empty() {
        return Err(crate::tools::types::ToolMappingError::invalid_args(
            "'operations' must contain at least one operation",
        ));
    }
    for op in operations {
        if op.path.trim().is_empty() {
            return Err(crate::tools::types::ToolMappingError::invalid_args(
                "operation 'path' must be non-empty",
            ));
        }
        match op.op.as_str() {
            "remove" => {
                if op.text.is_some() || op.bytes_b64.is_some() || op.blob_hash.is_some() {
                    return Err(crate::tools::types::ToolMappingError::invalid_args(
                        "remove operations cannot include content fields",
                    ));
                }
            }
            "write" => {
                let sources = op.text.is_some() as u8
                    + op.bytes_b64.is_some() as u8
                    + op.blob_hash.is_some() as u8;
                if sources != 1 {
                    return Err(crate::tools::types::ToolMappingError::invalid_args(
                        "write operations require exactly one of 'text', 'bytes_b64', or 'blob_hash'",
                    ));
                }
            }
            _ => {
                return Err(crate::tools::types::ToolMappingError::invalid_args(
                    "operation 'op' must be either 'write' or 'remove'",
                ));
            }
        }
    }
    Ok(())
}

fn validate_ref(input: &WorkspaceRefInput) -> Result<(), crate::tools::types::ToolMappingError> {
    ensure_single_target(input.workspace.as_ref(), input.root_hash.as_ref()).and_then(|_| {
        if input.workspace.is_none() && input.root_hash.is_none() {
            Err(crate::tools::types::ToolMappingError::invalid_args(
                "workspace ref must contain either 'workspace' or 'root_hash'",
            ))
        } else {
            Ok(())
        }
    })
}

fn ensure_single_target(
    workspace: Option<&String>,
    root_hash: Option<&String>,
) -> Result<(), crate::tools::types::ToolMappingError> {
    if workspace.is_some() && root_hash.is_some() {
        Err(crate::tools::types::ToolMappingError::invalid_args(
            "'workspace' and 'root_hash' are mutually exclusive",
        ))
    } else {
        Ok(())
    }
}

fn resolve_params(workspace: &str, version: Option<u64>) -> Value {
    json!({
        "workspace": workspace,
        "version": version,
    })
}

fn parse_args<T: for<'de> Deserialize<'de>>(
    arguments_json: &str,
) -> Result<T, crate::tools::types::ToolMappingError> {
    serde_json::from_str(arguments_json).map_err(|err| {
        crate::tools::types::ToolMappingError::invalid_args(format!(
            "arguments JSON invalid: {err}"
        ))
    })
}

fn emit(
    effect_op: ToolEffectOp,
    params_json: Value,
    state: WorkspaceState,
) -> Result<WorkspaceAction, crate::tools::types::ToolMappingError> {
    let state_json = serde_json::to_string(&state).map_err(|err| {
        crate::tools::types::ToolMappingError::unsupported(format!(
            "failed to encode internal workspace state: {err}"
        ))
    })?;
    Ok(WorkspaceAction::Emit {
        effect_op,
        params_json,
        state_json,
    })
}

fn blob_put(
    bytes: Vec<u8>,
    state: WorkspaceState,
) -> Result<WorkspaceAction, crate::tools::types::ToolMappingError> {
    let state_json = serde_json::to_string(&state).map_err(|err| {
        crate::tools::types::ToolMappingError::unsupported(format!(
            "failed to encode internal workspace state: {err}"
        ))
    })?;
    Ok(WorkspaceAction::BlobPut { bytes, state_json })
}

fn success_receipt(tool_name: &str, result: Value) -> ToolMappedReceipt {
    build_receipt(
        tool_name,
        "ok",
        result,
        false,
        ToolCallStatus::Succeeded,
        ToolRuntimeDelta::default(),
    )
}

fn decode_error_receipt(tool_name: &str, status: &str, payload: &[u8]) -> ToolMappedReceipt {
    if let Ok(message) = serde_cbor::from_slice::<String>(payload) {
        return failed_receipt(tool_name, status, "adapter_error", message);
    }
    if let Ok(value) = serde_cbor::from_slice::<Value>(payload) {
        let code = value
            .get("error_code")
            .and_then(Value::as_str)
            .unwrap_or("adapter_error");
        let detail = value
            .get("error_message")
            .and_then(Value::as_str)
            .unwrap_or("workspace tool request failed");
        return failed_receipt(tool_name, status, code, detail);
    }
    failed_receipt(
        tool_name,
        status,
        "adapter_error",
        format!("workspace tool failed with status={status}"),
    )
}

fn decode_workspace_name(bytes: &[u8]) -> Option<String> {
    serde_cbor::from_slice::<String>(bytes).ok()
}

fn meta_json(meta: &ReadMeta) -> Value {
    json!({
        "journal_height": meta.journal_height,
        "snapshot_hash": meta.snapshot_hash.as_ref().map(|hash| hash.to_string()),
        "manifest_hash": meta.manifest_hash.to_string(),
    })
}

fn list_entry_json(entry: &WorkspaceListEntry) -> Value {
    json!({
        "path": entry.path,
        "kind": entry.kind,
        "hash": entry.hash.as_ref().map(|hash| hash.to_string()),
        "size": entry.size,
        "mode": entry.mode,
    })
}

fn ref_entry_json(entry: &WorkspaceRefEntry) -> Value {
    json!({
        "kind": entry.kind,
        "hash": entry.hash.to_string(),
        "size": entry.size,
        "mode": entry.mode,
    })
}

fn decode_content(bytes: &[u8]) -> Value {
    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        return json!({
            "encoding": "json",
            "json": value,
            "bytes_len": bytes.len(),
        });
    }
    if let Ok(text) = core::str::from_utf8(bytes) {
        return json!({
            "encoding": "utf8",
            "text": text,
            "bytes_len": bytes.len(),
        });
    }
    json!({
        "encoding": "base64",
        "bytes_b64": BASE64_STANDARD.encode(bytes),
        "bytes_len": bytes.len(),
    })
}

fn resolved_workspace_json(resolved: &ResolvedWorkspace) -> Value {
    json!({
        "workspace": resolved.workspace,
        "requested_version": resolved.requested_version,
        "resolved_version": resolved.resolved_version,
        "head": resolved.head,
        "root_hash": resolved.root_hash,
    })
}

fn decode_workspace_read_bytes_payload(
    payload: &[u8],
) -> Result<WorkspaceReadBytesReceipt, String> {
    match serde_cbor::from_slice::<WorkspaceReadBytesReceipt>(payload) {
        Ok(value) => Ok(value),
        Err(primary_err) => {
            let value: CborValue = serde_cbor::from_slice(payload).map_err(|err| {
                format!(
                    "failed to decode workspace.read_bytes receipt: {primary_err}; raw cbor decode also failed: {err}"
                )
            })?;
            extract_bytes_from_cbor_value(&value).ok_or_else(|| {
                format!(
                    "failed to decode workspace.read_bytes receipt: {primary_err}; unexpected cbor shape: {value:?}"
                )
            })
        }
    }
}

fn extract_bytes_from_cbor_value(value: &CborValue) -> Option<Vec<u8>> {
    match value {
        CborValue::Bytes(bytes) => Some(bytes.clone()),
        CborValue::Array(items) => items
            .iter()
            .map(|item| match item {
                CborValue::Integer(value) if (0..=255).contains(value) => Some(*value as u8),
                _ => None,
            })
            .collect(),
        CborValue::Map(entries) => {
            for (key, inner) in entries {
                if matches!(key, CborValue::Text(text) if text == "$value" || text == "bytes")
                    && let Some(bytes) = extract_bytes_from_cbor_value(inner)
                {
                    return Some(bytes);
                }
            }
            None
        }
        _ => None,
    }
}
