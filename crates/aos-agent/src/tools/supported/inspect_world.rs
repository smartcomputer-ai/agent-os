use super::{build_receipt, failed_receipt, parse_json_object};
use crate::contracts::ToolCallStatus;
use crate::tools::types::{ToolMappedArgs, ToolMappedReceipt, ToolRuntimeDelta};
use alloc::format;
use alloc::string::ToString;
use aos_air_types::Manifest;
use aos_effect_types::introspect::IntrospectManifestReceipt;
use serde_json::{Value, json};

pub fn map_args(
    arguments_json: &str,
) -> Result<ToolMappedArgs, crate::tools::types::ToolMappingError> {
    let _ = parse_json_object(arguments_json)?;
    Ok(ToolMappedArgs::params(json!({
        "consistency": "head"
    })))
}

pub fn map_receipt(tool_name: &str, status: &str, payload: &[u8]) -> ToolMappedReceipt {
    if !status.trim().eq_ignore_ascii_case("ok") {
        return decode_error_receipt(tool_name, status, payload);
    }

    let receipt: IntrospectManifestReceipt = match serde_cbor::from_slice(payload) {
        Ok(value) => value,
        Err(err) => {
            return failed_receipt(
                tool_name,
                status,
                "receipt_decode_error",
                format!("failed to decode introspect.manifest receipt: {err}"),
            );
        }
    };
    let manifest: Manifest = match serde_cbor::from_slice(&receipt.manifest) {
        Ok(value) => value,
        Err(err) => {
            return failed_receipt(
                tool_name,
                status,
                "manifest_decode_error",
                format!("failed to decode manifest bytes: {err}"),
            );
        }
    };
    let manifest_json = match serde_json::to_value(&manifest) {
        Ok(value) => value,
        Err(err) => {
            return failed_receipt(
                tool_name,
                status,
                "manifest_encode_error",
                format!("failed to encode manifest as JSON: {err}"),
            );
        }
    };

    let result = json!({
        "summary": {
            "air_version": manifest.air_version,
            "journal_height": receipt.meta.journal_height,
            "manifest_hash": receipt.meta.manifest_hash.to_string(),
            "snapshot_hash": receipt.meta.snapshot_hash.map(|hash| hash.to_string()),
            "schema_count": manifest.schemas.len(),
            "module_count": manifest.modules.len(),
            "effect_count": manifest.ops.len(),
            "has_routing": manifest.routing.is_some(),
        },
        "modules": section(&manifest_json, "modules", json!([])),
        "effects": section(&manifest_json, "ops", json!([])),
        "routing": section(
            &manifest_json,
            "routing",
            json!({
                "subscriptions": [],
                "inboxes": [],
            }),
        ),
        "effect_bindings": section(&manifest_json, "effect_bindings", json!([])),
    });

    build_receipt(
        tool_name,
        status,
        result,
        false,
        ToolCallStatus::Succeeded,
        ToolRuntimeDelta::default(),
    )
}

fn section(root: &Value, key: &str, default: Value) -> Value {
    root.get(key).cloned().unwrap_or(default)
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
    fn map_args_uses_head_consistency() {
        let mapped = map_args("{}").expect("map args");
        assert_eq!(mapped.effect_kind, None);
        assert_eq!(mapped.params_json["consistency"], "head");
    }

    #[test]
    fn map_receipt_shapes_manifest_summary() {
        let receipt = IntrospectManifestReceipt {
            manifest: serde_cbor::to_vec(&Manifest {
                air_version: "2".into(),
                schemas: vec![],
                modules: vec![],
                ops: vec![],
                secrets: vec![],
                routing: None,
            })
            .expect("manifest"),
            meta: aos_effect_types::introspect::ReadMeta {
                journal_height: 7,
                snapshot_hash: None,
                manifest_hash: HashRef::new(
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                )
                .expect("hash"),
            },
        };

        let mapped = map_receipt(
            "inspect_world",
            "ok",
            &serde_cbor::to_vec(&receipt).expect("receipt"),
        );
        let output: Value =
            serde_json::from_str(&mapped.llm_output_json).expect("decode llm output json");
        assert_eq!(output["result"]["summary"]["journal_height"], 7);
        assert_eq!(output["result"]["summary"]["module_count"], 0);
    }
}
