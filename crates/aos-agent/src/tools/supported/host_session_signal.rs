use super::{
    optional_u64, parse_json_object, require_string, session_id_from_args_or_runtime, value_object,
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
    let signal = require_string(&args, "signal")?;

    let mut out = Map::new();
    out.insert("session_id".into(), Value::String(session_id));
    out.insert("signal".into(), Value::String(signal));
    if let Some(grace_timeout_ns) = optional_u64(&args, "grace_timeout_ns") {
        out.insert(
            "grace_timeout_ns".into(),
            Value::Number(grace_timeout_ns.into()),
        );
    }
    Ok(value_object(out))
}
