use aos_cbor::Hash;
use aos_kernel::secret::{ResolvedSecret, SecretResolver, SecretResolverError};
use aos_node::UniverseId;

use super::service::HostedVault;

#[derive(Clone)]
pub struct HostedSecretResolver {
    vault: HostedVault,
    universe_id: UniverseId,
}

impl HostedSecretResolver {
    pub fn new(vault: HostedVault, universe_id: UniverseId) -> Self {
        Self { vault, universe_id }
    }
}

impl SecretResolver for HostedSecretResolver {
    fn resolve(
        &self,
        binding_id: &str,
        version: u64,
        expected_digest: Option<&aos_air_types::HashRef>,
    ) -> Result<ResolvedSecret, SecretResolverError> {
        let value = self
            .vault
            .resolve_secret_value(self.universe_id, binding_id, version)?;
        let digest = Hash::of_bytes(&value);
        if let Some(expected) = expected_digest {
            let expected_hash = Hash::from_hex_str(expected.as_str()).map_err(|err| {
                SecretResolverError::InvalidExpectedDigest {
                    binding_id: binding_id.to_owned(),
                    reason: err.to_string(),
                }
            })?;
            if digest != expected_hash {
                return Err(SecretResolverError::DigestMismatch {
                    binding_id: binding_id.to_owned(),
                    expected: expected_hash.to_hex(),
                    found: digest.to_hex(),
                });
            }
        }
        Ok(ResolvedSecret {
            binding_id: binding_id.to_owned(),
            value,
            digest,
        })
    }
}
