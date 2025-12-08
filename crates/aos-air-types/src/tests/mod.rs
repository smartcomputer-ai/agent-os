use crate::schemas::COMMON;
use jsonschema::{JSONSchema, paths::JSONPointer};
use once_cell::sync::Lazy;
use serde_json::Value;

pub mod caps;
pub mod effects;
pub mod manifest;
pub mod modules;
pub mod patch;
pub mod plans;
pub mod policies;
pub mod schemas;

static COMMON_SCHEMA: Lazy<Value> =
    Lazy::new(|| serde_json::from_str(COMMON).expect("embedded common schema must be valid JSON"));

pub(crate) fn assert_json_schema(schema_json: &str, instance: &Value) {
    let schema_value: Value =
        serde_json::from_str(schema_json).expect("embedded schema must be valid JSON");
    let mut options = JSONSchema::options();
    for id in [
        "common.schema.json",
        "https://aos.dev/air/v1/common.schema.json",
    ] {
        options.with_document(id.to_string(), COMMON_SCHEMA.clone());
    }
    let compiled = options
        .compile(&schema_value)
        .expect("embedded schema must compile successfully");
    if let Err(errors) = compiled.validate(instance) {
        let mut messages = Vec::new();
        for err in errors {
            messages.push(format!("{}: {}", format_pointer(&err.instance_path), err));
        }
        panic!(
            "schema validation failed: {}\ninstance: {}",
            messages.join("; "),
            instance
        );
    }
}

fn format_pointer(pointer: &JSONPointer) -> String {
    let text = pointer.to_string();
    if text.is_empty() { "/".into() } else { text }
}
