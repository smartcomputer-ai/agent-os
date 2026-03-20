use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde_cbor::Value as CborValue;
use serde_json::{Value, json};

use crate::output::{OutputOpts, print_success};

pub(crate) fn print_state_cells(output: OutputOpts, data: Value) -> Result<()> {
    print_with_renderer(output, data, render_state_cells_value)
}

pub(crate) fn print_state_get(output: OutputOpts, data: Value, expand: bool) -> Result<()> {
    if output.json && !output.pretty {
        if expand {
            return print_success(output, augment_state_get_expanded(data)?, None, vec![]);
        }
        return print_success(output, data, None, vec![]);
    }

    print_success(output, render_state_get_value(data)?, None, vec![])
}

pub(crate) fn print_journal_entries(output: OutputOpts, data: Value) -> Result<()> {
    print_with_renderer(output, data, render_journal_entries_value)
}

pub(crate) fn print_trace(output: OutputOpts, data: Value) -> Result<()> {
    print_with_renderer(output, data, |value| Ok(render_json_value(value)))
}

pub(crate) fn print_trace_summary(output: OutputOpts, data: Value) -> Result<()> {
    print_with_renderer(output, data, |value| Ok(render_json_value(value)))
}

pub(crate) fn decode_workspace_key_bytes(cell: &Value) -> Result<Vec<u8>> {
    if let Some(key_b64) = cell.get("key_b64").and_then(Value::as_str) {
        return BASE64_STANDARD
            .decode(key_b64)
            .with_context(|| format!("decode workspace key '{key_b64}'"));
    }
    if let Some(items) = cell.get("key_bytes").and_then(Value::as_array) {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let value = item
                .as_u64()
                .ok_or_else(|| anyhow!("workspace key_bytes entry is not an integer"))?;
            let byte = u8::try_from(value)
                .map_err(|_| anyhow!("workspace key_bytes entry out of range: {value}"))?;
            out.push(byte);
        }
        return Ok(out);
    }
    Ok(Vec::new())
}

pub(crate) fn decode_payload_display_value(bytes: &[u8]) -> Value {
    if let Ok(value) = serde_cbor::from_slice::<CborValue>(bytes) {
        return cbor_value_to_json(value);
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            return value;
        }
        return Value::String(text.to_string());
    }
    Value::String(BASE64_STANDARD.encode(bytes))
}

fn print_with_renderer<F>(output: OutputOpts, data: Value, render: F) -> Result<()>
where
    F: FnOnce(Value) -> Result<Value>,
{
    if output.json && !output.pretty {
        return print_success(output, data, None, vec![]);
    }
    print_success(output, render(data)?, None, vec![])
}

fn render_state_cells_value(data: Value) -> Result<Value> {
    let workflow = data
        .get("workflow")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let journal_head = data
        .get("journal_head")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let cells = data
        .get("cells")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let formatted = cells
        .into_iter()
        .map(|cell| {
            let mut entry = serde_json::Map::new();
            entry.insert("key".into(), decode_cbor_key_display_value(&cell));
            if let Some(size) = cell.get("size").and_then(Value::as_u64) {
                entry.insert("size".into(), json!(size));
            }
            if let Some(last_active_ns) = cell.get("last_active_ns").and_then(Value::as_u64) {
                entry.insert("last_active_ns".into(), json!(last_active_ns));
            }
            if let Some(state_hash) = cell.get("state_hash").cloned() {
                entry.insert("state_hash".into(), state_hash);
            }
            Value::Object(entry)
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "workflow": workflow,
        "journal_head": journal_head,
        "cells": formatted,
    }))
}

fn render_state_get_value(data: Value) -> Result<Value> {
    let workflow = data
        .get("workflow")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>")
        .to_string();
    let journal_head = data
        .get("journal_head")
        .and_then(Value::as_u64)
        .unwrap_or_default();

    let mut out = serde_json::Map::new();
    out.insert("workflow".into(), Value::String(workflow));
    out.insert("journal_head".into(), json!(journal_head));

    if let Some(cell) = data.get("cell").cloned() {
        let mut entry = serde_json::Map::new();
        entry.insert("key".into(), decode_cbor_key_display_value(&cell));
        if let Some(size) = cell.get("size").and_then(Value::as_u64) {
            entry.insert("size".into(), json!(size));
        }
        if let Some(last_active_ns) = cell.get("last_active_ns").and_then(Value::as_u64) {
            entry.insert("last_active_ns".into(), json!(last_active_ns));
        }
        if let Some(state_hash) = cell.get("state_hash").cloned() {
            entry.insert("state_hash".into(), state_hash);
        }
        out.insert("cell".into(), Value::Object(entry));
    }

    if let Some(state_b64) = data.get("state_b64").and_then(Value::as_str) {
        let bytes = BASE64_STANDARD
            .decode(state_b64)
            .with_context(|| format!("decode state payload '{state_b64}'"))?;
        out.insert("state".into(), decode_payload_display_value(&bytes));
    }

    Ok(Value::Object(out))
}

fn augment_state_get_expanded(data: Value) -> Result<Value> {
    let mut out = match data {
        Value::Object(map) => map,
        other => return Ok(other),
    };
    if let Some(state_b64) = out.get("state_b64").and_then(Value::as_str) {
        let bytes = BASE64_STANDARD
            .decode(state_b64)
            .with_context(|| format!("decode state payload '{state_b64}'"))?;
        out.insert(
            "state_expanded".into(),
            decode_payload_display_value(&bytes),
        );
    }
    Ok(Value::Object(out))
}

fn render_journal_entries_value(data: Value) -> Result<Value> {
    let mut out = match data {
        Value::Object(map) => map,
        other => return Ok(render_json_value(other)),
    };
    if let Some(entries) = out.get_mut("entries").and_then(Value::as_array_mut) {
        for entry in entries.iter_mut() {
            let rendered = render_json_value(entry.take());
            *entry = rendered;
        }
    }
    Ok(Value::Object(out))
}

fn render_json_value(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(render_json_value).collect()),
        Value::Object(map) => render_object(map),
        other => other,
    }
}

fn render_object(map: serde_json::Map<String, Value>) -> Value {
    let mut out = serde_json::Map::new();
    for (key, value) in map {
        match key.as_str() {
            "key_bytes" => {
                if let Some(bytes) = decode_byte_array(&value) {
                    out.insert("key".into(), decode_payload_display_value(&bytes));
                }
            }
            "key_b64" => {
                if let Some(bytes) = decode_b64_string(&value) {
                    out.insert("key".into(), decode_payload_display_value(&bytes));
                } else {
                    out.insert(key, value);
                }
            }
            "value" => {
                if let Some(bytes) = decode_byte_array(&value) {
                    out.insert(key, decode_payload_display_value(&bytes));
                } else {
                    out.insert(key, render_json_value(value));
                }
            }
            "payload_b64" => {
                if let Some(bytes) = decode_b64_string(&value) {
                    out.insert("payload".into(), decode_payload_display_value(&bytes));
                } else {
                    out.insert(key, value);
                }
            }
            "entry_cbor" => {
                if let Some(bytes) = decode_byte_array(&value) {
                    out.insert("entry".into(), decode_payload_display_value(&bytes));
                } else {
                    out.insert(key, value);
                }
            }
            "entropy" | "key_hash" | "intent_hash" | "signature" => {
                if let Some(bytes) = decode_byte_array(&value) {
                    out.insert(key, Value::String(hex_encode(&bytes)));
                } else {
                    out.insert(key, render_json_value(value));
                }
            }
            _ => {
                if let Some(bytes) = decode_byte_array(&value) {
                    out.insert(key, Value::String(BASE64_STANDARD.encode(bytes)));
                } else {
                    out.insert(key, render_json_value(value));
                }
            }
        }
    }
    Value::Object(out)
}

fn decode_byte_array(value: &Value) -> Option<Vec<u8>> {
    let items = value.as_array()?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let value = item.as_u64()?;
        let byte = u8::try_from(value).ok()?;
        out.push(byte);
    }
    Some(out)
}

fn decode_b64_string(value: &Value) -> Option<Vec<u8>> {
    let raw = value.as_str()?;
    BASE64_STANDARD.decode(raw).ok()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_cbor_key_display_value(cell: &Value) -> Value {
    match decode_workspace_key_bytes(cell) {
        Ok(bytes) if bytes.is_empty() => Value::Null,
        Ok(bytes) => decode_payload_display_value(&bytes),
        Err(_) => Value::Null,
    }
}

fn cbor_value_to_json(value: CborValue) -> Value {
    match value {
        CborValue::Null => Value::Null,
        CborValue::Bool(v) => Value::Bool(v),
        CborValue::Integer(v) => json!(v),
        CborValue::Float(v) => json!(v),
        CborValue::Bytes(bytes) => Value::String(BASE64_STANDARD.encode(bytes)),
        CborValue::Text(text) => Value::String(text),
        CborValue::Array(items) => {
            Value::Array(items.into_iter().map(cbor_value_to_json).collect())
        }
        CborValue::Map(entries) => {
            let mut out = serde_json::Map::new();
            for (key, value) in entries {
                out.insert(cbor_key_to_string(key), cbor_value_to_json(value));
            }
            Value::Object(out)
        }
        CborValue::Tag(_, inner) => cbor_value_to_json(*inner),
        _ => Value::String(format!("{value:?}")),
    }
}

fn cbor_key_to_string(value: CborValue) -> String {
    match value {
        CborValue::Text(text) => text,
        CborValue::Integer(v) => v.to_string(),
        CborValue::Bool(v) => v.to_string(),
        CborValue::Bytes(bytes) => BASE64_STANDARD.encode(bytes),
        other => serde_json::to_string(&cbor_value_to_json(other))
            .unwrap_or_else(|_| "<invalid-key>".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_state_get_value_decodes_key_and_state() {
        let key = aos_cbor::to_canonical_cbor(&"workflow").expect("encode key");
        let state = aos_cbor::to_canonical_cbor(&json!({ "latest": 1 })).expect("encode state");
        let data = json!({
            "workflow": "sys/Workspace@1",
            "journal_head": 3,
            "cell": {
                "key_bytes": key,
                "size": state.len(),
                "last_active_ns": 3,
                "state_hash": "sha256:abc"
            },
            "state_b64": BASE64_STANDARD.encode(state),
        });
        let formatted = render_state_get_value(data).expect("format state");
        assert_eq!(
            formatted.get("workflow").and_then(Value::as_str),
            Some("sys/Workspace@1")
        );
        assert_eq!(
            formatted
                .get("state")
                .and_then(|v| v.get("latest"))
                .and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            formatted
                .get("cell")
                .and_then(|v| v.get("key"))
                .and_then(Value::as_str),
            Some("workflow")
        );
    }

    #[test]
    fn augment_state_get_expanded_adds_decoded_state() {
        let state = aos_cbor::to_canonical_cbor(&json!({ "latest": 1 })).expect("encode state");
        let data = json!({
            "workflow": "sys/Workspace@1",
            "journal_head": 3,
            "state_b64": BASE64_STANDARD.encode(state),
        });
        let formatted = augment_state_get_expanded(data).expect("format state");
        assert_eq!(
            formatted
                .get("state_expanded")
                .and_then(|v| v.get("latest"))
                .and_then(Value::as_u64),
            Some(1)
        );
    }

    #[test]
    fn decode_payload_display_value_handles_cbor_maps_with_integer_keys() {
        let nested = std::collections::BTreeMap::from([(
            serde_cbor::Value::Integer(1),
            serde_cbor::Value::Text("one".into()),
        )]);
        let root = std::collections::BTreeMap::from([(
            serde_cbor::Value::Text("versions".into()),
            serde_cbor::Value::Map(nested),
        )]);
        let bytes = serde_cbor::to_vec(&serde_cbor::Value::Map(root)).expect("encode cbor");
        let value = decode_payload_display_value(&bytes);
        assert_eq!(
            value
                .get("versions")
                .and_then(|v| v.get("1"))
                .and_then(Value::as_str),
            Some("one")
        );
    }

    #[test]
    fn render_json_value_decodes_domain_event_value_bytes() {
        let payload =
            aos_cbor::to_canonical_cbor(&json!({ "workspace": "alpha" })).expect("encode payload");
        let rendered = render_json_value(json!({
            "record": {
                "schema": "sys/WorkspaceCommit@1",
                "value": payload,
            }
        }));
        assert_eq!(
            rendered
                .get("record")
                .and_then(|v| v.get("value"))
                .and_then(|v| v.get("workspace"))
                .and_then(Value::as_str),
            Some("alpha")
        );
    }
}
