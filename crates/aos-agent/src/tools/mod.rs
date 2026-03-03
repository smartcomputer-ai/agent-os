pub mod registry;
mod supported;
pub mod types;

use crate::contracts::{ToolMapper, ToolRuntimeContext};
use alloc::string::String;
use supported::{map_args as map_supported_args, map_receipt as map_supported_receipt};
pub use types::{ToolEffectKind, ToolMappedReceipt, ToolMappingError};

pub fn map_tool_arguments_to_effect_params(
    mapper: ToolMapper,
    arguments_json: &str,
    runtime: &ToolRuntimeContext,
) -> Result<serde_json::Value, ToolMappingError> {
    map_supported_args(mapper, arguments_json, runtime)
}

pub fn map_tool_receipt_to_llm_result(
    mapper: ToolMapper,
    tool_name: &str,
    status: &str,
    payload: &[u8],
) -> ToolMappedReceipt {
    map_supported_receipt(mapper, tool_name, status, payload)
}

pub fn effect_kind_for_mapper(mapper: ToolMapper) -> ToolEffectKind {
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

pub fn mapper_for_effect_kind(effect_kind: &str) -> Option<ToolMapper> {
    match effect_kind {
        "host.session.open" => Some(ToolMapper::HostSessionOpen),
        "host.exec" => Some(ToolMapper::HostExec),
        "host.session.signal" => Some(ToolMapper::HostSessionSignal),
        "host.fs.read_file" => Some(ToolMapper::HostFsReadFile),
        "host.fs.write_file" => Some(ToolMapper::HostFsWriteFile),
        "host.fs.edit_file" => Some(ToolMapper::HostFsEditFile),
        "host.fs.apply_patch" => Some(ToolMapper::HostFsApplyPatch),
        "host.fs.grep" => Some(ToolMapper::HostFsGrep),
        "host.fs.glob" => Some(ToolMapper::HostFsGlob),
        "host.fs.stat" => Some(ToolMapper::HostFsStat),
        "host.fs.exists" => Some(ToolMapper::HostFsExists),
        "host.fs.list_dir" => Some(ToolMapper::HostFsListDir),
        _ => None,
    }
}

pub fn empty_json_object_string() -> String {
    String::from("{}")
}
