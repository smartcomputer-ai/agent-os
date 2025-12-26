use aos_cbor::to_canonical_cbor;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityGrant {
    pub name: String,
    pub cap: String,
    #[serde(with = "serde_bytes")]
    pub params_cbor: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry_ns: Option<u64>,
}

impl CapabilityGrant {
    pub fn builder<'a, P: Serialize>(
        name: impl Into<String>,
        cap: impl Into<String>,
        params: &'a P,
    ) -> CapabilityGrantBuilder<'a, P> {
        CapabilityGrantBuilder::new(name.into(), cap.into(), params)
    }

    pub fn params<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_cbor::Error> {
        serde_cbor::from_slice(&self.params_cbor)
    }
}

pub struct CapabilityGrantBuilder<'a, P> {
    name: String,
    cap: String,
    params: &'a P,
    expiry_ns: Option<u64>,
}

impl<'a, P: Serialize> CapabilityGrantBuilder<'a, P> {
    fn new(name: String, cap: String, params: &'a P) -> Self {
        Self {
            name,
            cap,
            params,
            expiry_ns: None,
        }
    }

    pub fn expiry_ns(mut self, expiry: u64) -> Self {
        self.expiry_ns = Some(expiry);
        self
    }

    pub fn build(self) -> Result<CapabilityGrant, CapabilityEncodeError> {
        let params_cbor = to_canonical_cbor(self.params)?;
        Ok(CapabilityGrant {
            name: self.name,
            cap: self.cap,
            params_cbor,
            expiry_ns: self.expiry_ns,
        })
    }
}

#[derive(Debug, Error)]
pub enum CapabilityEncodeError {
    #[error("failed to encode capability params: {0}")]
    Params(#[from] serde_cbor::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
    struct HttpGrantParams {
        hosts: Vec<String>,
    }

    #[test]
    fn grant_round_trip() {
        let params = HttpGrantParams {
            hosts: vec!["example.com".into()],
        };
        let grant = CapabilityGrant::builder("cap1", "sys/http.out@1", &params)
            .build()
            .unwrap();
        let decoded: HttpGrantParams = grant.params().unwrap();
        assert_eq!(decoded.hosts[0], "example.com");
    }
}
