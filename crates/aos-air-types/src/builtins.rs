use std::collections::HashMap;

use aos_cbor::Hash;
use once_cell::sync::Lazy;
use serde_json;

use crate::{DefSchema, HashRef};

static BUILTIN_SCHEMAS_RAW: &str = include_str!("../../../spec/defs/builtin-schemas.air.json");

#[derive(Debug)]
pub struct BuiltinSchema {
    pub schema: DefSchema,
    pub hash: Hash,
    pub hash_ref: HashRef,
}

static BUILTIN_SCHEMAS: Lazy<Vec<BuiltinSchema>> = Lazy::new(|| {
    let defs: Vec<DefSchema> = serde_json::from_str(BUILTIN_SCHEMAS_RAW)
        .expect("spec/defs/builtin-schemas.air.json must parse");
    defs.into_iter()
        .map(|schema| {
            let hash = Hash::of_cbor(&schema).expect("canonical hash");
            let hash_ref = HashRef::new(hash.to_hex()).expect("valid hash");
            BuiltinSchema {
                schema,
                hash,
                hash_ref,
            }
        })
        .collect()
});

static BUILTIN_SCHEMA_INDEX: Lazy<HashMap<String, usize>> = Lazy::new(|| {
    BUILTIN_SCHEMAS
        .iter()
        .enumerate()
        .map(|(idx, schema)| (schema.schema.name.clone(), idx))
        .collect()
});

/// Returns the parsed list of built-in `defschema` nodes (timer/blob params, receipts, and events).
pub fn builtin_schemas() -> &'static [BuiltinSchema] {
    &BUILTIN_SCHEMAS
}

/// Finds a built-in schema definition by name (e.g., `sys/TimerFired@1`).
pub fn find_builtin_schema(name: &str) -> Option<&'static BuiltinSchema> {
    BUILTIN_SCHEMA_INDEX
        .get(name)
        .and_then(|idx| BUILTIN_SCHEMAS.get(*idx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_expected_schema_names() {
        let names: Vec<_> = builtin_schemas()
            .iter()
            .map(|s| s.schema.name.as_str())
            .collect();
        assert!(names.contains(&"sys/TimerFired@1"));
        assert!(names.contains(&"sys/BlobPutResult@1"));
    }

    #[test]
    fn lookup_returns_same_instance() {
        let timer = find_builtin_schema("sys/TimerSetParams@1").expect("timer params");
        assert_eq!(timer.schema.name.as_str(), "sys/TimerSetParams@1");
    }
}
