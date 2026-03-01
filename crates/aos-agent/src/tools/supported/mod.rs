use super::types::{ToolMappedReceipt, ToolMappingError};
use crate::contracts::{HostSessionStatus, ToolCallStatus, ToolMapper, ToolRuntimeContext};
use alloc::format;
use alloc::string::{String, ToString};
use serde_json::{Map, Value, json};

mod host_exec;
mod host_fs_apply_patch;
mod host_fs_edit_file;
mod host_fs_exists;
mod host_fs_glob;
mod host_fs_grep;
mod host_fs_list_dir;
mod host_fs_read_file;
mod host_fs_stat;
mod host_fs_write_file;
mod host_session_open;
mod host_session_signal;

pub fn map_args(
    mapper: ToolMapper,
    arguments_json: &str,
    runtime: &ToolRuntimeContext,
) -> Result<Value, ToolMappingError> {
    match mapper {
        ToolMapper::HostSessionOpen => host_session_open::map_args(arguments_json),
        ToolMapper::HostExec => host_exec::map_args(arguments_json, runtime),
        ToolMapper::HostSessionSignal => host_session_signal::map_args(arguments_json, runtime),
        ToolMapper::HostFsReadFile => host_fs_read_file::map_args(arguments_json, runtime),
        ToolMapper::HostFsWriteFile => host_fs_write_file::map_args(arguments_json, runtime),
        ToolMapper::HostFsEditFile => host_fs_edit_file::map_args(arguments_json, runtime),
        ToolMapper::HostFsApplyPatch => host_fs_apply_patch::map_args(arguments_json, runtime),
        ToolMapper::HostFsGrep => host_fs_grep::map_args(arguments_json, runtime),
        ToolMapper::HostFsGlob => host_fs_glob::map_args(arguments_json, runtime),
        ToolMapper::HostFsStat => host_fs_stat::map_args(arguments_json, runtime),
        ToolMapper::HostFsExists => host_fs_exists::map_args(arguments_json, runtime),
        ToolMapper::HostFsListDir => host_fs_list_dir::map_args(arguments_json, runtime),
    }
}

pub fn map_receipt(
    mapper: ToolMapper,
    tool_name: &str,
    status: &str,
    payload: &[u8],
) -> ToolMappedReceipt {
    let payload_json = serde_cbor::from_slice::<Value>(payload).unwrap_or_else(|_| {
        json!({
            "status": "error",
            "error_code": "receipt_decode_error",
            "error_message": "failed to decode receipt payload as JSON"
        })
    });

    let status_normalized = status.trim().to_ascii_lowercase();
    let payload_status = payload_json
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("ok")
        .to_ascii_lowercase();

    let is_error = status_normalized != "ok"
        || payload_status.contains("error")
        || payload_status.contains("failed")
        || payload_json
            .get("error_code")
            .and_then(Value::as_str)
            .is_some();

    let (runtime_host_session_id, runtime_status_override) = match mapper {
        ToolMapper::HostSessionOpen => {
            let id = payload_json
                .get("session_id")
                .and_then(Value::as_str)
                .map(|s| s.to_string());
            let mapped_status = payload_json
                .get("status")
                .and_then(Value::as_str)
                .and_then(parse_host_session_status);
            (id, mapped_status)
        }
        ToolMapper::HostSessionSignal => {
            let mapped_status = payload_json
                .get("status")
                .and_then(Value::as_str)
                .and_then(parse_host_session_status);
            (None, mapped_status)
        }
        _ => (None, None),
    };

    let failed_code = payload_json
        .get("error_code")
        .and_then(Value::as_str)
        .unwrap_or(if status_normalized == "timeout" {
            "adapter_timeout"
        } else {
            "adapter_error"
        })
        .to_string();

    let failed_detail = payload_json
        .get("error_message")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("tool {tool_name} failed with status={status}"));

    let status_value = if is_error {
        ToolCallStatus::Failed {
            code: failed_code,
            detail: failed_detail,
        }
    } else {
        ToolCallStatus::Succeeded
    };

    let llm_output = json!({
        "tool": tool_name,
        "ok": !is_error,
        "status": payload_json
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or(status),
        "result": payload_json,
    });

    ToolMappedReceipt {
        status: status_value,
        llm_output_json: serde_json::to_string(&llm_output)
            .unwrap_or_else(|_| String::from("{\"ok\":false,\"error\":\"encode_failed\"}")),
        is_error,
        runtime_delta: super::types::ToolRuntimeDelta {
            host_session_id: runtime_host_session_id,
            host_session_status: runtime_status_override,
        },
    }
}

fn parse_host_session_status(value: &str) -> Option<HostSessionStatus> {
    match value.trim().to_ascii_lowercase().as_str() {
        "ready" | "ok" => Some(HostSessionStatus::Ready),
        "closed" => Some(HostSessionStatus::Closed),
        "expired" => Some(HostSessionStatus::Expired),
        "error" | "failed" => Some(HostSessionStatus::Error),
        _ => None,
    }
}

pub(super) fn parse_json_object(
    arguments_json: &str,
) -> Result<Map<String, Value>, ToolMappingError> {
    let parsed = serde_json::from_str::<Value>(arguments_json)
        .map_err(|err| ToolMappingError::invalid_args(format!("arguments JSON invalid: {err}")))?;

    parsed
        .as_object()
        .cloned()
        .ok_or_else(|| ToolMappingError::invalid_args("arguments must be a JSON object"))
}

pub(super) fn require_string(
    args: &Map<String, Value>,
    field: &str,
) -> Result<String, ToolMappingError> {
    args.get(field)
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            ToolMappingError::invalid_args(format!("'{field}' must be a non-empty string"))
        })
}

pub(super) fn optional_string(args: &Map<String, Value>, field: &str) -> Option<String> {
    args.get(field)
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
}

pub(super) fn optional_u64(args: &Map<String, Value>, field: &str) -> Option<u64> {
    args.get(field).and_then(Value::as_u64)
}

pub(super) fn optional_bool(args: &Map<String, Value>, field: &str) -> Option<bool> {
    args.get(field).and_then(Value::as_bool)
}

pub(super) fn optional_object(
    args: &Map<String, Value>,
    field: &str,
) -> Option<Map<String, Value>> {
    args.get(field).and_then(Value::as_object).cloned()
}

pub(super) fn session_id_from_args_or_runtime(
    args: &Map<String, Value>,
    runtime: &ToolRuntimeContext,
) -> Result<String, ToolMappingError> {
    if let Some(session_id) = optional_string(args, "session_id") {
        return Ok(session_id);
    }

    runtime
        .host_session_id
        .clone()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(ToolMappingError::missing_session)
}

pub(super) fn value_object(map: Map<String, Value>) -> Value {
    Value::Object(map)
}
