use crate::artifact::BuildArtifact;
use crate::backends::ModuleCompiler;
use crate::backends::rust::RustBackend;
use crate::cache;
use crate::config::BuildConfig;
use crate::error::BuildError;
use crate::hash::WasmDigest;
use anyhow::Result;
use camino::Utf8PathBuf;
use log::debug;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use toml::Value;
use walkdir::WalkDir;

#[derive(Clone, Copy, Debug)]
pub enum BackendKind {
    Rust,
}

#[derive(Clone, Debug)]
pub struct BuildRequest {
    pub source_dir: Utf8PathBuf,
    pub config: BuildConfig,
    pub backend: BackendKind,
    pub use_cache: bool,
    pub cache_dir: Option<PathBuf>,
}

impl BuildRequest {
    pub fn new(source_dir: impl Into<Utf8PathBuf>) -> Self {
        Self {
            source_dir: source_dir.into(),
            config: BuildConfig::default(),
            backend: BackendKind::Rust,
            use_cache: true,
            cache_dir: None,
        }
    }
}

pub struct Builder;

impl Builder {
    pub fn compile(request: BuildRequest) -> Result<BuildArtifact, BuildError> {
        let fingerprint = build_fingerprint(&request)?;
        let cache_override = request.cache_dir.clone();
        if request.use_cache {
            if let Some(bytes) =
                cache::lookup(&fingerprint, cache_override.as_deref()).map_err(BuildError::Io)?
            {
                let digest = WasmDigest::of_bytes(&bytes);
                debug!("cache hit for reducer (fingerprint {fingerprint})");
                return Ok(BuildArtifact {
                    wasm_bytes: bytes,
                    wasm_hash: digest,
                    build_log: Some("cache hit".into()),
                });
            }
            debug!("cache miss for reducer (fingerprint {fingerprint})");
        } else {
            debug!("cache disabled; building reducer");
        }
        let artifact = match request.backend {
            BackendKind::Rust => RustBackend::new().compile(request)?,
        };
        cache::store(
            &fingerprint,
            &artifact.wasm_bytes,
            cache_override.as_deref(),
        )
        .map_err(BuildError::Io)?;
        Ok(artifact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    #[test]
    fn builds_counter_reducer() {
        let root = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let source = root.join("../../examples/01-hello-timer/reducer");
        let mut request = BuildRequest::new(source);
        request.config.release = false;
        let artifact = Builder::compile(request.clone()).expect("compile hello timer reducer");
        assert!(!artifact.wasm_bytes.is_empty());
        let artifact_cached = Builder::compile(request).expect("compile cached reducer");
        assert_eq!(artifact.wasm_hash.0, artifact_cached.wasm_hash.0);
    }
}

fn build_fingerprint(request: &BuildRequest) -> Result<String, BuildError> {
    let source_hash = hash_directory(&request.source_dir)?;
    let path_dep_hash = hash_path_deps(&request.source_dir)?;
    let inputs = vec![
        ("source", source_hash),
        ("target", request.config.toolchain.target.clone()),
        (
            "profile",
            if request.config.release {
                "release".into()
            } else {
                "debug".into()
            },
        ),
    ];
    let mut inputs = inputs;
    if let Some(hash) = path_dep_hash {
        inputs.push(("path_deps", hash));
    }
    Ok(cache::fingerprint(&inputs))
}

fn hash_directory(path: &Utf8PathBuf) -> Result<String, BuildError> {
    let root = Path::new(path.as_str());
    let mut entries = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !should_skip(e))
    {
        let entry = entry.map_err(|e| BuildError::BuildFailed(e.to_string()))?;
        if entry.file_type().is_file() {
            let rel = entry
                .path()
                .strip_prefix(root)
                .map_err(|e| BuildError::BuildFailed(e.to_string()))?;
            entries.push(rel.to_path_buf());
        }
    }
    entries.sort();
    let mut hasher = Sha256::new();
    for rel in entries {
        hasher.update(rel.to_string_lossy().as_bytes());
        let data = fs::read(root.join(&rel)).map_err(BuildError::Io)?;
        hasher.update(&data);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn hash_path_deps(source_dir: &Utf8PathBuf) -> Result<Option<String>, BuildError> {
    let manifest_path = Path::new(source_dir.as_str()).join("Cargo.toml");
    if !manifest_path.exists() {
        return Ok(None);
    }
    let data = fs::read_to_string(&manifest_path).map_err(BuildError::Io)?;
    let manifest: Value = data
        .parse()
        .map_err(|err| BuildError::BuildFailed(format!("parse Cargo.toml: {err}")))?;
    let mut paths = BTreeSet::new();
    if let Some(table) = manifest.as_table() {
        collect_path_deps(table, &mut paths);
    }
    if paths.is_empty() {
        return Ok(None);
    }

    let base = Path::new(source_dir.as_str());
    let mut hasher = Sha256::new();
    for path_str in paths {
        let dep_path = Path::new(&path_str);
        let full = if dep_path.is_absolute() {
            dep_path.to_path_buf()
        } else {
            base.join(dep_path)
        };
        let full = full.canonicalize().map_err(BuildError::Io)?;
        let utf_path = Utf8PathBuf::from_path_buf(full.clone()).map_err(|_| {
            BuildError::BuildFailed(format!("path is not utf-8: {}", full.display()))
        })?;
        let dep_hash = hash_directory(&utf_path)?;
        hasher.update(path_str.as_bytes());
        hasher.update(b"=");
        hasher.update(dep_hash.as_bytes());
        hasher.update(b"\n");
    }
    Ok(Some(hex::encode(hasher.finalize())))
}

fn collect_path_deps(table: &toml::value::Table, paths: &mut BTreeSet<String>) {
    const SECTIONS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    for section in SECTIONS {
        if let Some(Value::Table(deps)) = table.get(section) {
            for dep in deps.values() {
                if let Some(path) = dep.get("path").and_then(|value| value.as_str()) {
                    paths.insert(path.to_string());
                }
            }
        }
    }
    if let Some(Value::Table(targets)) = table.get("target") {
        for target in targets.values() {
            if let Some(target_table) = target.as_table() {
                collect_path_deps(target_table, paths);
            }
        }
    }
}

fn should_skip(entry: &walkdir::DirEntry) -> bool {
    if entry.depth() == 0 {
        return false;
    }
    matches!(entry.file_name().to_str(), Some("target" | ".git" | ".aos"))
}
