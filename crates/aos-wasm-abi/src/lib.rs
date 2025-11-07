//! Reducer ABI envelopes shared by the kernel and WASM SDK.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current ABI version carried in reducer envelopes.
pub const ABI_VERSION: u8 = 1;

/// Reducer input envelope (kernel → WASM module).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReducerInput {
    pub version: u8,
    #[serde(with = "serde_bytes")]
    pub state: Option<Vec<u8>>,
    pub event: DomainEvent,
    pub ctx: CallContext,
}

impl ReducerInput {
    pub fn decode(bytes: &[u8]) -> Result<Self, AbiDecodeError> {
        let input: ReducerInput = serde_cbor::from_slice(bytes)?;
        if input.version != ABI_VERSION {
            return Err(AbiDecodeError::UnsupportedVersion {
                found: input.version,
            });
        }
        Ok(input)
    }

    pub fn encode(&self) -> Result<Vec<u8>, AbiEncodeError> {
        serde_cbor::to_vec(self).map_err(AbiEncodeError::Cbor)
    }
}

/// Reducer output envelope (WASM module → kernel).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ReducerOutput {
    #[serde(with = "serde_bytes")]
    pub state: Option<Vec<u8>>,
    #[serde(default)]
    pub domain_events: Vec<DomainEvent>,
    #[serde(default)]
    pub effects: Vec<ReducerEffect>,
    #[serde(default, with = "serde_bytes")]
    pub ann: Option<Vec<u8>>,
}

impl ReducerOutput {
    pub fn decode(bytes: &[u8]) -> Result<Self, AbiDecodeError> {
        serde_cbor::from_slice(bytes).map_err(AbiDecodeError::Cbor)
    }

    pub fn encode(&self) -> Result<Vec<u8>, AbiEncodeError> {
        serde_cbor::to_vec(self).map_err(AbiEncodeError::Cbor)
    }
}

/// Domain event value (schema + canonical CBOR payload).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DomainEvent {
    pub schema: String,
    #[serde(with = "serde_bytes")]
    pub value: Vec<u8>,
}

impl DomainEvent {
    pub fn new(schema: impl Into<String>, value: Vec<u8>) -> Self {
        Self {
            schema: schema.into(),
            value,
        }
    }
}

/// Contextual metadata provided with every reducer call.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CallContext {
    #[serde(with = "serde_bytes")]
    pub key: Option<Vec<u8>>,
    pub cell_mode: bool,
}

impl CallContext {
    pub fn new(cell_mode: bool, key: Option<Vec<u8>>) -> Self {
        Self { cell_mode, key }
    }
}

/// Micro-effect emitted directly from a reducer (timer/fs.blob, etc.).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReducerEffect {
    pub kind: String,
    #[serde(with = "serde_bytes")]
    pub params_cbor: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap_slot: Option<String>,
}

impl ReducerEffect {
    pub fn new(kind: impl Into<String>, params_cbor: Vec<u8>) -> Self {
        Self {
            kind: kind.into(),
            params_cbor,
            cap_slot: None,
        }
    }

    pub fn with_cap_slot(
        kind: impl Into<String>,
        params_cbor: Vec<u8>,
        slot: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            params_cbor,
            cap_slot: Some(slot.into()),
        }
    }
}

#[derive(Debug, Error)]
pub enum AbiDecodeError {
    #[error("ABI version {found} is not supported (expected {ABI_VERSION})")]
    UnsupportedVersion { found: u8 },
    #[error("failed to decode envelope: {0}")]
    Cbor(#[from] serde_cbor::Error),
}

#[derive(Debug, Error)]
pub enum AbiEncodeError {
    #[error("failed to encode envelope: {0}")]
    Cbor(#[from] serde_cbor::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_input() {
        let input = ReducerInput {
            version: ABI_VERSION,
            state: Some(vec![1, 2, 3]),
            event: DomainEvent::new("com.acme/Event@1", vec![0xaa]),
            ctx: CallContext::new(true, Some(vec![0x01, 0x02])),
        };
        let bytes = input.encode().expect("encode");
        let decoded = ReducerInput::decode(&bytes).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn rejects_wrong_version() {
        let mut input = ReducerInput {
            version: ABI_VERSION,
            state: None,
            event: DomainEvent::new("schema", vec![]),
            ctx: CallContext::new(false, None),
        };
        input.version = 99;
        let bytes = serde_cbor::to_vec(&input).unwrap();
        let err = ReducerInput::decode(&bytes).unwrap_err();
        assert!(matches!(err, AbiDecodeError::UnsupportedVersion { .. }));
    }

    #[test]
    fn round_trip_output() {
        let output = ReducerOutput {
            state: None,
            domain_events: vec![DomainEvent::new("schema", vec![1, 2])],
            effects: vec![ReducerEffect::with_cap_slot("timer.set", vec![9], "timer")],
            ann: Some(vec![0, 1]),
        };
        let bytes = output.encode().expect("encode");
        let decoded = ReducerOutput::decode(&bytes).expect("decode");
        assert_eq!(decoded, output);
    }
}
