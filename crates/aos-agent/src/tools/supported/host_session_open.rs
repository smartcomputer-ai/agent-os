use super::{optional_object, optional_u64, parse_json_object, value_object};
use crate::tools::types::ToolMappingError;
use alloc::string::ToString;
use serde_json::{Map, Value, json};

pub fn map_args(arguments_json: &str) -> Result<Value, ToolMappingError> {
    let args = parse_json_object(arguments_json)?;

    let target = if let Some(target) = optional_object(&args, "target") {
        Value::Object(target)
    } else {
        json!({ "local": { "network_mode": "off" } })
    };

    let labels = match args.get("labels").and_then(Value::as_object) {
        Some(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                if let Some(text) = v.as_str() {
                    out.insert(k.clone(), Value::String(text.to_string()));
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(Value::Object(out))
            }
        }
        None => None,
    };

    let mut out = Map::new();
    out.insert("target".into(), target);
    if let Some(ttl) = optional_u64(&args, "session_ttl_ns") {
        out.insert("session_ttl_ns".into(), Value::Number(ttl.into()));
    }
    if let Some(labels) = labels {
        out.insert("labels".into(), labels);
    }
    Ok(value_object(out))
}
