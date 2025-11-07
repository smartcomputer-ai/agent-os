use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

/// Signed adapter receipt referencing an effect intent hash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffectReceipt {
    pub intent_hash: [u8; 32],
    pub adapter_id: String,
    pub status: ReceiptStatus,
    #[serde(with = "serde_bytes")]
    pub payload_cbor: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cents: Option<u64>,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}

impl EffectReceipt {
    pub fn payload<T: DeserializeOwned>(&self) -> Result<T, ReceiptDecodeError> {
        serde_cbor::from_slice(&self.payload_cbor).map_err(ReceiptDecodeError::Payload)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    Ok,
    Error,
    Timeout,
}

#[derive(Debug, Error)]
pub enum ReceiptDecodeError {
    #[error("failed to decode receipt payload: {0}")]
    Payload(#[from] serde_cbor::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
    struct DummyReceipt {
        ok: bool,
    }

    #[test]
    fn payload_round_trip() {
        let payload = serde_cbor::to_vec(&DummyReceipt { ok: true }).unwrap();
        let receipt = EffectReceipt {
            intent_hash: [1u8; 32],
            adapter_id: "adapter.http".into(),
            status: ReceiptStatus::Ok,
            payload_cbor: payload,
            cost_cents: Some(42),
            signature: vec![9, 9, 9],
        };
        let decoded: DummyReceipt = receipt.payload().unwrap();
        assert_eq!(decoded, DummyReceipt { ok: true });
    }
}
