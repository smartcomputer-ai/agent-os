use anyhow::{Context, Result, anyhow};
use aos_agent::SessionId;
use aos_cbor::to_canonical_cbor;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde_json::Value;
use uuid::Uuid;

pub(crate) const SESSION_WORKFLOW: &str = "aos.agent/SessionWorkflow@1";
pub(crate) const SESSION_INPUT_SCHEMA: &str = "aos.agent/SessionInput@1";

pub(crate) fn new_session_id() -> String {
    Uuid::new_v4().to_string()
}

pub(crate) fn validate_session_id(value: &str) -> Result<String> {
    Uuid::parse_str(value)
        .map(|_| value.to_string())
        .with_context(|| format!("session id '{value}' must be a UUID"))
}

pub(crate) fn encode_session_key_b64(session_id: &str) -> Result<String> {
    let bytes = to_canonical_cbor(&SessionId(session_id.to_string()))
        .context("encode session id state key")?;
    Ok(BASE64_STANDARD.encode(bytes))
}

pub(crate) fn decode_session_key_bytes(bytes: &[u8]) -> Result<String> {
    if let Ok(session_id) = serde_cbor::from_slice::<SessionId>(bytes) {
        return validate_session_id(&session_id.0);
    }
    if let Ok(session_id) = serde_cbor::from_slice::<String>(bytes) {
        return validate_session_id(&session_id);
    }
    Err(anyhow!("state cell key is not an aos.agent SessionId"))
}

pub(crate) fn decode_cell_key(cell: &Value) -> Result<String> {
    if let Some(key_b64) = cell.get("key_b64").and_then(Value::as_str) {
        let bytes = BASE64_STANDARD
            .decode(key_b64)
            .with_context(|| format!("decode state cell key '{key_b64}'"))?;
        return decode_session_key_bytes(&bytes);
    }
    if let Some(items) = cell.get("key_bytes").and_then(Value::as_array) {
        let mut bytes = Vec::with_capacity(items.len());
        for item in items {
            let value = item
                .as_u64()
                .ok_or_else(|| anyhow!("state cell key byte entry is not an integer"))?;
            bytes.push(
                u8::try_from(value)
                    .map_err(|_| anyhow!("state cell key byte entry out of range: {value}"))?,
            );
        }
        return decode_session_key_bytes(&bytes);
    }
    Err(anyhow!("state cell is missing key_bytes/key_b64"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_key_round_trips() {
        let id = "018f2a66-31cc-7b25-a4f7-37e3310fdc6a";
        let key = encode_session_key_b64(id).expect("encode key");
        let bytes = BASE64_STANDARD.decode(key).expect("decode b64");
        assert_eq!(decode_session_key_bytes(&bytes).expect("decode key"), id);
    }
}
