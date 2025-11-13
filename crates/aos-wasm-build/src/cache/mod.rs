use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

const CACHE_DIR: &str = ".aos/cache/modules";

pub fn cache_root() -> PathBuf {
    if let Ok(dir) = std::env::var("AOS_WASM_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join(CACHE_DIR))
        .unwrap_or_else(|| PathBuf::from(CACHE_DIR))
}

pub fn fingerprint(inputs: &[(&str, String)]) -> String {
    let mut hasher = Sha256::new();
    for (k, v) in inputs {
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
        hasher.update(b"\n");
    }
    hex::encode(hasher.finalize())
}

pub fn lookup(fingerprint: &str) -> std::io::Result<Option<Vec<u8>>> {
    let path = cache_root().join(fingerprint).join("artifact.wasm");
    if path.exists() {
        Ok(Some(fs::read(path)?))
    } else {
        Ok(None)
    }
}

pub fn store(fingerprint: &str, bytes: &[u8]) -> std::io::Result<()> {
    let dir = cache_root().join(fingerprint);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("artifact.wasm"), bytes)?;
    Ok(())
}
