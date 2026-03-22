use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use aos_cbor::Hash;
use aos_node::{
    SecretBindingRecord, SecretBindingSourceKind, SecretBindingStatus, SecretVersionRecord,
    SecretVersionStatus, UniverseId,
};

use crate::blobstore::BlobStoreConfig;

use super::blobstore::{VaultBlobstore, VaultStoreError};
use super::config::HostedSecretConfig;
use super::crypto::{decrypt_secret_record, encrypt_secret_bytes};
use super::resolver::HostedSecretResolver;

#[derive(Debug, thiserror::Error)]
pub enum HostedVaultError {
    #[error(transparent)]
    Store(#[from] VaultStoreError),
    #[error("invalid secret input: {0}")]
    Invalid(String),
    #[error("secret binding not found: {0}")]
    NotFound(String),
    #[error("vault mutex poisoned")]
    Poisoned,
}

#[derive(Debug, Clone)]
pub struct UpsertSecretBinding {
    pub source_kind: SecretBindingSourceKind,
    pub env_var: Option<String>,
    pub required_placement_pin: Option<String>,
    pub status: SecretBindingStatus,
}

#[derive(Clone)]
pub struct HostedVault {
    config: HostedSecretConfig,
    backend: Arc<Mutex<VaultBlobstore>>,
}

impl std::fmt::Debug for HostedVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedVault").finish_non_exhaustive()
    }
}

impl HostedVault {
    pub fn new(blobstore_config: BlobStoreConfig) -> Result<Self, HostedVaultError> {
        let config = HostedSecretConfig::from_env().map_err(HostedVaultError::Invalid)?;
        let backend = VaultBlobstore::new(unscoped_blobstore_config(&blobstore_config))?;
        Ok(Self {
            config,
            backend: Arc::new(Mutex::new(backend)),
        })
    }

    pub fn resolver_for_universe(&self, universe_id: UniverseId) -> HostedSecretResolver {
        HostedSecretResolver::new(self.clone(), universe_id)
    }

    pub fn list_bindings(
        &self,
        universe_id: UniverseId,
    ) -> Result<Vec<SecretBindingRecord>, HostedVaultError> {
        let mut bindings = self
            .backend
            .lock()
            .map_err(|_| HostedVaultError::Poisoned)?
            .list_bindings(universe_id)?;
        bindings.sort_by(|left, right| left.binding_id.cmp(&right.binding_id));
        Ok(bindings)
    }

    pub fn get_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<Option<SecretBindingRecord>, HostedVaultError> {
        self.backend
            .lock()
            .map_err(|_| HostedVaultError::Poisoned)?
            .get_binding(universe_id, binding_id)
            .map_err(Into::into)
    }

    pub fn upsert_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
        update: UpsertSecretBinding,
    ) -> Result<SecretBindingRecord, HostedVaultError> {
        let now = now_ns();
        let mut backend = self
            .backend
            .lock()
            .map_err(|_| HostedVaultError::Poisoned)?;
        let existing = backend.get_binding(universe_id, binding_id)?;
        let record = SecretBindingRecord {
            binding_id: binding_id.to_owned(),
            source_kind: update.source_kind,
            env_var: update.env_var,
            required_placement_pin: update.required_placement_pin,
            latest_version: existing.as_ref().and_then(|record| record.latest_version),
            created_at_ns: existing
                .as_ref()
                .map(|record| record.created_at_ns)
                .unwrap_or(now),
            updated_at_ns: now,
            status: update.status,
        };
        backend.put_binding(universe_id, record.clone())?;
        Ok(record)
    }

    pub fn delete_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<SecretBindingRecord, HostedVaultError> {
        self.backend
            .lock()
            .map_err(|_| HostedVaultError::Poisoned)?
            .delete_binding(universe_id, binding_id)?
            .ok_or_else(|| HostedVaultError::NotFound(binding_id.to_owned()))
    }

    pub fn list_versions(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<Vec<SecretVersionRecord>, HostedVaultError> {
        let mut versions = self
            .backend
            .lock()
            .map_err(|_| HostedVaultError::Poisoned)?
            .list_versions(universe_id, binding_id)?;
        versions.sort_by_key(|record| record.version);
        Ok(versions)
    }

    pub fn get_version(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<Option<SecretVersionRecord>, HostedVaultError> {
        self.backend
            .lock()
            .map_err(|_| HostedVaultError::Poisoned)?
            .get_version(universe_id, binding_id, version)
            .map_err(Into::into)
    }

    pub fn put_secret_value(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
        plaintext: &[u8],
        expected_digest: Option<&str>,
        actor: Option<String>,
    ) -> Result<SecretVersionRecord, HostedVaultError> {
        let mut backend = self
            .backend
            .lock()
            .map_err(|_| HostedVaultError::Poisoned)?;
        let mut binding = backend
            .get_binding(universe_id, binding_id)?
            .ok_or_else(|| HostedVaultError::NotFound(binding_id.to_owned()))?;
        if !matches!(binding.status, SecretBindingStatus::Active) {
            return Err(HostedVaultError::Invalid(format!(
                "binding '{binding_id}' is disabled"
            )));
        }
        if !matches!(
            binding.source_kind,
            SecretBindingSourceKind::NodeSecretStore
        ) {
            return Err(HostedVaultError::Invalid(format!(
                "binding '{binding_id}' is not configured for node_secret_store"
            )));
        }

        let digest = Hash::of_bytes(plaintext).to_hex();
        if let Some(expected_digest) = expected_digest {
            let normalized = Hash::from_hex_str(expected_digest)
                .map_err(|err| {
                    HostedVaultError::Invalid(format!(
                        "invalid expected_digest '{expected_digest}': {err}"
                    ))
                })?
                .to_hex();
            if normalized != digest {
                return Err(HostedVaultError::Invalid(format!(
                    "secret digest mismatch: expected {normalized}, got {digest}"
                )));
            }
        }

        let next_version = binding.latest_version.unwrap_or(0).saturating_add(1);
        if let Some(previous_version) = binding.latest_version {
            if let Some(mut previous) =
                backend.get_version(universe_id, binding_id, previous_version)?
            {
                previous.status = SecretVersionStatus::Superseded;
                backend.put_version(universe_id, previous)?;
            }
        }

        let envelope =
            encrypt_secret_bytes(&self.config, plaintext).map_err(HostedVaultError::Invalid)?;
        let record = SecretVersionRecord {
            binding_id: binding_id.to_owned(),
            version: next_version,
            digest,
            ciphertext: envelope.ciphertext,
            dek_wrapped: envelope.dek_wrapped,
            nonce: envelope.nonce,
            enc_alg: envelope.enc_alg,
            kek_id: self.config.kek_id.clone(),
            created_at_ns: now_ns(),
            created_by: actor,
            status: SecretVersionStatus::Active,
        };
        backend.put_version(universe_id, record.clone())?;
        binding.latest_version = Some(next_version);
        binding.updated_at_ns = now_ns();
        backend.put_binding(universe_id, binding)?;
        Ok(record)
    }

    pub(crate) fn resolve_worker_env(
        &self,
        env_var: &str,
    ) -> Result<Vec<u8>, aos_kernel::secret::SecretResolverError> {
        std::env::var(env_var)
            .map(|value| value.into_bytes())
            .map_err(|_| aos_kernel::secret::SecretResolverError::NotFound(env_var.to_owned()))
    }

    pub(crate) fn resolve_secret_value(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<Vec<u8>, aos_kernel::secret::SecretResolverError> {
        let binding = self
            .get_binding(universe_id, binding_id)
            .map_err(|err| aos_kernel::secret::SecretResolverError::Backend(err.to_string()))?;
        let value = match binding {
            Some(binding) if matches!(binding.status, SecretBindingStatus::Active) => {
                match binding.source_kind {
                    SecretBindingSourceKind::NodeSecretStore => {
                        let version_record = self
                            .get_version(universe_id, binding_id, version)
                            .map_err(|err| {
                                aos_kernel::secret::SecretResolverError::Backend(err.to_string())
                            })?
                            .ok_or_else(|| {
                                aos_kernel::secret::SecretResolverError::NotFound(format!(
                                    "{binding_id}@{version}"
                                ))
                            })?;
                        decrypt_secret_record(&self.config, &version_record)?
                    }
                    SecretBindingSourceKind::WorkerEnv => {
                        let env_var = binding.env_var.as_deref().ok_or_else(|| {
                            aos_kernel::secret::SecretResolverError::Backend(format!(
                                "binding '{binding_id}' missing env_var"
                            ))
                        })?;
                        self.resolve_worker_env(env_var)?
                    }
                }
            }
            Some(_) => {
                return Err(aos_kernel::secret::SecretResolverError::NotFound(
                    binding_id.to_owned(),
                ));
            }
            None if self.config.allow_env_fallback && binding_id.starts_with("env:") => {
                self.resolve_worker_env(binding_id.trim_start_matches("env:"))?
            }
            None => {
                return Err(aos_kernel::secret::SecretResolverError::NotFound(
                    binding_id.to_owned(),
                ));
            }
        };
        Ok(value)
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn unscoped_blobstore_config(config: &BlobStoreConfig) -> BlobStoreConfig {
    let mut next = config.clone();
    next.prefix = format!("{}/shared", config.prefix.trim_end_matches('/'));
    next
}
