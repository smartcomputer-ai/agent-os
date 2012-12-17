//! Key encoding helpers using reducer key_schema.

use anyhow::{Context, Result, anyhow, bail};
use aos_air_types::{
    DefSchema,
    plan_literals::SchemaIndex,
    value_normalize::{normalize_value_with_schema, ValueNormalizeError},
};
use aos_store::FsStore;
use base64::Engine;
use serde_json::Value as JsonValue;
use serde_cbor::value::Value as CborValue;
use std::{collections::HashMap, sync::Arc};

use crate::opts::ResolvedDirs;

#[derive(Debug, Default)]
pub struct KeyOverrides {
    pub utf8: Option<String>,
    pub json: Option<String>,
    pub hex: Option<String>,
    pub b64: Option<String>,
}

/// Encode key bytes for a reducer using its key_schema and user overrides.
pub fn encode_key_for_reducer(
    dirs: &ResolvedDirs,
    reducer: &str,
    overrides: &KeyOverrides,
) -> Result<Vec<u8>> {
    let store = Arc::new(FsStore::open(&dirs.store_root).context("open store")?);
    let loaded = aos_host::manifest_loader::load_from_assets(store, &dirs.air_dir)
        .context("load manifest for key encoding")?
        .ok_or_else(|| anyhow!("no manifest found in {}", dirs.air_dir.display()))?;

    let module = loaded
        .modules
        .get(reducer)
        .ok_or_else(|| anyhow!("reducer '{}' not found in manifest", reducer))?;
    let key_schema = module
        .key_schema
        .as_ref()
        .map(|s| s.as_str().to_string());

    let schemas = schema_index(&loaded.schemas);
    let cbor_value = resolve_key_value(overrides)?;
    if let Some(schema_name) = key_schema {
        let schema = schemas
            .get(schema_name.as_str())
            .ok_or_else(|| anyhow!("key schema '{}' not found", schema_name))?;
        normalize_value_with_schema(cbor_value, schema, &schemas)
            .map(|n| n.bytes)
            .map_err(|e| anyhow!("key failed validation: {}", normalize_err(e)))
    } else {
        // No schema: encode as canonical CBOR directly.
        aos_cbor::to_canonical_cbor(&cbor_value).context("encode key as canonical CBOR")
    }
}

fn schema_index(schemas: &HashMap<aos_air_types::Name, DefSchema>) -> SchemaIndex {
    let mut map = HashMap::new();
    for (name, def) in schemas {
        map.insert(name.as_str().to_string(), def.ty.clone());
    }
    SchemaIndex::new(map)
}

fn resolve_key_value(overrides: &KeyOverrides) -> Result<CborValue> {
    if let Some(hex) = &overrides.hex {
        let bytes = hex::decode(hex).context("decode hex key")?;
        return Ok(CborValue::Bytes(bytes));
    }
    if let Some(b64) = &overrides.b64 {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("decode base64 key")?;
        return Ok(CborValue::Bytes(bytes));
    }
    if let Some(json_str) = &overrides.json {
        let json: JsonValue = serde_json::from_str(json_str).context("parse --key-json value")?;
        return json_to_cbor(json);
    }
    if let Some(utf8) = &overrides.utf8 {
        return json_to_cbor(JsonValue::String(utf8.clone()));
    }
    bail!("key is required for keyed reducer but no --key/--key-json/--key-hex/--key-b64 provided")
}

fn json_to_cbor(json: JsonValue) -> Result<CborValue> {
    serde_cbor::value::to_value(json).context("convert key to CBOR")
}

fn normalize_err(err: ValueNormalizeError) -> String {
    match err {
        ValueNormalizeError::SchemaNotFound(s) => format!("schema not found: {s}"),
        ValueNormalizeError::Decode(s) => format!("decode error: {s}"),
        ValueNormalizeError::Invalid(s) => format!("invalid value: {s}"),
        ValueNormalizeError::Encode(s) => format!("encode error: {s}"),
    }
}
