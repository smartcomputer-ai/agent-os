use aos_cbor::{Hash, to_canonical_cbor};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

pub type IdempotencyKey = [u8; 32];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "origin", rename_all = "lowercase")]
pub enum EffectSource {
    Workflow { name: String },
    Plan { name: String },
}

impl EffectSource {
    pub fn workflow(name: impl Into<String>) -> Self {
        EffectSource::Workflow { name: name.into() }
    }

    pub fn plan(name: impl Into<String>) -> Self {
        EffectSource::Plan { name: name.into() }
    }

    pub fn origin_kind(&self) -> &'static str {
        match self {
            EffectSource::Workflow { .. } => "workflow",
            EffectSource::Plan { .. } => "plan",
        }
    }

    pub fn origin_name(&self) -> &str {
        match self {
            EffectSource::Workflow { name } | EffectSource::Plan { name } => name,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffectIntent {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub effect: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_module: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_module_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_entrypoint: Option<String>,
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
        effect: impl Into<String>,
        params_cbor: Vec<u8>,
        idempotency_key: IdempotencyKey,
    ) -> Result<Self, IntentEncodeError> {
        Self::from_raw_params_with_identity(
            effect.into(),
            None,
            None,
            None,
            None,
            params_cbor,
            idempotency_key,
        )
    }

    pub fn from_raw_params_with_identity(
        effect: String,
        effect_hash: Option<String>,
        executor_module: Option<String>,
        executor_module_hash: Option<String>,
        executor_entrypoint: Option<String>,
        params_cbor: Vec<u8>,
        idempotency_key: IdempotencyKey,
    ) -> Result<Self, IntentEncodeError> {
        let hash = compute_intent_hash(
            &effect,
            effect_hash.as_deref(),
            executor_module.as_deref(),
            executor_module_hash.as_deref(),
            executor_entrypoint.as_deref(),
            &params_cbor,
            &idempotency_key,
        )?;
        Ok(Self {
            effect,
            effect_hash,
            executor_module,
            executor_module_hash,
            executor_entrypoint,
            params_cbor,
            idempotency_key,
            intent_hash: hash,
        })
    }
}

pub struct IntentBuilder<'a, P> {
    effect: String,
    params: &'a P,
    idempotency_key: IdempotencyKey,
}

impl<'a, P> IntentBuilder<'a, P> {
    pub fn new(effect: impl Into<String>, params: &'a P) -> Self {
        Self {
            effect: effect.into(),
            params,
            idempotency_key: [0u8; 32],
        }
    }

    pub fn builder(effect: impl Into<String>, params: &'a P) -> Self {
        Self::new(effect, params)
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
            self.effect.as_str(),
            None,
            None,
            None,
            None,
            &params_cbor,
            &self.idempotency_key,
        )?;
        Ok(EffectIntent {
            effect: self.effect,
            effect_hash: None,
            executor_module: None,
            executor_module_hash: None,
            executor_entrypoint: None,
            params_cbor,
            idempotency_key: self.idempotency_key,
            intent_hash: hash,
        })
    }
}

fn compute_intent_hash(
    effect: &str,
    effect_hash: Option<&str>,
    executor_module: Option<&str>,
    executor_module_hash: Option<&str>,
    executor_entrypoint: Option<&str>,
    params_cbor: &[u8],
    idempotency_key: &IdempotencyKey,
) -> Result<[u8; 32], serde_cbor::Error> {
    #[derive(Serialize)]
    struct Envelope<'a> {
        effect: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        effect_hash: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        executor_module: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        executor_module_hash: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        executor_entrypoint: Option<&'a str>,
        #[serde(with = "serde_bytes")]
        params: &'a [u8],
        #[serde(with = "serde_bytes")]
        idempotency_key: &'a [u8; 32],
    }

    let bytes = to_canonical_cbor(&Envelope {
        effect,
        effect_hash,
        executor_module,
        executor_module_hash,
        executor_entrypoint,
        params: params_cbor,
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
