//! Canonical CBOR helpers and hashing (skeleton)

use serde::Serialize;
use sha2::{Digest, Sha256};

pub fn to_canonical_cbor<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_cbor::Error> {
    let mut buf = Vec::with_capacity(128);
    let mut ser = serde_cbor::ser::Serializer::new(&mut buf);
    ser.self_describe();
    ser.canonical();
    value.serialize(&mut ser)?;
    Ok(buf)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Hash(pub [u8; 32]);

impl Hash {
    pub fn of_cbor<T: Serialize>(v: &T) -> Self {
        let bytes = to_canonical_cbor(v).expect("canonical CBOR");
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let out = hasher.finalize();
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&out);
        Hash(arr)
    }
    pub fn to_hex(&self) -> String { format!("sha256:{}", hex::encode(self.0)) }
}

