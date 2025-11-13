use sha2::{Digest, Sha256};
use std::io::Read;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WasmDigest(pub [u8; 32]);

impl WasmDigest {
    pub fn of_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&digest);
        WasmDigest(arr)
    }

    pub fn of_reader(mut reader: impl Read) -> std::io::Result<Self> {
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let digest = hasher.finalize();
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&digest);
        Ok(WasmDigest(arr))
    }
}
