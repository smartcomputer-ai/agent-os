use crate::artifact::BuildArtifact;
use crate::backends::ModuleCompiler;
use crate::backends::rust::RustBackend;
use crate::config::BuildConfig;
use crate::error::BuildError;
use anyhow::Result;
use camino::Utf8PathBuf;

#[derive(Clone, Copy, Debug)]
pub enum BackendKind {
    Rust,
}

#[derive(Clone, Debug)]
pub struct BuildRequest {
    pub source_dir: Utf8PathBuf,
    pub config: BuildConfig,
    pub backend: BackendKind,
}

impl BuildRequest {
    pub fn new(source_dir: impl Into<Utf8PathBuf>) -> Self {
        Self {
            source_dir: source_dir.into(),
            config: BuildConfig::default(),
            backend: BackendKind::Rust,
        }
    }
}

pub struct Builder;

impl Builder {
    pub fn compile(request: BuildRequest) -> Result<BuildArtifact, BuildError> {
        match request.backend {
            BackendKind::Rust => RustBackend::new().compile(request),
        }
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
        let artifact = Builder::compile(request).expect("compile hello timer reducer");
        assert!(!artifact.wasm_bytes.is_empty());
    }
}
