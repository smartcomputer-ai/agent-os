use super::{build_receipt, failed_receipt, optional_string, parse_json_object, require_string};
use crate::contracts::ToolCallStatus;
use crate::tools::types::{
    ToolEffectKind, ToolMappedArgs, ToolMappedReceipt, ToolMappingError, ToolRuntimeDelta,
};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use aos_effect_types::introspect::{
    IntrospectCellInfo, IntrospectListCellsReceipt, IntrospectWorkflowStateReceipt, ReadMeta,
};
use serde_cbor::Value as CborValue;
use serde_json::{Map, Value, json};

pub fn map_args(arguments_json: &str) -> Result<ToolMappedArgs, ToolMappingError> {
    let args = parse_json_object(arguments_json)?;
    let workflow = require_string(&args, "workflow")?;
    let view = optional_string(&args, "view").unwrap_or_else(|| "state".into());

    match view.as_str() {
        "state" => {
            let mut out = Map::new();
            out.insert("workflow".into(), Value::String(workflow));
            out.insert("consistency".into(), Value::String("head".into()));
            if let Some(key) = decode_cell_key(&args)? {
                out.insert("key".into(), bytes_json(&key));
            }
            Ok(ToolMappedArgs::with_effect_kind(
                ToolEffectKind::IntrospectWorkflowState,
                Value::Object(out),
            ))
        }
        "cells" => Ok(ToolMappedArgs::with_effect_kind(
            ToolEffectKind::IntrospectListCells,
            json!({
                "workflow": workflow
            }),
        )),
        _ => Err(ToolMappingError::invalid_args(
            "'view' must be either 'state' or 'cells'",
        )),
    }
}

pub fn map_receipt(tool_name: &str, status: &str, payload: &[u8]) -> ToolMappedReceipt {
    if !status.trim().eq_ignore_ascii_case("ok") {
        return decode_error_receipt(tool_name, status, payload);
    }

    let payload_value: CborValue = match serde_cbor::from_slice(payload) {
        Ok(value) => value,
        Err(err) => {
            return failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                format!("failed to decode introspection receipt: {err}"),
            );
        }
    };

    if payload_has_key(&payload_value, "cells") {
        let receipt: IntrospectListCellsReceipt = match serde_cbor::value::from_value(payload_value)
        {
            Ok(value) => value,
            Err(err) => {
                return failed_receipt(
                    tool_name,
                    status,
                    "receipt_decode_error",
                    format!("failed to decode list_cells receipt: {err}"),
                );
            }
        };
        return build_receipt(
            tool_name,
            status,
            json!({
                "kind": "cells",
                "cells": receipt.cells.iter().map(cell_json).collect::<Vec<_>>(),
                "meta": meta_json(&receipt.meta),
            }),
            false,
            ToolCallStatus::Succeeded,
            ToolRuntimeDelta::default(),
        );
    }

    let receipt: IntrospectWorkflowStateReceipt = match serde_cbor::value::from_value(payload_value)
    {
        Ok(value) => value,
        Err(err) => {
            return failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                format!("failed to decode workflow_state receipt: {err}"),
            );
        }
    };

    let (state_encoding, state_value, state_hex, state_bytes_len) = match receipt.state {
        Some(bytes) => {
            let state_bytes_len = bytes.len() as u64;
            if let Ok(decoded) = serde_cbor::from_slice::<Value>(&bytes) {
                ("cbor_json", decoded, None, state_bytes_len)
            } else if let Ok(text) = core::str::from_utf8(&bytes) {
                (
                    "utf8",
                    Value::String(text.to_string()),
                    None,
                    state_bytes_len,
                )
            } else {
                (
                    "hex",
                    Value::Null,
                    Some(Value::String(bytes_to_hex(&bytes))),
                    state_bytes_len,
                )
            }
        }
        None => ("none", Value::Null, None, 0),
    };

    build_receipt(
        tool_name,
        status,
        json!({
            "kind": "state",
            "state_encoding": state_encoding,
            "state": state_value,
            "state_hex": state_hex,
            "state_bytes_len": state_bytes_len,
            "meta": meta_json(&receipt.meta),
        }),
        false,
        ToolCallStatus::Succeeded,
        ToolRuntimeDelta::default(),
    )
}

fn decode_cell_key(args: &Map<String, Value>) -> Result<Option<Vec<u8>>, ToolMappingError> {
    let Some(cell_key) = args.get("cell_key").and_then(Value::as_object) else {
        return Ok(None);
    };
    let encoding = cell_key
        .get("encoding")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolMappingError::invalid_args("'cell_key.encoding' must be a string"))?;
    let value = cell_key
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolMappingError::invalid_args("'cell_key.value' must be a string"))?;

    match encoding {
        "utf8" => Ok(Some(value.as_bytes().to_vec())),
        "hex" => parse_hex(value).map(Some),
        _ => Err(ToolMappingError::invalid_args(
            "'cell_key.encoding' must be 'utf8' or 'hex'",
        )),
    }
}

fn parse_hex(value: &str) -> Result<Vec<u8>, ToolMappingError> {
    let bytes = value.as_bytes();
    if bytes.len() % 2 != 0 {
        return Err(ToolMappingError::invalid_args(
            "'cell_key.value' must have an even number of hex digits",
        ));
    }

    let mut out = Vec::with_capacity(bytes.len() / 2);
    for idx in (0..bytes.len()).step_by(2) {
        let hi = decode_hex_nibble(bytes[idx]).ok_or_else(|| {
            ToolMappingError::invalid_args("'cell_key.value' contains non-hex characters")
        })?;
        let lo = decode_hex_nibble(bytes[idx + 1]).ok_or_else(|| {
            ToolMappingError::invalid_args("'cell_key.value' contains non-hex characters")
        })?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn decode_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn bytes_json(bytes: &[u8]) -> Value {
    Value::Array(
        bytes
            .iter()
            .map(|byte| Value::Number((*byte as u64).into()))
            .collect(),
    )
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(hex_char(byte >> 4));
        out.push(hex_char(byte & 0x0f));
    }
    out
}

const fn hex_char(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'a' + (value - 10)) as char,
    }
}

fn payload_has_key(value: &CborValue, key: &str) -> bool {
    let CborValue::Map(entries) = value else {
        return false;
    };
    entries
        .iter()
        .any(|(entry_key, _)| matches!(entry_key, CborValue::Text(text) if text == key))
}

fn cell_json(cell: &IntrospectCellInfo) -> Value {
    let utf8 = core::str::from_utf8(&cell.key)
        .ok()
        .map(|text| Value::String(text.to_string()))
        .unwrap_or(Value::Null);
    json!({
        "key": {
            "utf8": utf8,
            "hex": bytes_to_hex(&cell.key),
            "bytes_len": cell.key.len(),
        },
        "state_hash": cell.state_hash.to_string(),
        "size": cell.size,
        "last_active_ns": cell.last_active_ns,
    })
}

fn meta_json(meta: &ReadMeta) -> Value {
    json!({
        "journal_height": meta.journal_height,
        "snapshot_hash": meta.snapshot_hash.as_ref().map(|hash| hash.to_string()),
        "manifest_hash": meta.manifest_hash.to_string(),
    })
}

fn decode_error_receipt(tool_name: &str, status: &str, payload: &[u8]) -> ToolMappedReceipt {
    let payload_json = serde_cbor::from_slice::<Value>(payload).unwrap_or_else(|_| {
        json!({
            "error_code": "adapter_error",
            "error_message": format!("tool {tool_name} failed with status={status}"),
        })
    });
    let code = payload_json
        .get("error_code")
        .and_then(Value::as_str)
        .unwrap_or("adapter_error");
    let detail = payload_json
        .get("error_message")
        .and_then(Value::as_str)
        .unwrap_or("introspection request failed");
    failed_receipt(tool_name, status, code, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use aos_effect_types::HashRef;

    #[test]
    fn cells_view_overrides_effect_kind() {
        let mapped = map_args(r#"{"workflow":"demo/Flow@1","view":"cells"}"#).expect("map args");
        assert_eq!(
            mapped.effect_kind,
            Some(ToolEffectKind::IntrospectListCells)
        );
        assert_eq!(mapped.params_json["workflow"], "demo/Flow@1");
    }

    #[test]
    fn state_view_decodes_hex_key() {
        let mapped = map_args(
            r#"{"workflow":"demo/Flow@1","view":"state","cell_key":{"encoding":"hex","value":"6162"}}"#,
        )
        .expect("map args");
        assert_eq!(
            mapped.params_json["key"],
            Value::Array(vec![
                Value::Number(97u64.into()),
                Value::Number(98u64.into())
            ])
        );
    }

    #[test]
    fn map_receipt_shapes_cells() {
        let receipt = IntrospectListCellsReceipt {
            cells: vec![IntrospectCellInfo {
                key: b"abc".to_vec(),
                state_hash: HashRef::new(
                    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                )
                .expect("hash"),
                size: 12,
                last_active_ns: 42,
            }],
            meta: ReadMeta {
                journal_height: 2,
                snapshot_hash: None,
                manifest_hash: HashRef::new(
                    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                )
                .expect("hash"),
            },
        };

        let mapped = map_receipt(
            "inspect_workflow",
            "ok",
            &serde_cbor::to_vec(&receipt).expect("receipt"),
        );
        let output: Value =
            serde_json::from_str(&mapped.llm_output_json).expect("decode llm output json");
        assert_eq!(output["result"]["kind"], "cells");
        assert_eq!(output["result"]["cells"][0]["key"]["utf8"], "abc");
    }
}
