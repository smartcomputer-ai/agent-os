use aos_cbor::{Hash, to_canonical_cbor};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

use crate::EffectKind;

pub type IdempotencyKey = [u8; 32];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "origin", rename_all = "lowercase")]
pub enum EffectSource {
    Reducer { name: String },
    Plan { name: String },
}

impl EffectSource {
    pub fn reducer(name: impl Into<String>) -> Self {
        EffectSource::Reducer { name: name.into() }
    }

    pub fn plan(name: impl Into<String>) -> Self {
        EffectSource::Plan { name: name.into() }
    }

    pub fn origin_kind(&self) -> &'static str {
        match self {
            EffectSource::Reducer { .. } => "reducer",
            EffectSource::Plan { .. } => "plan",
        }
    }

    pub fn origin_name(&self) -> &str {
        match self {
            EffectSource::Reducer { name } | EffectSource::Plan { name } => name,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffectIntent {
    pub kind: EffectKind,
    pub cap_name: String,
    #[serde(with = "serde_bytes")]
    pub params_cbor: Vec<u8>,
    pub idempotency_key: IdempotencyKey,
    pub intent_hash: [u8; 32],
}

impl EffectIntent {
    pub fn params<T: DeserializeOwned>(&self) -> Result<T, serde_cbor::Error> {
        serde_cbor::from_slice(&self.params_cbor)
    }

    pub fn from_raw_params(
        kind: EffectKind,
        cap_name: impl Into<String>,
        params_cbor: Vec<u8>,
        idempotency_key: IdempotencyKey,
    ) -> Result<Self, IntentEncodeError> {
        let cap_name = cap_name.into();
        let hash = compute_intent_hash(kind.as_str(), &params_cbor, &cap_name, &idempotency_key)?;
        Ok(Self {
            kind,
            cap_name,
            params_cbor,
            idempotency_key,
            intent_hash: hash,
        })
    }
}

pub struct IntentBuilder<'a, P> {
    kind: EffectKind,
    cap_name: String,
    params: &'a P,
    idempotency_key: IdempotencyKey,
}

impl<'a, P> IntentBuilder<'a, P> {
    pub fn new(kind: EffectKind, cap_name: impl Into<String>, params: &'a P) -> Self {
        let cap_name = cap_name.into();
        Self {
            kind,
            cap_name,
            params,
            idempotency_key: [0u8; 32],
        }
    }

    pub fn builder(
        kind: impl Into<EffectKind>,
        cap_name: impl Into<String>,
        params: &'a P,
    ) -> Self {
        Self::new(kind.into(), cap_name, params)
    }

    pub fn idempotency_key(mut self, key: IdempotencyKey) -> Self {
        self.idempotency_key = key;
        self
    }

    pub fn build(self) -> Result<EffectIntent, IntentEncodeError>
    where
        P: Serialize,
    {
        let params_cbor = to_canonical_cbor(self.params)?;
        let hash = compute_intent_hash(
            self.kind.as_str(),
            &params_cbor,
            &self.cap_name,
            &self.idempotency_key,
        )?;
        Ok(EffectIntent {
            kind: self.kind,
            cap_name: self.cap_name,
            params_cbor,
            idempotency_key: self.idempotency_key,
            intent_hash: hash,
        })
    }
}

fn compute_intent_hash(
    kind: &str,
    params_cbor: &[u8],
    cap_name: &str,
    idempotency_key: &IdempotencyKey,
) -> Result<[u8; 32], serde_cbor::Error> {
    #[derive(Serialize)]
    struct Envelope<'a> {
        kind: &'a str,
        #[serde(with = "serde_bytes")]
        params: &'a [u8],
        cap: &'a str,
        #[serde(with = "serde_bytes")]
        idempotency_key: &'a [u8; 32],
    }

    let bytes = to_canonical_cbor(&Envelope {
        kind,
        params: params_cbor,
        cap: cap_name,
        idempotency_key,
    })?;
    let hash: Hash = Hash::of_bytes(&bytes);
    Ok(hash.into())
}

#[derive(Debug, Error)]
pub enum IntentEncodeError {
    #[error("failed to encode intent params: {0}")]
    Params(#[from] serde_cbor::Error),
}
