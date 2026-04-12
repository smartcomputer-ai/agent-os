use aos_node::{SecretBindingRecord, SecretVersionRecord, UniverseId};

use crate::vault::{HostedVault, HostedVaultError, UpsertSecretBinding};

#[derive(Clone)]
pub struct HostedSecretService {
    vault: HostedVault,
}

impl HostedSecretService {
    pub fn new(vault: HostedVault) -> Self {
        Self { vault }
    }

    pub fn list_bindings(
        &self,
        universe_id: UniverseId,
    ) -> Result<Vec<SecretBindingRecord>, HostedVaultError> {
        self.vault.list_bindings(universe_id)
    }

    pub fn get_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<Option<SecretBindingRecord>, HostedVaultError> {
        self.vault.get_binding(universe_id, binding_id)
    }

    pub fn upsert_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
        request: UpsertSecretBinding,
    ) -> Result<SecretBindingRecord, HostedVaultError> {
        self.vault.upsert_binding(universe_id, binding_id, request)
    }

    pub fn delete_binding(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<SecretBindingRecord, HostedVaultError> {
        self.vault.delete_binding(universe_id, binding_id)
    }

    pub fn list_versions(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
    ) -> Result<Vec<SecretVersionRecord>, HostedVaultError> {
        self.vault.list_versions(universe_id, binding_id)
    }

    pub fn get_version(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<Option<SecretVersionRecord>, HostedVaultError> {
        self.vault.get_version(universe_id, binding_id, version)
    }

    pub fn put_secret_value(
        &self,
        universe_id: UniverseId,
        binding_id: &str,
        plaintext: &[u8],
        expected_digest: Option<&str>,
        actor: Option<String>,
    ) -> Result<SecretVersionRecord, HostedVaultError> {
        self.vault
            .put_secret_value(universe_id, binding_id, plaintext, expected_digest, actor)
    }
}
