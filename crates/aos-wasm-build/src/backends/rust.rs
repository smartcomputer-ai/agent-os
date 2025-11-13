use crate::artifact::BuildArtifact;
use crate::builder::BuildRequest;
use crate::error::BuildError;
use crate::hash::WasmDigest;
use crate::util::resolve_cargo;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

pub struct RustBackend;

impl RustBackend {
    pub fn new() -> Self {
        Self
    }

    fn run_cargo(&self, request: &BuildRequest, temp_out: &TempDir) -> Result<PathBuf, BuildError> {
        let cargo = resolve_cargo().map_err(|e| BuildError::CargoNotFound(e.to_string()))?;
        let mut cmd = Command::new(cargo);
        cmd.current_dir(&request.source_dir);
        cmd.arg("build");
        cmd.arg("--target");
        cmd.arg(&request.config.toolchain.target);
        if request.config.release {
            cmd.arg("--release");
        }
        cmd.env("CARGO_TARGET_DIR", temp_out.path());
        let output = cmd
            .output()
            .map_err(|e| BuildError::BuildFailed(e.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            return Err(BuildError::BuildFailed(stderr));
        }
        let profile_dir = temp_out.path().join(&request.config.toolchain.target).join(
            if request.config.release {
                "release"
            } else {
                "debug"
            },
        );
        let wasm_path = find_wasm_artifact(&profile_dir)?;
        Ok(wasm_path)
    }
}

impl crate::backends::ModuleCompiler for RustBackend {
    fn compile(&self, request: BuildRequest) -> Result<BuildArtifact, BuildError> {
        let temp_out = TempDir::new().map_err(BuildError::Io)?;
        let wasm_path = self.run_cargo(&request, &temp_out)?;
        let wasm_bytes = fs::read(&wasm_path).map_err(BuildError::Io)?;
        let digest = WasmDigest::of_bytes(&wasm_bytes);
        Ok(BuildArtifact {
            wasm_bytes,
            wasm_hash: digest,
            build_log: None,
        })
    }
}

fn find_wasm_artifact(dir: &Path) -> Result<PathBuf, BuildError> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(dir).map_err(BuildError::Io)? {
        let entry = entry.map_err(BuildError::Io)?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("wasm") {
            candidates.push(path);
        }
    }
    candidates
        .into_iter()
        .next()
        .ok_or_else(|| BuildError::ArtifactNotFound(dir.to_path_buf()))
}
