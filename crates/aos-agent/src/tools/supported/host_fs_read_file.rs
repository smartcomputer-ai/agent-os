use super::{
    optional_string, optional_u64, parse_json_object, require_string,
    session_id_from_args_or_runtime, value_object,
};
use crate::contracts::ToolRuntimeContext;
use crate::tools::types::ToolMappingError;
use serde_json::{Map, Value};

pub fn map_args(
    arguments_json: &str,
    runtime: &ToolRuntimeContext,
) -> Result<Value, ToolMappingError> {
    let args = parse_json_object(arguments_json)?;
    let session_id = session_id_from_args_or_runtime(&args, runtime)?;
    let path = require_string(&args, "path")?;

    let mut out = Map::new();
    out.insert("session_id".into(), Value::String(session_id));
    out.insert("path".into(), Value::String(path));
    if let Some(offset_bytes) = optional_u64(&args, "offset_bytes") {
        out.insert("offset_bytes".into(), Value::Number(offset_bytes.into()));
    }
    if let Some(max_bytes) = optional_u64(&args, "max_bytes") {
        out.insert("max_bytes".into(), Value::Number(max_bytes.into()));
    }
    if let Some(encoding) = optional_string(&args, "encoding") {
        out.insert("encoding".into(), Value::String(encoding));
    }
    if let Some(output_mode) = optional_string(&args, "output_mode") {
        out.insert("output_mode".into(), Value::String(output_mode));
    }
    Ok(value_object(out))
}
