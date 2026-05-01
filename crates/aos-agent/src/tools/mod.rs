pub mod registry;
pub(crate) mod supported;
pub mod types;

use crate::contracts::{ToolMapper, ToolRuntimeContext};
use alloc::string::String;
pub use supported::{
    CompositeToolAction, continue_composite_tool, is_composite_tool_mapper, resume_composite_tool,
    start_composite_tool,
};
use supported::{map_args as map_supported_args, map_receipt as map_supported_receipt};
pub use types::{ToolEffectOp, ToolMappedArgs, ToolMappedReceipt, ToolMappingError};

pub fn map_tool_arguments_to_effect_params(
    mapper: ToolMapper,
    arguments_json: &str,
    runtime: &ToolRuntimeContext,
) -> Result<ToolMappedArgs, ToolMappingError> {
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

pub fn effect_for_mapper(mapper: ToolMapper) -> ToolEffectOp {
    match mapper {
        ToolMapper::HostSessionOpen => ToolEffectOp::HostSessionOpen,
        ToolMapper::HostExec => ToolEffectOp::HostExec,
        ToolMapper::HostSessionSignal => ToolEffectOp::HostSessionSignal,
        ToolMapper::HostFsReadFile => ToolEffectOp::HostFsReadFile,
        ToolMapper::HostFsWriteFile => ToolEffectOp::HostFsWriteFile,
        ToolMapper::HostFsEditFile => ToolEffectOp::HostFsEditFile,
        ToolMapper::HostFsApplyPatch => ToolEffectOp::HostFsApplyPatch,
        ToolMapper::HostFsGrep => ToolEffectOp::HostFsGrep,
        ToolMapper::HostFsGlob => ToolEffectOp::HostFsGlob,
        ToolMapper::HostFsStat => ToolEffectOp::HostFsStat,
        ToolMapper::HostFsExists => ToolEffectOp::HostFsExists,
        ToolMapper::HostFsListDir => ToolEffectOp::HostFsListDir,
        ToolMapper::InspectWorld => ToolEffectOp::IntrospectManifest,
        ToolMapper::InspectWorkflow => ToolEffectOp::IntrospectWorkflowState,
        ToolMapper::WorkspaceInspect => ToolEffectOp::WorkspaceResolve,
        ToolMapper::WorkspaceList => ToolEffectOp::WorkspaceList,
        ToolMapper::WorkspaceRead => ToolEffectOp::WorkspaceReadRef,
        ToolMapper::WorkspaceApply => ToolEffectOp::WorkspaceWriteBytes,
        ToolMapper::WorkspaceDiff => ToolEffectOp::WorkspaceDiff,
        ToolMapper::WorkspaceCommit => {
            panic!("workspace commit does not map to an effect")
        }
    }
}

pub fn mapper_for_effect(effect: &str) -> Option<ToolMapper> {
    match effect {
        "sys/host.session.open@1" | "host.session.open" => Some(ToolMapper::HostSessionOpen),
        "sys/host.exec@1" | "host.exec" => Some(ToolMapper::HostExec),
        "sys/host.session.signal@1" | "host.session.signal" => Some(ToolMapper::HostSessionSignal),
        "sys/host.fs.read_file@1" | "host.fs.read_file" => Some(ToolMapper::HostFsReadFile),
        "sys/host.fs.write_file@1" | "host.fs.write_file" => Some(ToolMapper::HostFsWriteFile),
        "sys/host.fs.edit_file@1" | "host.fs.edit_file" => Some(ToolMapper::HostFsEditFile),
        "sys/host.fs.apply_patch@1" | "host.fs.apply_patch" => Some(ToolMapper::HostFsApplyPatch),
        "sys/host.fs.grep@1" | "host.fs.grep" => Some(ToolMapper::HostFsGrep),
        "sys/host.fs.glob@1" | "host.fs.glob" => Some(ToolMapper::HostFsGlob),
        "sys/host.fs.stat@1" | "host.fs.stat" => Some(ToolMapper::HostFsStat),
        "sys/host.fs.exists@1" | "host.fs.exists" => Some(ToolMapper::HostFsExists),
        "sys/host.fs.list_dir@1" | "host.fs.list_dir" => Some(ToolMapper::HostFsListDir),
        "sys/introspect.manifest@1" | "introspect.manifest" => Some(ToolMapper::InspectWorld),
        "sys/introspect.workflow_state@1" | "introspect.workflow_state" => {
            Some(ToolMapper::InspectWorkflow)
        }
        "sys/introspect.list_cells@1" | "introspect.list_cells" => {
            Some(ToolMapper::InspectWorkflow)
        }
        _ => None,
    }
}

pub fn empty_json_object_string() -> String {
    String::from("{}")
}
