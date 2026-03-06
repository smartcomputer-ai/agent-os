use super::{
    optional_string, optional_u64, parse_json_object, require_string,
    session_id_from_args_or_runtime, value_object,
};
use crate::contracts::ToolRuntimeContext;
use crate::tools::types::{ToolMappedArgs, ToolMappingError};
use serde_json::{Map, Value};

pub fn map_args(
    arguments_json: &str,
    runtime: &ToolRuntimeContext,
) -> Result<ToolMappedArgs, ToolMappingError> {
    let args = parse_json_object(arguments_json)?;
    let session_id = session_id_from_args_or_runtime(&args, runtime)?;
    let pattern = require_string(&args, "pattern")?;

    let mut out = Map::new();
    out.insert("session_id".into(), Value::String(session_id));
    out.insert("pattern".into(), Value::String(pattern));

    if let Some(path) = optional_string(&args, "path") {
        out.insert("path".into(), Value::String(path));
    }
    if let Some(max_results) = optional_u64(&args, "max_results") {
        out.insert("max_results".into(), Value::Number(max_results.into()));
    }

    Ok(ToolMappedArgs::params(value_object(out)))
}
