//! Canonical CBOR helpers and stable SHA-256 hashing utilities used across AgentOS.

use serde::Serialize;
use serde_cbor::{
    ser::Write as CborWrite,
    value::Value as CborValue,
};
use sha2::{Digest, Sha256};
use std::fmt;

/// Prefix for serialized hashes. Matches AIR references (e.g. `sha256:deadbeef`).
pub const HASH_PREFIX: &str = "sha256:";

/// Serialize a value into canonical CBOR bytes using RFC 8949 deterministic rules.
pub fn to_canonical_cbor<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_cbor::Error> {
    let mut buf = Vec::with_capacity(256);
    write_canonical_cbor(value, &mut buf)?;
    Ok(buf)
}

/// Serialize a value directly into an arbitrary CBOR writer using canonical settings.
pub fn write_canonical_cbor<T: Serialize, W>(value: &T, writer: W) -> Result<(), serde_cbor::Error>
where
    W: CborWrite,
{
    let canonical_value: CborValue = serde_cbor::value::to_value(value)?;
    let mut serializer = serde_cbor::ser::Serializer::new(writer);
    serializer.self_describe()?;
    canonical_value.serialize(&mut serializer)
}

/// Wrapper around a 32-byte SHA-256 digest used for content addressing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash([u8; 32]);

impl Hash {
    /// Compute the hash of a value's canonical CBOR encoding.
    pub fn of_cbor<T: Serialize>(value: &T) -> Result<Self, serde_cbor::Error> {
        Ok(Self::of_bytes(&to_canonical_cbor(value)?))
    }

    /// Compute the hash of the provided byte slice.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&digest);
        Hash(arr)
    }

    /// Borrow the raw digest bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Render the digest as a `sha256:...` hex string.
    pub fn to_hex(&self) -> String {
        format!("{HASH_PREFIX}{}", hex::encode(self.0))
    }

    /// Parse a hash from its `sha256:`-prefixed hex string representation.
    pub fn from_hex_str(s: &str) -> Result<Self, HashParseError> {
        let rest = s.strip_prefix(HASH_PREFIX).ok_or(HashParseError::MissingPrefix)?;
        if rest.len() != 64 {
            return Err(HashParseError::InvalidLength(rest.len()));
        }
        let mut buf = [0u8; 32];
        hex::decode_to_slice(rest, &mut buf).map_err(HashParseError::InvalidHex)?;
        Ok(Hash(buf))
    }

    /// Attempt to build a hash from raw bytes, ensuring the length matches.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, HashLengthError> {
        if bytes.len() != 32 {
            return Err(HashLengthError(bytes.len()));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        Ok(Hash(arr))
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Hash").field(&self.to_hex()).finish()
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl From<[u8; 32]> for Hash {
    fn from(value: [u8; 32]) -> Self {
        Hash(value)
    }
}

impl From<Hash> for [u8; 32] {
    fn from(value: Hash) -> Self {
        value.0
    }
}

impl AsRef<[u8; 32]> for Hash {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl TryFrom<&[u8]> for Hash {
    type Error = HashLengthError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Hash::from_bytes(value)
    }
}

impl TryFrom<&str> for Hash {
    type Error = HashParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Hash::from_hex_str(value)
    }
}

/// Error returned when a `sha256:` string is malformed.
#[derive(Debug, thiserror::Error)]
pub enum HashParseError {
    #[error("hash string missing '{HASH_PREFIX}' prefix")]
    MissingPrefix,
    #[error("hash hex length must be 64, got {0}")]
    InvalidLength(usize),
    #[error("invalid hex: {0}")]
    InvalidHex(#[from] hex::FromHexError),
}

/// Error returned when attempting to create a hash from the wrong byte length.
#[derive(Debug, thiserror::Error)]
#[error("hash must be 32 bytes, got {0}")]
pub struct HashLengthError(pub usize);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::{fs, path::PathBuf};

    fn load_schema(name: &str) -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../spec/schemas")
            .join(name);
        let data = fs::read_to_string(path).expect("schema file");
        serde_json::from_str(&data).expect("valid json")
    }

    #[test]
    fn canonical_round_trip_and_hashes() {
        let cases = [
            (
                "common.schema.json",
                "sha256:84037f2b1956ead1d3e70b21f5680bc77e07e6ba906d698865b2d9065a5c1877",
            ),
            (
                "defschema.schema.json",
                "sha256:492c7e9583481d3060bf444efb82f7263434f49f85aab238504152ff8ec1115c",
            ),
            (
                "defmodule.schema.json",
                "sha256:9db4dc958903289dde3e63efc4ebc91e5c4a00fd4a910a0a90b69efa9cb006eb",
            ),
            (
                "defplan.schema.json",
                "sha256:6b1f7f97293280bb211f6c5a438817e21bd99ff167acf430b069a22c48ef5d22",
            ),
            (
                "defcap.schema.json",
                "sha256:e869bc489103999552fee90c5a72b01616b8b3e408245a31a4b1486e9155fe98",
            ),
            (
                "defpolicy.schema.json",
                "sha256:8fc34e93510ea5d23bf8fd7edad69a610b4ece9d63e26ddb9436440e8ffd17ee",
            ),
            (
                "manifest.schema.json",
                "sha256:377d11b05843e834a87730ca2d425f73ad8d26ed73e7149dd7e75f037a7065b3",
            ),
        ];

        for (name, expected_hash) in cases {
            let value = load_schema(name);
            let bytes = to_canonical_cbor(&value).expect("canonical encode");
            let decoded: Value = serde_cbor::from_slice(&bytes).expect("decode");
            assert_eq!(value, decoded, "round trip mismatch for {name}");
            let hash = Hash::of_cbor(&value).expect("hash");
            assert_eq!(expected_hash, hash.to_hex(), "hash mismatch for {name}");
        }
    }

    #[test]
    fn parse_and_format_round_trip() {
        let original = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let hash = Hash::from_hex_str(original).expect("parse");
        assert_eq!(hash.to_hex(), original);
        assert!(Hash::from_hex_str("0123").is_err());
        assert!(Hash::from_bytes(&[0u8; 31]).is_err());
    }
}
