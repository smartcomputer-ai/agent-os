use aos_cbor::{Hash, to_canonical_cbor};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

use crate::EffectKind;

pub type IdempotencyKey = [u8; 32];

/// Canonical effect intent stored in the journal/outbox.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffectIntent {
    pub kind: EffectKind,
    pub cap_name: String,
    #[serde(with = "serde_bytes")]
    pub params_cbor: Vec<u8>,
    pub idempotency_key: IdempotencyKey,
    pub intent_hash: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EffectSource {
    Reducer { name: String },
    Plan { name: String },
}

impl EffectSource {
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

impl EffectIntent {
    pub fn builder<'a, P: Serialize>(
        kind: impl Into<EffectKind>,
        cap_name: impl Into<String>,
        params: &'a P,
    ) -> IntentBuilder<'a, P> {
        IntentBuilder::new(kind.into(), cap_name.into(), params)
    }

    pub fn params<T: DeserializeOwned>(&self) -> Result<T, serde_cbor::Error> {
        serde_cbor::from_slice(&self.params_cbor)
    }

    pub fn intent_hash_hex(&self) -> String {
        format!("sha256:{}", hex::encode(self.intent_hash))
    }
}

pub struct IntentBuilder<'a, P> {
    kind: EffectKind,
    cap_name: String,
    params: &'a P,
    idempotency_key: IdempotencyKey,
}

impl<'a, P> IntentBuilder<'a, P> {
    fn new(kind: EffectKind, cap_name: String, params: &'a P) -> Self {
        Self {
            kind,
            cap_name,
            params,
            idempotency_key: [0u8; 32],
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EffectKind;
    use serde::Serialize;

    #[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
    struct DummyParams {
        foo: String,
    }

    #[test]
    fn builder_encodes_params_and_hash() {
        let params = DummyParams { foo: "bar".into() };
        let key = [7u8; 32];
        let intent = EffectIntent::builder(EffectKind::HTTP_REQUEST, "cap_http", &params)
            .idempotency_key(key)
            .build()
            .expect("builder");
        assert_eq!(intent.cap_name, "cap_http");
        let decoded: DummyParams = intent.params().expect("decode params");
        assert_eq!(decoded.foo, "bar");
        assert_eq!(intent.idempotency_key, key);
        assert_ne!(intent.intent_hash, [0u8; 32]);
    }
}
