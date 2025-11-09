use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Identifies an effect kind (e.g., `http.request`). New kinds can be added via AIR.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectKind(String);

impl EffectKind {
    pub const HTTP_REQUEST: &'static str = "http.request";
    pub const BLOB_PUT: &'static str = "blob.put";
    pub const BLOB_GET: &'static str = "blob.get";
    pub const TIMER_SET: &'static str = "timer.set";
    pub const LLM_GENERATE: &'static str = "llm.generate";

    pub fn new(kind: impl Into<String>) -> Self {
        Self(kind.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<S: Into<String>> From<S> for EffectKind {
    fn from(value: S) -> Self {
        Self::new(value)
    }
}

impl FromStr for EffectKind {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(s.to_owned()))
    }
}

impl fmt::Display for EffectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for EffectKind {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl EffectKind {
    pub fn from_air(kind: aos_air_types::EffectKind) -> Self {
        match kind {
            aos_air_types::EffectKind::HttpRequest => EffectKind::new(Self::HTTP_REQUEST),
            aos_air_types::EffectKind::BlobPut => EffectKind::new(Self::BLOB_PUT),
            aos_air_types::EffectKind::BlobGet => EffectKind::new(Self::BLOB_GET),
            aos_air_types::EffectKind::TimerSet => EffectKind::new(Self::TIMER_SET),
            aos_air_types::EffectKind::LlmGenerate => EffectKind::new(Self::LLM_GENERATE),
        }
    }
}
