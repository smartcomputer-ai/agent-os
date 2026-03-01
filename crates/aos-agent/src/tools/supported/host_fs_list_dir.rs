use super::{
    optional_string, optional_u64, parse_json_object, session_id_from_args_or_runtime, value_object,
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

    let mut out = Map::new();
    out.insert("session_id".into(), Value::String(session_id));

    if let Some(path) = optional_string(&args, "path") {
        out.insert("path".into(), Value::String(path));
    }
    if let Some(max_results) = optional_u64(&args, "max_results") {
        out.insert("max_results".into(), Value::Number(max_results.into()));
    }
    if let Some(output_mode) = optional_string(&args, "output_mode") {
        out.insert("output_mode".into(), Value::String(output_mode));
    }

    Ok(value_object(out))
}
