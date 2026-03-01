use super::{
    optional_object, optional_string, optional_u64, parse_json_object,
    session_id_from_args_or_runtime, value_object,
};
use crate::contracts::ToolRuntimeContext;
use crate::tools::types::ToolMappingError;
use alloc::string::ToString;
use alloc::vec::Vec;
use serde_json::{Map, Value};

pub fn map_args(
    arguments_json: &str,
    runtime: &ToolRuntimeContext,
) -> Result<Value, ToolMappingError> {
    let args = parse_json_object(arguments_json)?;
    let session_id = session_id_from_args_or_runtime(&args, runtime)?;

    let argv = args
        .get("argv")
        .and_then(Value::as_array)
        .ok_or_else(|| ToolMappingError::invalid_args("'argv' must be an array of strings"))?;

    let mut argv_out = Vec::new();
    for item in argv {
        let Some(text) = item.as_str() else {
            return Err(ToolMappingError::invalid_args(
                "'argv' entries must be strings",
            ));
        };
        argv_out.push(Value::String(text.to_string()));
    }
    if argv_out.is_empty() {
        return Err(ToolMappingError::invalid_args("'argv' must not be empty"));
    }

    let mut out = Map::new();
    out.insert("session_id".into(), Value::String(session_id));
    out.insert("argv".into(), Value::Array(argv_out));

    if let Some(cwd) = optional_string(&args, "cwd") {
        out.insert("cwd".into(), Value::String(cwd));
    }
    if let Some(timeout_ns) = optional_u64(&args, "timeout_ns") {
        out.insert("timeout_ns".into(), Value::Number(timeout_ns.into()));
    }
    if let Some(output_mode) = optional_string(&args, "output_mode") {
        out.insert("output_mode".into(), Value::String(output_mode));
    }
    if let Some(stdin_ref) = optional_string(&args, "stdin_ref") {
        out.insert(
            "stdin_ref".into(),
            Value::Object(Map::from_iter([
                ("algorithm".into(), Value::String("sha256".into())),
                (
                    "digest".into(),
                    Value::Array(
                        decode_hash_hex_bytes(stdin_ref.as_str())
                            .into_iter()
                            .map(|byte| Value::Number((byte as u64).into()))
                            .collect(),
                    ),
                ),
            ])),
        );
    }

    if let Some(env_patch) = optional_object(&args, "env_patch") {
        let mut env_out = Map::new();
        for (k, v) in env_patch {
            if let Some(text) = v.as_str() {
                env_out.insert(k, Value::String(text.to_string()));
            }
        }
        if !env_out.is_empty() {
            out.insert("env_patch".into(), Value::Object(env_out));
        }
    }

    Ok(value_object(out))
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
