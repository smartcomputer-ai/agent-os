use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use aos_cbor::Hash;
use aos_effects::{
    EffectIntent, EffectReceipt, ReceiptStatus,
    builtins::{
        HostFileContentInput, HostFsApplyPatchParams, HostFsWriteFileParams, HostPatchInput,
    },
};
use aos_kernel::Store;

pub(crate) fn now_wallclock_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

pub(crate) fn build_receipt<T: serde::Serialize>(
    intent: &EffectIntent,
    status: ReceiptStatus,
    payload: &T,
) -> anyhow::Result<EffectReceipt> {
    Ok(EffectReceipt {
        intent_hash: intent.intent_hash,
        status,
        payload_cbor: serde_cbor::to_vec(payload)
            .with_context(|| format!("encode {} payload", intent.effect))?,
        cost_cents: Some(0),
        signature: vec![0; 64],
    })
}

pub(crate) fn normalize_host_fs_read_encoding(
    encoding: Option<&str>,
) -> Result<&'static str, String> {
    let Some(encoding) = encoding else {
        return Ok("utf8");
    };
    let trimmed = encoding.trim();
    if trimmed.eq_ignore_ascii_case("utf8") || trimmed.eq_ignore_ascii_case("utf-8") {
        return Ok("utf8");
    }
    if trimmed.eq_ignore_ascii_case("bytes") {
        return Ok("bytes");
    }
    Err(format!("unsupported encoding '{encoding}'"))
}

pub(crate) fn decode_host_fs_write_file_params(
    bytes: &[u8],
) -> anyhow::Result<HostFsWriteFileParams> {
    if let Ok(params) = serde_cbor::from_slice::<HostFsWriteFileParams>(bytes) {
        return Ok(params);
    }

    let mut value: serde_json::Value = serde_cbor::from_slice(bytes)?;
    normalize_write_file_param_shape(&mut value)?;
    serde_json::from_value(value).map_err(anyhow::Error::from)
}

fn normalize_write_file_param_shape(value: &mut serde_json::Value) -> anyhow::Result<()> {
    let obj = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("write_file params must be an object"))?;
    let Some(content_value) = obj.get_mut("content") else {
        return Err(anyhow::anyhow!("write_file params missing 'content'"));
    };

    let Some(content_obj) = content_value.as_object_mut() else {
        return Ok(());
    };

    let Some(tag) = content_obj.get("$tag").and_then(serde_json::Value::as_str) else {
        return Ok(());
    };
    let Some(payload) = content_obj.get("$value").cloned() else {
        return Err(anyhow::anyhow!(
            "write_file tagged content missing '$value'"
        ));
    };

    let normalized = match tag {
        "inline_text" => serde_json::json!({ "inline_text": payload }),
        "inline_bytes" => serde_json::json!({ "inline_bytes": payload }),
        "blob_ref" => serde_json::json!({ "blob_ref": payload }),
        other => {
            return Err(anyhow::anyhow!(
                "write_file tagged content has unsupported tag '{}'",
                other
            ));
        }
    };
    *content_value = normalized;
    Ok(())
}

pub(crate) fn decode_host_fs_apply_patch_params(
    bytes: &[u8],
) -> anyhow::Result<HostFsApplyPatchParams> {
    if let Ok(params) = serde_cbor::from_slice::<HostFsApplyPatchParams>(bytes) {
        return Ok(params);
    }

    let mut value: serde_json::Value = serde_cbor::from_slice(bytes)?;
    normalize_apply_patch_param_shape(&mut value)?;
    serde_json::from_value(value).map_err(anyhow::Error::from)
}

fn normalize_apply_patch_param_shape(value: &mut serde_json::Value) -> anyhow::Result<()> {
    let obj = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("apply_patch params must be an object"))?;
    let Some(patch_value) = obj.get_mut("patch") else {
        return Err(anyhow::anyhow!("apply_patch params missing 'patch'"));
    };

    let Some(patch_obj) = patch_value.as_object_mut() else {
        return Ok(());
    };

    let Some(tag) = patch_obj.get("$tag").and_then(serde_json::Value::as_str) else {
        return Ok(());
    };
    let Some(payload) = patch_obj.get("$value").cloned() else {
        return Err(anyhow::anyhow!("apply_patch tagged patch missing '$value'"));
    };

    let normalized = match tag {
        "inline_text" => serde_json::json!({ "inline_text": payload }),
        "blob_ref" => serde_json::json!({ "blob_ref": payload }),
        other => {
            return Err(anyhow::anyhow!(
                "apply_patch tagged patch has unsupported tag '{}'",
                other
            ));
        }
    };
    *patch_value = normalized;
    Ok(())
}

pub(crate) fn resolve_file_content<S: Store>(
    store: &S,
    content: &HostFileContentInput,
) -> Result<Vec<u8>, String> {
    match content {
        HostFileContentInput::InlineText { inline_text } => {
            Ok(inline_text.text.as_bytes().to_vec())
        }
        HostFileContentInput::InlineBytes { inline_bytes } => Ok(inline_bytes.bytes.clone()),
        HostFileContentInput::BlobRef { blob_ref } => {
            let hash =
                Hash::from_hex_str(blob_ref.blob_ref.as_str()).map_err(|err| err.to_string())?;
            store.get_blob(hash).map_err(|err| err.to_string())
        }
    }
}

pub(crate) fn resolve_patch_text<S: Store>(
    store: &S,
    params: &HostFsApplyPatchParams,
) -> Result<String, String> {
    let bytes = match &params.patch {
        HostPatchInput::InlineText { inline_text } => inline_text.text.as_bytes().to_vec(),
        HostPatchInput::BlobRef { blob_ref } => {
            let hash =
                Hash::from_hex_str(blob_ref.blob_ref.as_str()).map_err(|err| err.to_string())?;
            store.get_blob(hash).map_err(|err| err.to_string())?
        }
    };

    String::from_utf8(bytes).map_err(|_| "patch must be valid utf8".into())
}
