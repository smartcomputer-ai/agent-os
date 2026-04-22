use crate::contracts::{HostSessionStatus, ToolCallStatus, ToolMapper};
use alloc::string::{String, ToString};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "$tag", content = "$value")]
pub enum ToolEffectKind {
    HostSessionOpen,
    HostExec,
    HostSessionSignal,
    HostFsReadFile,
    HostFsWriteFile,
    HostFsEditFile,
    HostFsApplyPatch,
    HostFsGrep,
    HostFsGlob,
    HostFsStat,
    HostFsExists,
    HostFsListDir,
    IntrospectManifest,
    IntrospectWorkflowState,
    IntrospectListCells,
    WorkspaceResolve,
    WorkspaceEmptyRoot,
    WorkspaceList,
    WorkspaceReadRef,
    WorkspaceReadBytes,
    WorkspaceWriteBytes,
    WorkspaceWriteRef,
    WorkspaceRemove,
    WorkspaceDiff,
}

impl ToolEffectKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HostSessionOpen => "sys/host.session.open@1",
            Self::HostExec => "sys/host.exec@1",
            Self::HostSessionSignal => "sys/host.session.signal@1",
            Self::HostFsReadFile => "sys/host.fs.read_file@1",
            Self::HostFsWriteFile => "sys/host.fs.write_file@1",
            Self::HostFsEditFile => "sys/host.fs.edit_file@1",
            Self::HostFsApplyPatch => "sys/host.fs.apply_patch@1",
            Self::HostFsGrep => "sys/host.fs.grep@1",
            Self::HostFsGlob => "sys/host.fs.glob@1",
            Self::HostFsStat => "sys/host.fs.stat@1",
            Self::HostFsExists => "sys/host.fs.exists@1",
            Self::HostFsListDir => "sys/host.fs.list_dir@1",
            Self::IntrospectManifest => "sys/introspect.manifest@1",
            Self::IntrospectWorkflowState => "sys/introspect.workflow_state@1",
            Self::IntrospectListCells => "sys/introspect.list_cells@1",
            Self::WorkspaceResolve => "sys/workspace.resolve@1",
            Self::WorkspaceEmptyRoot => "sys/workspace.empty_root@1",
            Self::WorkspaceList => "sys/workspace.list@1",
            Self::WorkspaceReadRef => "sys/workspace.read_ref@1",
            Self::WorkspaceReadBytes => "sys/workspace.read_bytes@1",
            Self::WorkspaceWriteBytes => "sys/workspace.write_bytes@1",
            Self::WorkspaceWriteRef => "sys/workspace.write_ref@1",
            Self::WorkspaceRemove => "sys/workspace.remove@1",
            Self::WorkspaceDiff => "sys/workspace.diff@1",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolMappedArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_kind: Option<ToolEffectKind>,
    pub params_json: Value,
}

impl ToolMappedArgs {
    pub fn params(params_json: Value) -> Self {
        Self {
            effect_kind: None,
            params_json,
        }
    }

    pub fn with_effect_kind(effect_kind: ToolEffectKind, params_json: Value) -> Self {
        Self {
            effect_kind: Some(effect_kind),
            params_json,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolRuntimeDelta {
    pub host_session_id: Option<String>,
    pub host_session_status: Option<HostSessionStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolMappedReceipt {
    pub status: ToolCallStatus,
    pub llm_output_json: String,
    pub is_error: bool,
    pub runtime_delta: ToolRuntimeDelta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMappingErrorCode {
    InvalidArgs,
    MissingSession,
    Unsupported,
}

impl ToolMappingErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgs => "tool_invalid_args",
            Self::MissingSession => "missing_session",
            Self::Unsupported => "tool_unsupported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolMappingError {
    pub code: ToolMappingErrorCode,
    pub detail: String,
}

impl ToolMappingError {
    pub fn new(code: ToolMappingErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub fn invalid_args(detail: impl Into<String>) -> Self {
        Self::new(ToolMappingErrorCode::InvalidArgs, detail)
    }

    pub fn missing_session() -> Self {
        Self::new(
            ToolMappingErrorCode::MissingSession,
            "session_id is required and no host session is active",
        )
    }

    pub fn unsupported(detail: impl Into<String>) -> Self {
        Self::new(ToolMappingErrorCode::Unsupported, detail)
    }

    pub fn to_failed_status(&self) -> ToolCallStatus {
        ToolCallStatus::Failed {
            code: self.code.as_str().to_string(),
            detail: self.detail.clone(),
        }
    }

    pub fn to_code_text(&self) -> String {
        self.code.as_str().to_string()
    }
}

pub fn mapper_effect_kind(mapper: ToolMapper) -> ToolEffectKind {
    match mapper {
        ToolMapper::HostSessionOpen => ToolEffectKind::HostSessionOpen,
        ToolMapper::HostExec => ToolEffectKind::HostExec,
        ToolMapper::HostSessionSignal => ToolEffectKind::HostSessionSignal,
        ToolMapper::HostFsReadFile => ToolEffectKind::HostFsReadFile,
        ToolMapper::HostFsWriteFile => ToolEffectKind::HostFsWriteFile,
        ToolMapper::HostFsEditFile => ToolEffectKind::HostFsEditFile,
        ToolMapper::HostFsApplyPatch => ToolEffectKind::HostFsApplyPatch,
        ToolMapper::HostFsGrep => ToolEffectKind::HostFsGrep,
        ToolMapper::HostFsGlob => ToolEffectKind::HostFsGlob,
        ToolMapper::HostFsStat => ToolEffectKind::HostFsStat,
        ToolMapper::HostFsExists => ToolEffectKind::HostFsExists,
        ToolMapper::HostFsListDir => ToolEffectKind::HostFsListDir,
        ToolMapper::InspectWorld => ToolEffectKind::IntrospectManifest,
        ToolMapper::InspectWorkflow => ToolEffectKind::IntrospectWorkflowState,
        ToolMapper::WorkspaceInspect => ToolEffectKind::WorkspaceResolve,
        ToolMapper::WorkspaceList => ToolEffectKind::WorkspaceList,
        ToolMapper::WorkspaceRead => ToolEffectKind::WorkspaceReadRef,
        ToolMapper::WorkspaceApply => ToolEffectKind::WorkspaceWriteBytes,
        ToolMapper::WorkspaceDiff => ToolEffectKind::WorkspaceDiff,
        ToolMapper::WorkspaceCommit => {
            panic!("workspace commit does not map to an effect kind")
        }
    }
}
