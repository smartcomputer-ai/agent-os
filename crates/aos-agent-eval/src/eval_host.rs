use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_air_types::HashRef;
use aos_host::config::HostConfig;
use aos_host::host::WorldHost;
use aos_host::manifest_loader;
use aos_host::testhost::TestHost;
use aos_host::util::{is_placeholder_hash, patch_modules, reset_journal};
use aos_kernel::cell_index::CellIndex;
use aos_kernel::{Kernel, KernelConfig, LoadedManifest};
use aos_store::{FsStore, Store};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::runtime::{Builder, Runtime};

pub struct EvalHost {
    host: TestHost<FsStore>,
    workflow_name: String,
    event_schema: String,
    store: Arc<FsStore>,
    runtime: Runtime,
}

pub struct EvalHostConfig<'a> {
    pub world_root: &'a Path,
    pub assets_root: &'a Path,
    pub import_roots: &'a [PathBuf],
    pub workspace_root: &'a Path,
    pub workflow_name: &'a str,
    pub event_schema: &'a str,
    pub host_config: HostConfig,
    pub module_package: &'a str,
    pub module_bin: &'a str,
}

impl EvalHost {
    pub fn prepare(cfg: EvalHostConfig<'_>) -> Result<Self> {
        reset_journal(cfg.world_root)?;

        let module_cache = cfg
            .workspace_root
            .join("target")
            .join("aos-agent-eval")
            .join("cache")
            .join("modules");
        let wasm_bytes = compile_wasm_bin(
            cfg.workspace_root,
            cfg.module_package,
            cfg.module_bin,
            &module_cache,
        )
        .with_context(|| {
            format!(
                "compile {} --bin {} for eval workflow patch",
                cfg.module_package, cfg.module_bin
            )
        })?;

        let store = Arc::new(FsStore::open(cfg.world_root).context("open eval FsStore")?);
        let wasm_hash = store
            .put_blob(&wasm_bytes)
            .context("store eval workflow wasm")?;
        let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).context("hash eval workflow wasm")?;

        let mut loaded = load_and_patch(
            store.clone(),
            cfg.assets_root,
            cfg.import_roots,
            cfg.workflow_name,
            &wasm_hash_ref,
        )?;

        let mut sys_module_cache = HashMap::new();
        maybe_patch_sys_enforcers(
            cfg.workspace_root,
            store.clone(),
            &mut loaded,
            &mut sys_module_cache,
        )?;

        let kernel_config = kernel_config(cfg.world_root)?;
        let world_host = WorldHost::from_loaded_manifest(
            store.clone(),
            loaded,
            cfg.world_root,
            cfg.host_config,
            kernel_config,
        )?;
        let host = TestHost::from_world_host(world_host);
        let runtime = Builder::new_current_thread().enable_all().build()?;

        Ok(Self {
            host,
            workflow_name: cfg.workflow_name.to_string(),
            event_schema: cfg.event_schema.to_string(),
            store,
            runtime,
        })
    }

    pub fn send_event<T: Serialize>(&mut self, event: &T) -> Result<()> {
        let cbor = serde_cbor::to_vec(event).context("encode event cbor")?;
        self.host
            .send_event_cbor(&self.event_schema, cbor)
            .context("enqueue event")?;
        self.run_to_idle()
    }

    pub fn run_to_idle(&mut self) -> Result<()> {
        self.host.run_to_idle().context("drain kernel")
    }

    pub fn read_state_for_session<T: DeserializeOwned>(&self, session_id: &str) -> Result<T> {
        let root = self
            .host
            .kernel()
            .workflow_index_root(&self.workflow_name)
            .ok_or_else(|| anyhow!("missing keyed index for workflow '{}'", self.workflow_name))?;
        let index = CellIndex::new(self.store.as_ref());
        let mut matched: Option<Vec<u8>> = None;
        for entry in index.iter(root) {
            let entry = entry?;
            let Ok(candidate) = serde_cbor::from_slice::<String>(&entry.key_bytes) else {
                continue;
            };
            if candidate == session_id {
                if matched.is_some() {
                    anyhow::bail!(
                        "workflow '{}' has duplicate keyed state for session '{}'",
                        self.workflow_name,
                        session_id
                    );
                }
                matched = Some(entry.key_bytes);
            }
        }
        let key_bytes = matched.ok_or_else(|| {
            anyhow!(
                "workflow '{}' has no state for session '{}'",
                self.workflow_name,
                session_id
            )
        })?;
        let bytes = self
            .host
            .kernel()
            .workflow_state_bytes(&self.workflow_name, Some(&key_bytes))?
            .ok_or_else(|| anyhow!("missing keyed workflow state bytes"))?;
        serde_cbor::from_slice(&bytes).context("decode keyed workflow state")
    }

    pub fn kernel_mut(&mut self) -> &mut Kernel<FsStore> {
        self.host.kernel_mut()
    }

    pub fn store(&self) -> Arc<FsStore> {
        self.store.clone()
    }

    pub fn adapter_registry(&self) -> &aos_host::adapters::registry::AdapterRegistry {
        self.host.adapter_registry()
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }
}

fn kernel_config(world_root: &Path) -> Result<KernelConfig> {
    let cache_dir = world_root.join(".aos").join("cache").join("wasmtime");
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;
    Ok(KernelConfig {
        module_cache_dir: Some(cache_dir),
        eager_module_load: true,
        secret_resolver: None,
        allow_placeholder_secrets: false,
    })
}

fn compile_wasm_bin(
    workspace_root: &Path,
    package: &str,
    bin: &str,
    cache_dir: &Path,
) -> Result<Vec<u8>> {
    fs::create_dir_all(cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;

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
        .env("CARGO_TARGET_DIR", cache_dir)
        .status()
        .map_err(|err| anyhow!("failed to spawn cargo build: {err}"))?;

    if !status.success() {
        anyhow::bail!("cargo build -p {package} --bin {bin} failed with status {status}");
    }

    let artifact = cache_dir
        .join("wasm32-unknown-unknown")
        .join("debug")
        .join(format!("{bin}.wasm"));
    fs::read(&artifact).with_context(|| format!("read wasm artifact {}", artifact.display()))
}

fn patch_module_hash(
    loaded: &mut LoadedManifest,
    workflow_name: &str,
    wasm_hash: &HashRef,
) -> Result<()> {
    let patched = patch_modules(loaded, wasm_hash, |name, _| name == workflow_name);
    if patched == 0 {
        anyhow::bail!("module '{workflow_name}' missing from manifest");
    }
    Ok(())
}

fn load_and_patch(
    store: Arc<FsStore>,
    assets_root: &Path,
    import_roots: &[PathBuf],
    workflow_name: &str,
    wasm_hash: &HashRef,
) -> Result<LoadedManifest> {
    let mut loaded =
        manifest_loader::load_from_assets_with_imports(store, assets_root, import_roots)
            .context("load manifest from eval assets")?
            .ok_or_else(|| anyhow!("eval manifest missing at {}", assets_root.display()))?;
    patch_module_hash(&mut loaded, workflow_name, wasm_hash)?;
    Ok(loaded)
}

fn maybe_patch_sys_enforcers(
    workspace_root: &Path,
    store: Arc<FsStore>,
    loaded: &mut LoadedManifest,
    cache: &mut HashMap<&'static str, HashRef>,
) -> Result<()> {
    for (module_name, bin_name) in [
        ("sys/Workspace@1", "workspace"),
        ("sys/HttpPublish@1", "http_publish"),
        ("sys/CapEnforceHttpOut@1", "cap_enforce_http_out"),
        ("sys/CapEnforceLlmBasic@1", "cap_enforce_llm_basic"),
        ("sys/CapEnforceWorkspace@1", "cap_enforce_workspace"),
    ] {
        maybe_patch_sys_module(
            workspace_root,
            store.clone(),
            loaded,
            cache,
            module_name,
            bin_name,
        )?;
    }
    Ok(())
}

fn maybe_patch_sys_module(
    workspace_root: &Path,
    store: Arc<FsStore>,
    loaded: &mut LoadedManifest,
    cache: &mut HashMap<&'static str, HashRef>,
    module_name: &'static str,
    bin_name: &'static str,
) -> Result<()> {
    let needs_patch = loaded
        .modules
        .get(module_name)
        .map(is_placeholder_hash)
        .unwrap_or(false);
    if !needs_patch {
        return Ok(());
    }

    let wasm_hash_ref = if let Some(existing) = cache.get(module_name) {
        existing.clone()
    } else {
        let cache_dir = workspace_root
            .join("target")
            .join("aos-agent-eval")
            .join("cache")
            .join("modules");
        let wasm_bytes = compile_wasm_bin(workspace_root, "aos-sys", bin_name, &cache_dir)?;
        let wasm_hash = store
            .put_blob(&wasm_bytes)
            .with_context(|| format!("store {module_name} wasm blob"))?;
        let wasm_hash_ref =
            HashRef::new(wasm_hash.to_hex()).with_context(|| format!("hash {module_name}"))?;
        cache.insert(module_name, wasm_hash_ref.clone());
        wasm_hash_ref
    };

    let patched = patch_modules(loaded, &wasm_hash_ref, |name, _| name == module_name);
    if patched == 0 {
        anyhow::bail!("module '{module_name}' missing in manifest");
    }
    Ok(())
}
