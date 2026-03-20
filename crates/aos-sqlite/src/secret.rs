use std::sync::Arc;

use aes_gcm_siv::aead::{Aead, KeyInit};
use aes_gcm_siv::{Aes256GcmSiv, Nonce};
use aos_cbor::Hash;
use aos_kernel::secret::{ResolvedSecret, SecretResolver, SecretResolverError};
use aos_node::{
    PutSecretVersionRequest, SecretBindingRecord, SecretBindingSourceKind, SecretBindingStatus,
    SecretStore, SecretVersionRecord, UniverseId,
};
use sha2::{Digest, Sha256};

const NONCE_LEN: usize = 12;
const COMBINED_NONCE_LEN: usize = NONCE_LEN * 2;
const DEFAULT_UNSAFE_KEK_HEX: &str =
    "8f8e8d8c8b8a898887868584838281807f7e7d7c7b7a79787776757473727170";

#[derive(Debug, Clone)]
pub struct LocalSecretConfig {
    pub kek_id: String,
    pub kek_bytes: [u8; 32],
    pub allow_env_fallback: bool,
}

impl Default for LocalSecretConfig {
    fn default() -> Self {
        Self {
            kek_id: "unsafe-local-dev".into(),
            kek_bytes: decode_kek_hex(DEFAULT_UNSAFE_KEK_HEX)
                .expect("default local KEK must decode"),
            allow_env_fallback: false,
        }
    }
}

impl LocalSecretConfig {
    pub fn from_env() -> Result<Self, String> {
        let mut config = Self::default();
        if let Ok(value) = std::env::var("AOS_LOCAL_KEK_ID") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                config.kek_id = trimmed.to_string();
            }
        }
        if let Ok(value) = std::env::var("AOS_LOCAL_KEK_HEX") {
            config.kek_bytes = decode_kek_hex(&value)?;
        }
        if let Ok(value) = std::env::var("AOS_LOCAL_SECRET_ENV_FALLBACK") {
            config.allow_env_fallback = matches!(value.trim(), "1" | "true" | "TRUE" | "yes");
        }
        Ok(config)
    }
}

#[derive(Debug, Clone)]
pub struct LocalSecretPutResult {
    pub version: SecretVersionRecord,
}

#[derive(Clone)]
pub struct LocalSecretService<P> {
    persistence: Arc<P>,
    universe: UniverseId,
    config: LocalSecretConfig,
}

impl<P> LocalSecretService<P>
where
    P: SecretStore + 'static,
{
    pub fn new(persistence: Arc<P>, universe: UniverseId, config: LocalSecretConfig) -> Self {
        Self {
            persistence,
            universe,
            config,
        }
    }

    pub fn put_secret_value(
        &self,
        binding: &SecretBindingRecord,
        plaintext: &[u8],
        expected_digest: Option<&str>,
        actor: Option<String>,
        created_at_ns: u64,
    ) -> Result<LocalSecretPutResult, String> {
        if !matches!(
            binding.source_kind,
            SecretBindingSourceKind::NodeSecretStore
        ) {
            return Err(format!(
                "binding '{}' is not configured for node_secret_store",
                binding.binding_id
            ));
        }
        if !matches!(binding.status, SecretBindingStatus::Active) {
            return Err(format!("binding '{}' is disabled", binding.binding_id));
        }
        let digest = Hash::of_bytes(plaintext).to_hex();
        if let Some(expected_digest) = expected_digest {
            let normalized = Hash::from_hex_str(expected_digest)
                .map_err(|err| format!("invalid expected_digest '{expected_digest}': {err}"))?
                .to_hex();
            if normalized != digest {
                return Err(format!(
                    "secret digest mismatch: expected {normalized}, got {digest}"
                ));
            }
        }
        let envelope = encrypt_secret_bytes(&self.config, plaintext)?;
        let version = self
            .persistence
            .put_secret_version(
                self.universe,
                PutSecretVersionRequest {
                    binding_id: binding.binding_id.clone(),
                    digest,
                    ciphertext: envelope.ciphertext,
                    dek_wrapped: envelope.dek_wrapped,
                    nonce: envelope.nonce,
                    enc_alg: envelope.enc_alg,
                    kek_id: self.config.kek_id.clone(),
                    created_at_ns,
                    created_by: actor,
                },
            )
            .map_err(|err| err.to_string())?;
        Ok(LocalSecretPutResult { version })
    }
}

#[derive(Clone)]
pub struct LocalSecretResolver<P> {
    persistence: Arc<P>,
    universe: UniverseId,
    config: LocalSecretConfig,
}

impl<P> LocalSecretResolver<P>
where
    P: SecretStore + 'static,
{
    pub fn new(persistence: Arc<P>, universe: UniverseId, config: LocalSecretConfig) -> Self {
        Self {
            persistence,
            universe,
            config,
        }
    }
}

impl<P> SecretResolver for LocalSecretResolver<P>
where
    P: SecretStore + 'static,
{
    fn resolve(
        &self,
        binding_id: &str,
        version: u64,
        expected_digest: Option<&aos_air_types::HashRef>,
    ) -> Result<ResolvedSecret, SecretResolverError> {
        let binding = self
            .persistence
            .get_secret_binding(self.universe, binding_id)
            .map_err(|err| SecretResolverError::Backend(err.to_string()))?;
        let value = match binding {
            Some(binding) if matches!(binding.status, SecretBindingStatus::Active) => {
                match binding.source_kind {
                    SecretBindingSourceKind::NodeSecretStore => {
                        let version_record = self
                            .persistence
                            .get_secret_version(self.universe, binding_id, version)
                            .map_err(|err| SecretResolverError::Backend(err.to_string()))?
                            .ok_or_else(|| {
                                SecretResolverError::NotFound(format!("{binding_id}@{version}"))
                            })?;
                        decrypt_secret_record(&self.config, &version_record)?
                    }
                    SecretBindingSourceKind::WorkerEnv => {
                        let env_var = binding.env_var.as_deref().ok_or_else(|| {
                            SecretResolverError::Backend(format!(
                                "binding '{binding_id}' missing env_var"
                            ))
                        })?;
                        std::env::var(env_var)
                            .map(|value| value.into_bytes())
                            .map_err(|_| SecretResolverError::NotFound(env_var.to_string()))?
                    }
                }
            }
            Some(_) => return Err(SecretResolverError::NotFound(binding_id.to_string())),
            None if self.config.allow_env_fallback && binding_id.starts_with("env:") => {
                let env_var = binding_id.trim_start_matches("env:");
                std::env::var(env_var)
                    .map(|value| value.into_bytes())
                    .map_err(|_| SecretResolverError::NotFound(env_var.to_string()))?
            }
            None => return Err(SecretResolverError::NotFound(binding_id.to_string())),
        };

        let digest = Hash::of_bytes(&value);
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
            value,
            digest,
        })
    }
}

struct SecretEnvelope {
    ciphertext: Vec<u8>,
    dek_wrapped: Vec<u8>,
    nonce: Vec<u8>,
    enc_alg: String,
}

fn encrypt_secret_bytes(
    config: &LocalSecretConfig,
    plaintext: &[u8],
) -> Result<SecretEnvelope, String> {
    let mut dek = [0u8; 32];
    let mut data_nonce = [0u8; NONCE_LEN];
    let mut wrap_nonce = [0u8; NONCE_LEN];
    fill_random(&mut dek)?;
    fill_random(&mut data_nonce)?;
    fill_random(&mut wrap_nonce)?;
    let dek_cipher = Aes256GcmSiv::new_from_slice(&dek).map_err(|err| err.to_string())?;
    let ciphertext = dek_cipher
        .encrypt(&Nonce::from(data_nonce), plaintext)
        .map_err(|err| err.to_string())?;
    let kek_cipher =
        Aes256GcmSiv::new_from_slice(&config.kek_bytes).map_err(|err| err.to_string())?;
    let dek_wrapped = kek_cipher
        .encrypt(&Nonce::from(wrap_nonce), dek.as_slice())
        .map_err(|err| err.to_string())?;
    let mut nonce = Vec::with_capacity(COMBINED_NONCE_LEN);
    nonce.extend_from_slice(&data_nonce);
    nonce.extend_from_slice(&wrap_nonce);
    Ok(SecretEnvelope {
        ciphertext,
        dek_wrapped,
        nonce,
        enc_alg: "aes-256-gcm-siv+wrap-v1".into(),
    })
}

fn decrypt_secret_record(
    config: &LocalSecretConfig,
    record: &SecretVersionRecord,
) -> Result<Vec<u8>, SecretResolverError> {
    if record.nonce.len() != COMBINED_NONCE_LEN {
        return Err(SecretResolverError::Backend(format!(
            "secret record '{}' has invalid nonce length {}",
            record.binding_id,
            record.nonce.len()
        )));
    }
    let (data_nonce, wrap_nonce) = record.nonce.split_at(NONCE_LEN);
    let data_nonce: [u8; NONCE_LEN] = data_nonce
        .try_into()
        .map_err(|_| SecretResolverError::Backend("data nonce length mismatch".into()))?;
    let wrap_nonce: [u8; NONCE_LEN] = wrap_nonce
        .try_into()
        .map_err(|_| SecretResolverError::Backend("wrap nonce length mismatch".into()))?;
    let kek_cipher = Aes256GcmSiv::new_from_slice(&config.kek_bytes)
        .map_err(|err| SecretResolverError::Backend(format!("initialize KEK cipher: {err}")))?;
    let dek = kek_cipher
        .decrypt(&Nonce::from(wrap_nonce), record.dek_wrapped.as_ref())
        .map_err(|err| SecretResolverError::Backend(format!("unwrap DEK: {err}")))?;
    let dek_cipher = Aes256GcmSiv::new_from_slice(&dek)
        .map_err(|err| SecretResolverError::Backend(format!("initialize DEK cipher: {err}")))?;
    dek_cipher
        .decrypt(&Nonce::from(data_nonce), record.ciphertext.as_ref())
        .map_err(|err| SecretResolverError::Backend(format!("decrypt secret: {err}")))
}

fn fill_random(target: &mut [u8]) -> Result<(), String> {
    getrandom::getrandom(target).map_err(|err| format!("random source unavailable: {err}"))
}

fn decode_kek_hex(value: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(value.trim())
        .map_err(|err| format!("invalid KEK hex '{}': {err}", value.trim()))?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| "KEK must decode to exactly 32 bytes".to_string())
}

#[allow(dead_code)]
pub fn default_kek_fingerprint(config: &LocalSecretConfig) -> String {
    let mut hasher = Sha256::new();
    hasher.update(config.kek_bytes);
    hex::encode(hasher.finalize())
}
