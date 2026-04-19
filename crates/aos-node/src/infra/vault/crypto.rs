use aes_gcm_siv::aead::{Aead, KeyInit};
use aes_gcm_siv::{Aes256GcmSiv, Nonce};
use aos_kernel::secret::SecretResolverError;
use aos_node::SecretVersionRecord;

use super::config::HostedSecretConfig;

const NONCE_LEN: usize = 12;
const COMBINED_NONCE_LEN: usize = NONCE_LEN * 2;

pub struct SecretEnvelope {
    pub ciphertext: Vec<u8>,
    pub dek_wrapped: Vec<u8>,
    pub nonce: Vec<u8>,
    pub enc_alg: String,
}

pub fn encrypt_secret_bytes(
    config: &HostedSecretConfig,
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

pub fn decrypt_secret_record(
    config: &HostedSecretConfig,
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
