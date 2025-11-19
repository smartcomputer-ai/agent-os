use crate::hash::WasmDigest;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct BuildArtifact {
    pub wasm_bytes: Vec<u8>,
    pub wasm_hash: WasmDigest,
    pub build_log: Option<String>,
}

impl BuildArtifact {
    pub fn write_to(&self, output: impl AsRef<Path>) -> std::io::Result<()> {
        fs::write(output, &self.wasm_bytes)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.wasm_bytes
    }
}
