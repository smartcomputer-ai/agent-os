//! JSON schema helpers for tools and structured output.

use serde_json::Value;

/// Returns true when the schema root is an object schema.
pub fn is_object_schema(schema: &Value) -> bool {
    schema
        .get("type")
        .and_then(|value| value.as_str())
        .map(|value| value == "object")
        .unwrap_or(false)
}

/// Ensure the schema root is an object schema.
pub fn require_object_schema(schema: &Value) -> Result<(), SchemaError> {
    if is_object_schema(schema) {
        Ok(())
    } else {
        Err(SchemaError::RootNotObject)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    RootNotObject,
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaError::RootNotObject => write!(f, "schema root must be an object"),
        }
    }
}

impl std::error::Error for SchemaError {}
