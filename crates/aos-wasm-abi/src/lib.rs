//! Reducer/pure ABI envelopes shared by the kernel and WASM SDK.

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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub ctx: Option<Vec<u8>>,
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

/// Pure module input envelope (kernel → WASM module).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PureInput {
    pub version: u8,
    #[serde(with = "serde_bytes")]
    pub input: Vec<u8>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub ctx: Option<Vec<u8>>,
}

impl PureInput {
    pub fn decode(bytes: &[u8]) -> Result<Self, AbiDecodeError> {
        let input: PureInput = serde_cbor::from_slice(bytes)?;
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

/// Pure module output envelope (WASM module → kernel).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PureOutput {
    #[serde(with = "serde_bytes")]
    pub output: Vec<u8>,
}

impl PureOutput {
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub key: Option<Vec<u8>>,
}

impl DomainEvent {
    pub fn new(schema: impl Into<String>, value: Vec<u8>) -> Self {
        Self {
            schema: schema.into(),
            value,
            key: None,
        }
    }

    pub fn with_key(schema: impl Into<String>, value: Vec<u8>, key: Vec<u8>) -> Self {
        Self {
            schema: schema.into(),
            value,
            key: Some(key),
        }
    }
}

mod serde_bytes_opt {
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_bytes::{ByteBuf, Bytes};

    pub fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(bytes) => serializer.serialize_some(Bytes::new(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<ByteBuf>::deserialize(deserializer).map(|opt| opt.map(|buf| buf.into_vec()))
    }
}

/// Contextual metadata provided with every reducer call.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReducerContext {
    pub now_ns: u64,
    pub logical_now_ns: u64,
    pub journal_height: u64,
    #[serde(with = "serde_bytes")]
    pub entropy: Vec<u8>,
    pub event_hash: String,
    pub manifest_hash: String,
    pub reducer: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub key: Option<Vec<u8>>,
    pub cell_mode: bool,
}

impl ReducerContext {
    pub fn decode(bytes: &[u8]) -> Result<Self, serde_cbor::Error> {
        serde_cbor::from_slice(bytes)
    }
}

/// Contextual metadata provided with every pure module call.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PureContext {
    pub logical_now_ns: u64,
    pub journal_height: u64,
    pub manifest_hash: String,
    pub module: String,
}

impl PureContext {
    pub fn decode(bytes: &[u8]) -> Result<Self, serde_cbor::Error> {
        serde_cbor::from_slice(bytes)
    }
}

/// Micro-effect emitted directly from a reducer (timer/blob, etc.).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReducerEffect {
    pub kind: String,
    #[serde(with = "serde_bytes")]
    pub params_cbor: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap_slot: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub idempotency_key: Option<Vec<u8>>,
}

impl ReducerEffect {
    pub fn new(kind: impl Into<String>, params_cbor: Vec<u8>) -> Self {
        Self {
            kind: kind.into(),
            params_cbor,
            cap_slot: None,
            idempotency_key: None,
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
            idempotency_key: None,
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
        let ctx = ReducerContext {
            now_ns: 10,
            logical_now_ns: 12,
            journal_height: 7,
            entropy: vec![0x11; 64],
            event_hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            manifest_hash: "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                .into(),
            reducer: "com.acme/Reducer@1".into(),
            key: Some(vec![0x01, 0x02]),
            cell_mode: true,
        };
        let ctx_bytes = serde_cbor::to_vec(&ctx).expect("ctx bytes");
        let input = ReducerInput {
            version: ABI_VERSION,
            state: Some(vec![1, 2, 3]),
            event: DomainEvent::new("com.acme/Event@1", vec![0xaa]),
            ctx: Some(ctx_bytes),
        };
        let bytes = input.encode().expect("encode");
        let decoded = ReducerInput::decode(&bytes).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn rejects_wrong_version() {
        let ctx = ReducerContext {
            now_ns: 10,
            logical_now_ns: 12,
            journal_height: 7,
            entropy: vec![0x11; 64],
            event_hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            manifest_hash: "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                .into(),
            reducer: "com.acme/Reducer@1".into(),
            key: None,
            cell_mode: false,
        };
        let ctx_bytes = serde_cbor::to_vec(&ctx).expect("ctx bytes");
        let mut input = ReducerInput {
            version: ABI_VERSION,
            state: None,
            event: DomainEvent::new("schema", vec![]),
            ctx: Some(ctx_bytes),
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

    #[test]
    fn round_trip_pure_envelopes() {
        let ctx = PureContext {
            logical_now_ns: 9,
            journal_height: 3,
            manifest_hash: "sha256:2222222222222222222222222222222222222222222222222222222222222222"
                .into(),
            module: "com.acme/Pure@1".into(),
        };
        let ctx_bytes = serde_cbor::to_vec(&ctx).expect("ctx bytes");
        let input = PureInput {
            version: ABI_VERSION,
            input: vec![0xaa, 0xbb],
            ctx: Some(ctx_bytes),
        };
        let bytes = input.encode().expect("encode");
        let decoded = PureInput::decode(&bytes).expect("decode");
        assert_eq!(decoded, input);

        let output = PureOutput {
            output: vec![0x01, 0x02],
        };
        let out_bytes = output.encode().expect("encode");
        let out_decoded = PureOutput::decode(&out_bytes).expect("decode");
        assert_eq!(out_decoded, output);
    }
}
