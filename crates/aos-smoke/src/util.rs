use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_kernel::KernelConfig;
use aos_kernel::secret::{MapSecretResolver, SecretResolver};
use aos_wasm_build::{BuildRequest, Builder};
use camino::Utf8PathBuf;
use log::debug;
use once_cell::sync::OnceCell;

pub use aos_host::util::reset_journal;

fn workspace_root() -> PathBuf {
    PathBuf::from(crate::workspace_root())
}

static FORCE_BUILD: OnceCell<bool> = OnceCell::new();

pub(crate) fn set_force_build(flag: bool) {
    let _ = FORCE_BUILD.set(flag);
}

fn force_build() -> bool {
    FORCE_BUILD.get().copied().unwrap_or(false)
}

pub fn compile_reducer(crate_rel: &str) -> Result<Vec<u8>> {
    let source_path = workspace_root().join(crate_rel);
    let utf_path = Utf8PathBuf::from_path_buf(source_path.clone())
        .map_err(|_| anyhow!("path is not utf-8: {}", source_path.display()))?;
    let example_root = source_path
        .parent()
        .ok_or_else(|| anyhow!("reducer path missing parent: {}", source_path.display()))?;
    let cache_dir = example_root.join(".aos").join("cache").join("modules");
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;
    let mut request = BuildRequest::new(utf_path);
    request.config.release = false;
    request.cache_dir = Some(cache_dir);
    if force_build() {
        debug!("forcing rebuild for {crate_rel}");
        request.use_cache = false;
    }
    let artifact = Builder::compile(request).context("compile reducer via aos-wasm-build")?;
    debug!(
        "build result: {} ({} bytes)",
        hex::encode(&artifact.wasm_hash.0),
        artifact.wasm_bytes.len()
    );
    Ok(artifact.wasm_bytes)
}

pub fn kernel_config(example_root: &Path) -> Result<KernelConfig> {
    let cache_dir = example_root.join(".aos").join("cache").join("wasmtime");
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;
    let secret_resolver = load_secret_resolver();
    Ok(KernelConfig {
        module_cache_dir: Some(cache_dir),
        eager_module_load: true,
        secret_resolver: secret_resolver.clone(),
        allow_placeholder_secrets: false,
    })
}

/// Compile a specific binary in a workspace package to wasm32.
/// Intended for built-in system reducers like sys/Workspace.
pub fn compile_wasm_bin(
    workspace_root: &Path,
    package: &str,
    bin: &str,
    cache_dir: &Path,
) -> Result<Vec<u8>> {
    fs::create_dir_all(cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;
    let target_dir = cache_dir;
    let status = Command::new("cargo")
        .current_dir(workspace_root)
        .args([
            "build",
            "-p",
            package,
            "--bin",
            bin,
            "--target",
            "wasm32-unknown-unknown",
        ])
        .env("CARGO_TARGET_DIR", target_dir)
        .status()
        .map_err(|e| anyhow!("failed to spawn cargo: {e}"))?;
    if !status.success() {
        anyhow::bail!("cargo build -p {package} --bin {bin} failed with status {status}");
    }
    let artifact = target_dir
        .join("wasm32-unknown-unknown")
        .join("debug")
        .join(format!("{bin}.wasm"));
    let bytes = fs::read(&artifact)
        .with_context(|| format!("read wasm artifact {}", artifact.display()))?;
    Ok(bytes)
}

fn load_secret_resolver() -> Option<Arc<dyn SecretResolver>> {
    const DEMO_LLM_API_KEY: &str = "demo-llm-api-key";
    let mut map: HashMap<String, Vec<u8>> = HashMap::new();

    let llm_api_key = env_or_dotenv_var("LLM_API_KEY").unwrap_or_else(|| DEMO_LLM_API_KEY.into());
    map.insert("env:LLM_API_KEY".to_string(), llm_api_key.into_bytes());

    if let Some(openai_key) = env_or_dotenv_var("OPENAI_API_KEY") {
        map.insert("env:OPENAI_API_KEY".to_string(), openai_key.into_bytes());
    }
    if let Some(anthropic_key) = env_or_dotenv_var("ANTHROPIC_API_KEY") {
        map.insert(
            "env:ANTHROPIC_API_KEY".to_string(),
            anthropic_key.into_bytes(),
        );
    }

    Some(Arc::new(MapSecretResolver::new(map)))
}

fn env_or_dotenv_var(key: &str) -> Option<String> {
    if let Ok(value) = std::env::var(key) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    for path in dotenv_candidates() {
        let Ok(contents) = fs::read_to_string(path) else {
            continue;
        };
        if let Some(value) = parse_dotenv_value(&contents, key) {
            return Some(value);
        }
    }
    None
}

fn dotenv_candidates() -> Vec<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    vec![
        workspace_root().join(".env"),
        manifest_dir.join(".env"),
        PathBuf::from(".env"),
    ]
}

fn parse_dotenv_value(contents: &str, key: &str) -> Option<String> {
    for raw_line in contents.lines() {
        let mut line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(stripped) = line.strip_prefix("export ") {
            line = stripped.trim();
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != key {
            continue;
        }
        let value = value.trim();
        let unquoted = if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
            || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        if !unquoted.is_empty() {
            return Some(unquoted.to_string());
        }
    }
    None
}
