use std::env;

const DEFAULT_UNSAFE_KEK_HEX: &str =
    "8f8e8d8c8b8a898887868584838281807f7e7d7c7b7a79787776757473727170";

#[derive(Debug, Clone)]
pub struct HostedSecretConfig {
    pub kek_id: String,
    pub kek_bytes: [u8; 32],
    pub allow_env_fallback: bool,
}

impl Default for HostedSecretConfig {
    fn default() -> Self {
        Self {
            kek_id: "unsafe-dev".into(),
            kek_bytes: decode_kek_hex(DEFAULT_UNSAFE_KEK_HEX)
                .expect("default hosted dev KEK must decode"),
            allow_env_fallback: false,
        }
    }
}

impl HostedSecretConfig {
    pub fn from_env() -> Result<Self, String> {
        let mut config = Self::default();
        if let Ok(value) = env::var("AOS_HOSTED_KEK_ID") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                config.kek_id = trimmed.to_owned();
            }
        }
        if let Ok(value) = env::var("AOS_HOSTED_KEK_HEX") {
            config.kek_bytes = decode_kek_hex(&value)?;
        }
        if let Ok(value) = env::var("AOS_HOSTED_SECRET_ENV_FALLBACK") {
            config.allow_env_fallback = matches!(value.trim(), "1" | "true" | "TRUE" | "yes");
        }
        Ok(config)
    }
}

fn decode_kek_hex(value: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(value.trim())
        .map_err(|err| format!("invalid hosted KEK hex '{}': {err}", value.trim()))?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| "hosted KEK must decode to exactly 32 bytes".to_string())
}
