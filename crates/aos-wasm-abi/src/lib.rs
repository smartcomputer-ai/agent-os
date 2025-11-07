//! Shared ABI envelopes for reducers and pure components (skeleton)

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct EventEnvelope {
    pub schema: String,
    #[serde(with = "serde_bytes")]
    pub value: Vec<u8>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct CallCtx {
    #[serde(with = "serde_bytes")]
    pub key: Option<Vec<u8>>,
    pub cell_mode: bool,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct InEnvelope {
    pub version: u8,
    #[serde(with = "serde_bytes")]
    pub state: Option<Vec<u8>>,
    pub event: EventEnvelope,
    pub ctx: CallCtx,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct OutEnvelope {
    #[serde(with = "serde_bytes")]
    pub state: Option<Vec<u8>>,
    #[serde(default)]
    pub domain_events: Vec<EventEnvelope>,
    #[serde(default)]
    pub effects: Vec<EffectIntent>,
    #[serde(default, with = "serde_bytes")]
    pub ann: Option<Vec<u8>>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct EffectIntent {
    pub kind: String,
    #[serde(with = "serde_bytes")]
    pub params_cbor: Vec<u8>,
    pub cap_slot: Option<String>,
}
