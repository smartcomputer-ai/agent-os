use crate::contracts::{HostSessionStatus, ToolCallStatus, ToolMapper};
use alloc::string::{String, ToString};
use serde::{Deserialize, Serialize};

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
}

impl ToolEffectKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HostSessionOpen => "host.session.open",
            Self::HostExec => "host.exec",
            Self::HostSessionSignal => "host.session.signal",
            Self::HostFsReadFile => "host.fs.read_file",
            Self::HostFsWriteFile => "host.fs.write_file",
            Self::HostFsEditFile => "host.fs.edit_file",
            Self::HostFsApplyPatch => "host.fs.apply_patch",
            Self::HostFsGrep => "host.fs.grep",
            Self::HostFsGlob => "host.fs.glob",
            Self::HostFsStat => "host.fs.stat",
            Self::HostFsExists => "host.fs.exists",
            Self::HostFsListDir => "host.fs.list_dir",
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
    }
}
