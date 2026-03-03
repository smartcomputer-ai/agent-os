use super::{parse_json_object, require_string, session_id_from_args_or_runtime, value_object};
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
    Ok(value_object(out))
}
