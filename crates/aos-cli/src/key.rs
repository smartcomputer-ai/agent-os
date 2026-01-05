//! Key encoding helpers using reducer key_schema.

use anyhow::{Context, Result, anyhow, bail};
use aos_air_types::{
    DefSchema,
    plan_literals::SchemaIndex,
    value_normalize::{ValueNormalizeError, normalize_value_with_schema},
};
use aos_kernel::LoadedManifest;
use aos_store::FsStore;
use base64::Engine;
use serde_cbor::value::Value as CborValue;
use serde_json::Value as JsonValue;
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
    let loaded = load_manifest_for_keys(dirs)?;
    let cbor_value = resolve_key_value(overrides)?;
    encode_key_value_for_reducer(&loaded, reducer, cbor_value)
}

fn schema_index(schemas: &HashMap<aos_air_types::Name, DefSchema>) -> SchemaIndex {
    let mut map = HashMap::new();
    for (name, def) in schemas {
        map.insert(name.as_str().to_string(), def.ty.clone());
    }
    SchemaIndex::new(map)
}

fn load_manifest_for_keys(dirs: &ResolvedDirs) -> Result<LoadedManifest> {
    let store = Arc::new(FsStore::open(&dirs.store_root).context("open store")?);
    let Some(manifest_hash) =
        crate::util::latest_manifest_hash_from_journal(&dirs.store_root)? else {
            anyhow::bail!("no manifest found in journal; run `aos push` first");
        };
    aos_kernel::ManifestLoader::load_from_hash(store.as_ref(), manifest_hash)
        .context("load manifest for key encoding")
}

fn encode_key_value_for_reducer(
    loaded: &LoadedManifest,
    reducer: &str,
    cbor_value: CborValue,
) -> Result<Vec<u8>> {
    let module = loaded
        .modules
        .get(reducer)
        .ok_or_else(|| anyhow!("reducer '{}' not found in manifest", reducer))?;
    let key_schema = module.key_schema.as_ref().map(|s| s.as_str().to_string());

    let schemas = schema_index(&loaded.schemas);
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

fn resolve_key_value(overrides: &KeyOverrides) -> Result<CborValue> {
    if let Some(hex) = &overrides.hex {
        let trimmed = hex.trim_start_matches("0x").trim_start_matches("0X");
        let bytes = hex::decode(trimmed).context("decode hex key")?;
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

fn overrides_present(overrides: &KeyOverrides) -> bool {
    overrides.utf8.is_some()
        || overrides.json.is_some()
        || overrides.hex.is_some()
        || overrides.b64.is_some()
}

/// Derive key bytes for an event using manifest routing and payload, with override escape hatches.
///
/// Returns `Ok(Some(key_bytes))` when a keyed route is found (or overrides are provided),
/// `Ok(None)` when the target route is unkeyed or missing a key_field, and errors on
/// missing routing/fields when a key is required.
pub fn derive_event_key(
    dirs: &ResolvedDirs,
    event_schema: &str,
    payload_json: &JsonValue,
    overrides: &KeyOverrides,
) -> Result<Option<Vec<u8>>> {
    let loaded = load_manifest_for_keys(dirs)?;
    let route = loaded.manifest.routing.as_ref().and_then(|r| {
        r.events
            .iter()
            .find(|evt| evt.event.as_str() == event_schema)
    });

    let route = if overrides_present(overrides) {
        route.ok_or_else(|| {
            anyhow!(
                "no routing entry for event '{}' (needed for key overrides)",
                event_schema
            )
        })?
    } else if let Some(r) = route {
        r
    } else {
        // No routing entry and no overrides; nothing to derive.
        return Ok(None);
    };

    let reducer = route.reducer.as_str();
    // If the route isn't keyed, we don't derive.
    let key_field = match &route.key_field {
        Some(field) => field,
        None => {
            if overrides_present(overrides) {
                bail!(
                    "reducer '{}' is not keyed; --key overrides are not allowed",
                    reducer
                );
            }
            return Ok(None);
        }
    };

    // Determine the CBOR value to encode: override wins, otherwise extract from payload.
    let cbor_value = if overrides_present(overrides) {
        resolve_key_value(overrides)?
    } else {
        let extracted = extract_json_path(payload_json, key_field)
            .ok_or_else(|| anyhow!("event '{}' missing key field '{}'", event_schema, key_field))?;
        json_to_cbor(extracted.clone())?
    };

    let key_bytes = encode_key_value_for_reducer(&loaded, reducer, cbor_value)?;
    Ok(Some(key_bytes))
}

fn extract_json_path<'a>(value: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    let mut current = value;
    for segment in path.split('.') {
        match current {
            JsonValue::Object(map) => {
                current = map.get(segment)?;
            }
            JsonValue::Array(arr) => {
                let idx: usize = segment.parse().ok()?;
                current = arr.get(idx)?;
            }
            _ => return None,
        }
    }
    Some(current)
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

#[cfg(test)]
mod tests {
    use super::*;
    use aos_host::config::HostConfig;
    use aos_host::host::WorldHost;
    use aos_host::manifest_loader;
    use aos_kernel::KernelConfig;
    use aos_store::FsStore;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    const ZERO_HASH: &str =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn derives_key_from_payload_via_routing() {
        let (_tmp, dirs) = build_test_world();
        let payload = json!({
            "$tag": "Payload",
            "$value": { "id": "abc", "payload": "x" }
        });
        let key = derive_event_key(
            &dirs,
            "com.acme/Event@1",
            &payload,
            &KeyOverrides::default(),
        )
        .unwrap()
        .expect("key derived");

        let expected = aos_cbor::to_canonical_cbor(&CborValue::Text("abc".into())).expect("encode");
        assert_eq!(key, expected);
    }

    #[test]
    fn override_key_wins_over_payload() {
        let (_tmp, dirs) = build_test_world();
        let payload = json!({ "$tag": "Payload", "$value": { "id": "abc" } });
        let overrides = KeyOverrides {
            utf8: Some("override".into()),
            ..Default::default()
        };
        let key = derive_event_key(&dirs, "com.acme/Event@1", &payload, &overrides)
            .unwrap()
            .expect("key derived");

        let expected =
            aos_cbor::to_canonical_cbor(&CborValue::Text("override".into())).expect("encode");
        assert_eq!(key, expected);
    }

    #[test]
    fn error_when_payload_missing_key_field() {
        let (_tmp, dirs) = build_test_world();
        let payload = json!({ "$tag": "Payload", "$value": { "not_id": "abc" } });
        let err = derive_event_key(
            &dirs,
            "com.acme/Event@1",
            &payload,
            &KeyOverrides::default(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("missing key field"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn override_requires_route() {
        let (_tmp, dirs) = build_test_world_without_routing();
        let overrides = KeyOverrides {
            utf8: Some("abc".into()),
            ..Default::default()
        };
        let payload = json!({ "$tag": "Payload", "$value": { "id": "abc" } });
        let err = derive_event_key(&dirs, "com.acme/Event@1", &payload, &overrides).unwrap_err();
        assert!(
            err.to_string().contains("no routing entry"),
            "unexpected error: {err}"
        );
    }

    fn build_test_world() -> (TempDir, ResolvedDirs) {
        let tmp = TempDir::new().expect("tmpdir");
        let world = tmp.path();
        let air_dir = world.join("air");
        let store_root = world.join(".aos");
        fs::create_dir_all(&air_dir).unwrap();
        fs::create_dir_all(&store_root).unwrap();

        write_manifest(&air_dir);
        write_defs(&air_dir);
        write_modules(&air_dir);

        let dirs = ResolvedDirs {
            world: world.to_path_buf(),
            air_dir,
            reducer_dir: world.join("reducer"),
            store_root: store_root.clone(),
            control_socket: store_root.join("control.sock"),
        };
        seed_world(&dirs);
        (tmp, dirs)
    }

    fn build_test_world_without_routing() -> (TempDir, ResolvedDirs) {
        let (tmp, dirs) = build_test_world();
        // Overwrite manifest without routing to exercise error path.
        fs::write(
            dirs.air_dir.join("manifest.air.json"),
            r#"{
  "$kind":"manifest",
  "air_version":"1",
  "schemas": [
    { "name": "com.acme/Key@1", "hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000" },
    { "name": "com.acme/State@1", "hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000" },
    { "name": "com.acme/EventPayload@1", "hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000" },
    { "name": "com.acme/Event@1", "hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000" }
  ],
  "modules": [ { "name": "com.acme/Reducer@1", "hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000" } ],
  "plans": [],
  "effects": [],
  "caps": [],
  "policies": [],
  "secrets": [],
  "routing": { "events": [], "inboxes": [] },
  "triggers": [],
  "defaults": null
}"#,
        )
        .unwrap();
        reset_journal(&dirs.store_root);
        seed_world(&dirs);
        (tmp, dirs)
    }

    fn seed_world(dirs: &ResolvedDirs) {
        let store = Arc::new(FsStore::open(&dirs.store_root).expect("open store"));
        let assets = manifest_loader::load_from_assets_with_defs(store.clone(), &dirs.air_dir)
            .expect("load assets")
            .expect("manifest assets");
        let mut host = WorldHost::from_loaded_manifest(
            store,
            assets.loaded,
            &dirs.store_root,
            HostConfig {
                allow_placeholder_secrets: true,
                ..HostConfig::default()
            },
            KernelConfig {
                allow_placeholder_secrets: true,
                ..KernelConfig::default()
            },
        )
        .expect("create host");
        host.kernel_mut().create_snapshot().expect("snapshot");
    }

    fn reset_journal(store_root: &PathBuf) {
        let journal = store_root.join(".aos/journal/journal.log");
        let _ = fs::remove_file(journal);
    }

    fn write_manifest(air_dir: &PathBuf) {
        let manifest = format!(
            r#"{{
  "$kind":"manifest",
  "air_version":"1",
  "schemas": [
    {{ "name":"com.acme/Key@1", "hash":"{zero}" }},
    {{ "name":"com.acme/State@1", "hash":"{zero}" }},
    {{ "name":"com.acme/EventPayload@1", "hash":"{zero}" }},
    {{ "name":"com.acme/Event@1", "hash":"{zero}" }}
  ],
  "modules": [ {{ "name":"com.acme/Reducer@1", "hash":"{zero}" }} ],
  "plans": [],
  "effects": [],
  "caps": [],
  "policies": [],
  "secrets": [],
  "routing": {{
    "events": [ {{ "event":"com.acme/Event@1", "reducer":"com.acme/Reducer@1", "key_field":"$value.id" }} ],
    "inboxes": []
  }},
  "triggers": [],
  "defaults": null
}}"#,
            zero = ZERO_HASH
        );
        fs::write(air_dir.join("manifest.air.json"), manifest).unwrap();
    }

    fn write_defs(air_dir: &PathBuf) {
        let defs = r#"[
  { "$kind":"defschema", "name":"com.acme/Key@1", "type": { "text": {} } },
  { "$kind":"defschema", "name":"com.acme/State@1", "type": { "bool": {} } },
  { "$kind":"defschema", "name":"com.acme/EventPayload@1", "type": { "record": { "id": { "text": {} }, "payload": { "text": {} } } } },
  { "$kind":"defschema", "name":"com.acme/Event@1", "type": { "variant": { "Payload": { "ref": "com.acme/EventPayload@1" } } } }
]"#;
        fs::write(air_dir.join("defs.schema.json"), defs).unwrap();
    }

    fn write_modules(air_dir: &PathBuf) {
        let modules = format!(
            r#"[
  {{
    "$kind":"defmodule",
    "name":"com.acme/Reducer@1",
    "module_kind":"reducer",
    "wasm_hash":"{zero}",
    "key_schema":"com.acme/Key@1",
    "abi": {{
      "reducer": {{
        "state":"com.acme/State@1",
        "event":"com.acme/Event@1",
        "effects_emitted": [],
        "cap_slots": {{}}
      }}
    }}
  }}
]"#,
            zero = ZERO_HASH
        );
        fs::write(air_dir.join("defs.module.json"), modules).unwrap();
    }
}
