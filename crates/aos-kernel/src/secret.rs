use std::{collections::HashMap, sync::Arc};

use aos_air_types::{HashRef, SecretDecl};
use aos_cbor::Hash;
use thiserror::Error;
use aos_effects::EffectSource;

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

/// Catalog of declared secrets from the manifest for quick lookup.
#[derive(Clone, Default)]
pub struct SecretCatalog {
    by_key: HashMap<(String, u64), SecretDecl>,
}

impl SecretCatalog {
    pub fn new(decls: &[SecretDecl]) -> Self {
        let mut by_key = HashMap::new();
        for decl in decls {
            by_key.insert((decl.alias.clone(), decl.version), decl.clone());
        }
        Self { by_key }
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    pub fn lookup(&self, alias: &str, version: u64) -> Option<&SecretDecl> {
        self.by_key.get(&(alias.to_string(), version))
    }
}

/// Walks effect params CBOR for SecretRef variants and validates them against the catalog and resolver.
pub fn validate_secrets_in_params(
    params_cbor: &[u8],
    catalog: &SecretCatalog,
    resolver: &dyn SecretResolver,
) -> Result<(), SecretResolverError> {
    let value: serde_cbor::Value =
        serde_cbor::from_slice(params_cbor).map_err(|err| SecretResolverError::InvalidParams {
            reason: err.to_string(),
        })?;
    let mut refs = Vec::new();
    collect_secret_refs_value(&value, &mut refs);
    for (alias, version) in refs {
        let decl = catalog
            .lookup(&alias, version)
            .ok_or_else(|| SecretResolverError::NotFound(format!("{alias}@{version}")))?;
        resolver.resolve(&decl.binding_id, decl.expected_digest.as_ref())?;
    }
    Ok(())
}

fn collect_secret_refs_value(value: &serde_cbor::Value, refs: &mut Vec<(String, u64)>) {
    match value {
        serde_cbor::Value::Array(items) => {
            for item in items {
                collect_secret_refs_value(item, refs);
            }
        }
        serde_cbor::Value::Map(map) => {
            // Look for variant arm {"secret": {"alias": "...", "version": N}}
            if map.len() == 1 {
                if let Some((serde_cbor::Value::Text(tag), serde_cbor::Value::Map(inner))) =
                    map.iter().next()
                {
                    if tag == "secret" {
                        if let (Some(alias_val), Some(version_val)) = (
                            inner.get(&serde_cbor::Value::Text("alias".into())),
                            inner.get(&serde_cbor::Value::Text("version".into())),
                        ) {
                            if let (serde_cbor::Value::Text(alias), serde_cbor::Value::Integer(v)) =
                                (alias_val, version_val)
                            {
                                refs.push((alias.clone(), *v as u64));
                            }
                        }
                    }
                }
            }
            for (_k, v) in map {
                collect_secret_refs_value(v, refs);
            }
        }
        _ => {}
    }
}

/// Resolve and inject secrets into params CBOR, replacing secret variants with literal text/bytes.
/// Returns canonical CBOR bytes with injected values.
pub fn inject_secrets_in_params(
    params_cbor: &[u8],
    catalog: &SecretCatalog,
    resolver: &dyn SecretResolver,
) -> Result<Vec<u8>, SecretResolverError> {
    let mut value: serde_cbor::Value =
        serde_cbor::from_slice(params_cbor).map_err(|err| SecretResolverError::InvalidParams {
            reason: err.to_string(),
        })?;
    inject_in_value(&mut value, catalog, resolver)?;
    serde_cbor::to_vec(&value).map_err(|err| SecretResolverError::InvalidParams {
        reason: err.to_string(),
    })
}

fn inject_in_value(
    value: &mut serde_cbor::Value,
    catalog: &SecretCatalog,
    resolver: &dyn SecretResolver,
) -> Result<(), SecretResolverError> {
    match value {
        serde_cbor::Value::Array(items) => {
            for item in items {
                inject_in_value(item, catalog, resolver)?;
            }
        }
        serde_cbor::Value::Map(map) => {
            // Detect variant {"secret": {"alias": "...", "version": n}}
            if map.len() == 1 {
                if let Some((serde_cbor::Value::Text(tag), serde_cbor::Value::Map(inner))) =
                    map.iter_mut().next()
                {
                    if tag == "secret" {
                        if let (Some(alias_val), Some(version_val)) = (
                            inner.get(&serde_cbor::Value::Text("alias".into())),
                            inner.get(&serde_cbor::Value::Text("version".into())),
                        ) {
                            if let (serde_cbor::Value::Text(alias), serde_cbor::Value::Integer(v)) =
                                (alias_val, version_val)
                            {
                                let version = *v as u64;
                                let decl = catalog
                                    .lookup(alias, version)
                                    .ok_or_else(|| SecretResolverError::NotFound(format!("{alias}@{version}")))?;
                                let resolved = resolver
                                    .resolve(&decl.binding_id, decl.expected_digest.as_ref())?;
                                // Replace with text if utf-8, else bytes
                                if let Ok(text) = std::str::from_utf8(&resolved.value) {
                                    *value = serde_cbor::Value::Text(text.to_string());
                                } else {
                                    *value = serde_cbor::Value::Bytes(resolved.value);
                                }
                                return Ok(());
                            }
                        }
                    }
                }
            }
            for (_k, v) in map.iter_mut() {
                inject_in_value(v, catalog, resolver)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Collect all SecretRef occurrences from params CBOR.
pub fn collect_secret_refs(params_cbor: &[u8]) -> Result<Vec<(String, u64)>, SecretResolverError> {
    let value: serde_cbor::Value =
        serde_cbor::from_slice(params_cbor).map_err(|err| SecretResolverError::InvalidParams {
            reason: err.to_string(),
        })?;
    let mut refs = Vec::new();
    collect_secret_refs_value(&value, &mut refs);
    Ok(refs)
}

/// Enforce per-secret policy (allowed_caps / allowed_plans) against collected secret refs.
pub fn enforce_secret_policy(
    params_cbor: &[u8],
    catalog: &SecretCatalog,
    origin: &EffectSource,
    cap_name: &str,
) -> Result<(), crate::error::KernelError> {
    let refs =
        collect_secret_refs(params_cbor).map_err(|e| crate::error::KernelError::SecretResolution(
            e.to_string(),
        ))?;
    for (alias, version) in refs {
        if let Some(decl) = catalog.lookup(&alias, version) {
            if let Some(policy) = &decl.policy {
                if !policy.allowed_caps.is_empty()
                    && !policy.allowed_caps.contains(&cap_name.to_string())
                {
                    return Err(crate::error::KernelError::SecretPolicyDenied {
                        alias: alias.clone(),
                        version,
                        reason: format!("cap grant '{cap_name}' not allowed"),
                    });
                }
                if let EffectSource::Plan { name } = origin {
                    if !policy.allowed_plans.is_empty() && !policy.allowed_plans.contains(name) {
                        return Err(crate::error::KernelError::SecretPolicyDenied {
                            alias: alias.clone(),
                            version,
                            reason: format!("plan '{name}' not allowed"),
                        });
                    }
                }
            }
        }
    }
    Ok(())
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
    #[error("failed to parse effect params for secrets: {reason}")]
    InvalidParams { reason: String },
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

    #[test]
    fn injects_secret_into_params() {
        let mut map = HashMap::new();
        map.insert("binding".to_string(), b"token123".to_vec());
        let resolver = MapSecretResolver::new(map);
        let decl = SecretDecl {
            alias: "auth/api".into(),
            version: 1,
            binding_id: "binding".into(),
            expected_digest: None,
            policy: None,
        };
        let catalog = SecretCatalog::new(&[decl]);
        // params: {headers: {"authorization": {"secret": {"alias": "...", "version": 1}}}}
        use serde_cbor::value::to_value;
        use std::collections::BTreeMap;
        let mut secret_map = BTreeMap::new();
        secret_map.insert(
            serde_cbor::Value::Text("alias".into()),
            serde_cbor::Value::Text("auth/api".into()),
        );
        secret_map.insert(
            serde_cbor::Value::Text("version".into()),
            serde_cbor::Value::Integer(1),
        );
        let mut auth_map = BTreeMap::new();
        auth_map.insert(
            serde_cbor::Value::Text("secret".into()),
            serde_cbor::Value::Map(secret_map),
        );
        let mut headers_map = BTreeMap::new();
        headers_map.insert(
            serde_cbor::Value::Text("authorization".into()),
            serde_cbor::Value::Map(auth_map),
        );
        let mut root_map = BTreeMap::new();
        root_map.insert(
            serde_cbor::Value::Text("headers".into()),
            serde_cbor::Value::Map(headers_map),
        );
        let params = serde_cbor::to_vec(&serde_cbor::Value::Map(root_map))
            .unwrap();

        let injected =
            inject_secrets_in_params(&params, &catalog, &resolver).expect("inject secrets");
        let value: serde_cbor::Value = serde_cbor::from_slice(&injected).unwrap();
        // Ensure the secret variant was replaced with the resolved text.
        if let serde_cbor::Value::Map(root) = value {
            let headers = root
                .iter()
                .find(|(k, _)| matches!(k, serde_cbor::Value::Text(t) if t == "headers"))
                .unwrap()
                .1
                .clone();
            if let serde_cbor::Value::Map(hmap) = headers {
                let auth = hmap
                    .iter()
                    .find(|(k, _)| matches!(k, serde_cbor::Value::Text(t) if t == "authorization"))
                    .unwrap()
                    .1
                    .clone();
                assert!(matches!(auth, serde_cbor::Value::Text(t) if t == "token123"));
            } else {
                panic!("headers not a map");
            }
        } else {
            panic!("root not a map");
        }
    }
}
