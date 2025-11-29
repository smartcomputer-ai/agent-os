//! Canonical CBOR helpers and stable SHA-256 hashing utilities used across AgentOS.

use serde::Serialize;
use serde_cbor::{ser::Write as CborWrite, value::Value as CborValue};
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
        let rest = s
            .strip_prefix(HASH_PREFIX)
            .ok_or(HashParseError::MissingPrefix)?;
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
    use serde_json::{Value, json};
    use std::{fs, path::PathBuf};

    fn load_spec(relative: &str) -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../")
            .join(relative);
        let data = fs::read_to_string(path).expect("schema file");
        serde_json::from_str(&data).expect("valid json")
    }

    #[test]
    fn canonical_round_trip_and_hashes() {
        let cases = [
            (
                "spec/schemas/common.schema.json",
                "sha256:fbec3ac2624e8b4c5e75b086840267c3647c370dbdfd099da820fdfddf783124",
            ),
            (
                "spec/schemas/defschema.schema.json",
                "sha256:492c7e9583481d3060bf444efb82f7263434f49f85aab238504152ff8ec1115c",
            ),
            (
                "spec/schemas/defmodule.schema.json",
                "sha256:9db4dc958903289dde3e63efc4ebc91e5c4a00fd4a910a0a90b69efa9cb006eb",
            ),
            (
                "spec/schemas/defplan.schema.json",
                "sha256:17e4c5c9eec58d537c4e95b340c5afd046228eb45b4240a936b89d06fb58b3a5",
            ),
            (
                "spec/schemas/defcap.schema.json",
                "sha256:860a76e1de58d91d17de70d234eb83123cee4e4bca64fb9a7596e1e39fcd3af7",
            ),
            (
                "spec/schemas/defpolicy.schema.json",
                "sha256:c8146b00718449e9d49ddfa70589d848202bd5bde695b656a088c8f39481d843",
            ),
            (
                "spec/schemas/manifest.schema.json",
                "sha256:b7bbdf2f686803fb87b99c182afa0fe3fc725c97eb0001fa56ac84f645623cdc",
            ),
            (
                "spec/schemas/defsecret.schema.json",
                "sha256:2ff01ee39376e4bc37722bf1ceb763d4412f142be325903513dfaaf8e7a729f7",
            ),
            (
                "spec/defs/builtin-schemas.air.json",
                "sha256:c4c2a4b0bf29c03afd4c7731bf8ac3fceee8a552580247500cfdaef7d0bd77ab",
            ),
        ];

        for (name, expected_hash) in cases {
            let value = load_spec(name);
            let bytes = to_canonical_cbor(&value).expect("canonical encode");
            let decoded: Value = serde_cbor::from_slice(&bytes).expect("decode");
            assert_eq!(value, decoded, "round trip mismatch for {name}");
            let hash = Hash::of_cbor(&value).expect("hash");
            assert_eq!(expected_hash, hash.to_hex(), "hash mismatch for {name}");
        }
    }

    #[derive(serde::Deserialize)]
    struct Vector {
        label: String,
        json: Value,
        cbor_hex: String,
        hash: String,
    }

    fn load_vectors(relative: &str) -> Vec<Vector> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../")
            .join(relative);
        let data = fs::read_to_string(path).expect("vector file");
        serde_json::from_str(&data).expect("valid vectors json")
    }

    #[test]
    fn spec_test_vectors_match_hashes() {
        for file in [
            "spec/test-vectors/canonical-cbor.json",
            "spec/test-vectors/schemas.json",
            "spec/test-vectors/plans.json",
        ] {
            for vector in load_vectors(file) {
                let bytes = to_canonical_cbor(&vector.json).expect("canonical cbor");
                assert_eq!(
                    hex::encode(&bytes),
                    vector.cbor_hex,
                    "cbor hex mismatch for {} in {}",
                    vector.label,
                    file
                );
                assert_eq!(
                    Hash::of_bytes(&bytes).to_hex(),
                    vector.hash,
                    "hash mismatch for {} in {}",
                    vector.label,
                    file
                );
            }
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

    #[test]
    fn hash_is_order_insensitive_for_maps() {
        let alpha_first = json!({"a": 1, "b": 2});
        let beta_first = json!({"b": 2, "a": 1});
        let hash1 = Hash::of_cbor(&alpha_first).expect("hash");
        let hash2 = Hash::of_cbor(&beta_first).expect("hash");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn canonical_serializer_orders_map_keys() {
        let shuffled = json!({"b": 1, "a": {"inner": 2}});
        let mut buf = Vec::new();
        write_canonical_cbor(&shuffled, &mut buf).expect("serialize");
        let decoded: serde_cbor::Value = serde_cbor::from_slice(&buf).expect("decode");
        let serde_cbor::Value::Map(entries) = decoded else {
            panic!("expected CBOR map");
        };
        let keys: Vec<String> = entries
            .iter()
            .map(|(key, _)| match key {
                serde_cbor::Value::Text(text) => text.clone(),
                other => panic!("unexpected key {:?}", other),
            })
            .collect();
        assert_eq!(keys, vec!["a", "b"], "map keys must be sorted canonically");
    }
}
