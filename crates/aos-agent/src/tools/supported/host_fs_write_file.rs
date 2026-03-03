use super::{
    optional_bool, optional_string, parse_json_object, require_string,
    session_id_from_args_or_runtime, value_object,
};
use crate::contracts::ToolRuntimeContext;
use crate::tools::types::ToolMappingError;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde_json::{Map, Value};

pub fn map_args(
    arguments_json: &str,
    runtime: &ToolRuntimeContext,
) -> Result<Value, ToolMappingError> {
    let args = parse_json_object(arguments_json)?;
    let session_id = session_id_from_args_or_runtime(&args, runtime)?;
    let path = require_string(&args, "path")?;

    let content = map_content(&args)?;

    let mut out = Map::new();
    out.insert("session_id".into(), Value::String(session_id));
    out.insert("path".into(), Value::String(path));
    out.insert("content".into(), content);

    if let Some(create_parents) = optional_bool(&args, "create_parents") {
        out.insert("create_parents".into(), Value::Bool(create_parents));
    }
    if let Some(mode) = optional_string(&args, "mode") {
        out.insert("mode".into(), Value::String(mode));
    }

    Ok(value_object(out))
}

fn map_content(args: &Map<String, Value>) -> Result<Value, ToolMappingError> {
    if let Some(text) = args.get("text").and_then(Value::as_str) {
        return Ok(Value::Object(Map::from_iter([(
            "inline_text".into(),
            Value::Object(Map::from_iter([(
                "text".into(),
                Value::String(text.to_string()),
            )])),
        )])));
    }

    if let Some(blob_ref) = args.get("blob_ref").and_then(Value::as_str) {
        return Ok(Value::Object(Map::from_iter([(
            "blob_ref".into(),
            Value::Object(Map::from_iter([(
                "blob_ref".into(),
                decode_hash_ref(blob_ref),
            )])),
        )])));
    }

    Err(ToolMappingError::invalid_args(
        "host.fs.write_file requires one of: text, blob_ref",
    ))
}

fn decode_hash_ref(value: &str) -> Value {
    Value::Object(Map::from_iter([
        ("algorithm".into(), Value::String("sha256".into())),
        (
            "digest".into(),
            Value::Array(
                decode_hash_hex_bytes(value)
                    .into_iter()
                    .map(|byte| Value::Number((byte as u64).into()))
                    .collect(),
            ),
        ),
    ]))
}

fn decode_hash_hex_bytes(value: &str) -> Vec<u8> {
    let mut trimmed = value.trim();
    if let Some(rest) = trimmed.strip_prefix("sha256:") {
        trimmed = rest;
    }

    let bytes = trimmed.as_bytes();
    if bytes.len() % 2 != 0 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(bytes.len() / 2);
    for idx in (0..bytes.len()).step_by(2) {
        let hi = from_hex(bytes[idx]);
        let lo = from_hex(bytes[idx + 1]);
        match (hi, lo) {
            (Some(hi), Some(lo)) => out.push((hi << 4) | lo),
            _ => return Vec::new(),
        }
    }
    out
}

const fn from_hex(ch: u8) -> Option<u8> {
    match ch {
        b'0'..=b'9' => Some(ch - b'0'),
        b'a'..=b'f' => Some(ch - b'a' + 10),
        b'A'..=b'F' => Some(ch - b'A' + 10),
        _ => None,
    }
}
