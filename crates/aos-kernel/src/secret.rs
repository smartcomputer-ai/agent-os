use std::{collections::HashMap, sync::Arc};

use aos_air_types::HashRef;
use aos_cbor::Hash;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSecret {
    pub binding_id: String,
    pub value: Vec<u8>,
    pub digest: Hash,
}

pub type SharedSecretResolver = Arc<dyn SecretResolver>;

pub trait SecretResolver: Send + Sync {
    fn resolve(
        &self,
        binding_id: &str,
        expected_digest: Option<&HashRef>,
    ) -> Result<ResolvedSecret, SecretResolverError>;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SecretResolverError {
    #[error("secret binding '{0}' not found")]
    NotFound(String),
    #[error(
        "secret digest mismatch for binding '{binding_id}': expected {expected}, found {found}"
    )]
    DigestMismatch {
        binding_id: String,
        expected: String,
        found: String,
    },
    #[error("invalid expected digest for binding '{binding_id}': {reason}")]
    InvalidExpectedDigest { binding_id: String, reason: String },
}

pub struct MapSecretResolver {
    values: HashMap<String, Vec<u8>>,
}

impl MapSecretResolver {
    pub fn new(values: HashMap<String, Vec<u8>>) -> Self {
        Self { values }
    }
}

impl SecretResolver for MapSecretResolver {
    fn resolve(
        &self,
        binding_id: &str,
        expected_digest: Option<&HashRef>,
    ) -> Result<ResolvedSecret, SecretResolverError> {
        let value = self
            .values
            .get(binding_id)
            .ok_or_else(|| SecretResolverError::NotFound(binding_id.to_string()))?;
        let digest = Hash::of_bytes(value);
        if let Some(expected) = expected_digest {
            let expected_hash = Hash::from_hex_str(expected.as_str()).map_err(|err| {
                SecretResolverError::InvalidExpectedDigest {
                    binding_id: binding_id.to_string(),
                    reason: err.to_string(),
                }
            })?;
            if digest != expected_hash {
                return Err(SecretResolverError::DigestMismatch {
                    binding_id: binding_id.to_string(),
                    expected: expected_hash.to_hex(),
                    found: digest.to_hex(),
                });
            }
        }
        Ok(ResolvedSecret {
            binding_id: binding_id.to_string(),
            value: value.clone(),
            digest,
        })
    }
}

/// Placeholder resolver used in shadow runs when no real resolver is configured.
/// It returns deterministic empty values and, when an expected digest is provided,
/// echoes it back so downstream checks remain stable.
pub struct PlaceholderSecretResolver;

impl SecretResolver for PlaceholderSecretResolver {
    fn resolve(
        &self,
        binding_id: &str,
        expected_digest: Option<&HashRef>,
    ) -> Result<ResolvedSecret, SecretResolverError> {
        let digest = if let Some(expected) = expected_digest {
            Hash::from_hex_str(expected.as_str()).map_err(|err| {
                SecretResolverError::InvalidExpectedDigest {
                    binding_id: binding_id.to_string(),
                    reason: err.to_string(),
                }
            })?
        } else {
            Hash::of_bytes(binding_id.as_bytes())
        };
        Ok(ResolvedSecret {
            binding_id: binding_id.to_string(),
            value: Vec::new(),
            digest,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter::FromIterator;

    #[test]
    fn map_resolver_returns_value_and_digest() {
        let resolver = MapSecretResolver::new(HashMap::from_iter([(
            "stripe:prod".to_string(),
            b"secret".to_vec(),
        )]));

        let resolved = resolver
            .resolve("stripe:prod", None)
            .expect("resolve secret");

        assert_eq!(resolved.value, b"secret".to_vec());
        assert_eq!(resolved.digest, Hash::of_bytes(b"secret"));
    }

    #[test]
    fn map_resolver_rejects_missing_binding() {
        let resolver = MapSecretResolver::new(HashMap::new());
        let err = resolver
            .resolve("missing", None)
            .expect_err("missing binding should error");

        assert!(matches!(err, SecretResolverError::NotFound(binding) if binding == "missing"));
    }

    #[test]
    fn map_resolver_rejects_digest_mismatch() {
        let resolver = MapSecretResolver::new(HashMap::from_iter([(
            "stripe:prod".to_string(),
            b"secret".to_vec(),
        )]));
        let expected = HashRef::new(Hash::of_bytes(b"other").to_hex()).unwrap();

        let err = resolver
            .resolve("stripe:prod", Some(&expected))
            .expect_err("digest mismatch should error");

        assert!(matches!(err, SecretResolverError::DigestMismatch { .. }));
    }

    #[test]
    fn placeholder_echoes_expected_digest() {
        let expected = HashRef::new(Hash::of_bytes(b"bytes").to_hex()).unwrap();
        let resolver = PlaceholderSecretResolver;

        let resolved = resolver
            .resolve("binding", Some(&expected))
            .expect("resolve placeholder");

        assert_eq!(
            resolved.digest,
            Hash::from_hex_str(expected.as_str()).unwrap()
        );
        assert!(resolved.value.is_empty());
    }
}
