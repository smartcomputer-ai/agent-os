use super::{
    optional_bool, optional_string, parse_json_object, require_string,
    session_id_from_args_or_runtime, value_object,
};
use crate::contracts::ToolRuntimeContext;
use crate::tools::types::ToolMappingError;
use alloc::string::ToString;
use serde_json::{Map, Value};

pub fn map_args(
    arguments_json: &str,
    runtime: &ToolRuntimeContext,
) -> Result<Value, ToolMappingError> {
    let args = parse_json_object(arguments_json)?;
    let session_id = session_id_from_args_or_runtime(&args, runtime)?;
    let patch_text = require_string(&args, "patch")?;

    let mut out = Map::new();
    out.insert("session_id".into(), Value::String(session_id));
    out.insert(
        "patch".into(),
        Value::Object(Map::from_iter([(
            "inline_text".into(),
            Value::Object(Map::from_iter([(
                "text".into(),
                Value::String(patch_text.to_string()),
            )])),
        )])),
    );

    if let Some(patch_format) = optional_string(&args, "patch_format") {
        out.insert("patch_format".into(), Value::String(patch_format));
    }
    if let Some(dry_run) = optional_bool(&args, "dry_run") {
        out.insert("dry_run".into(), Value::Bool(dry_run));
    }

    Ok(value_object(out))
}
