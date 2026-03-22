use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_air_types::HashRef;
use aos_authoring::{
    WorldBundle, build_world_harness_from_bundle, local_state_paths, resolve_placeholder_modules,
};
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_kernel::Store;
use aos_kernel::{Kernel, KernelConfig, LoadedManifest};
use aos_node::FsCas;
use aos_node::{EmbeddedWorldHarness, LocalKernelGuard};
use aos_runtime::manifest_loader;
use aos_runtime::util::patch_modules;
use aos_runtime::{EffectMode, WorldConfig};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub struct EvalHost {
    host: EmbeddedWorldHarness,
    workflow_name: String,
    event_schema: String,
}

pub enum EvalModuleBuild<'a> {
    CargoBin {
        package: &'a str,
        bin: &'a str,
    },
    CargoManifestLib {
        manifest_path: &'a Path,
        artifact_stem: &'a str,
    },
}

pub struct EvalModulePatch<'a> {
    pub module_name: &'a str,
    pub build: EvalModuleBuild<'a>,
}

pub struct EvalHostConfig<'a> {
    pub world_root: &'a Path,
    pub assets_root: &'a Path,
    pub import_roots: &'a [PathBuf],
    pub workspace_root: &'a Path,
    pub workflow_name: &'a str,
    pub event_schema: &'a str,
    pub world_config: WorldConfig,
    pub adapter_config: EffectAdapterConfig,
    pub module_patches: &'a [EvalModulePatch<'a>],
}

impl EvalHost {
    pub fn prepare(cfg: EvalHostConfig<'_>) -> Result<Self> {
        let paths = local_state_paths(cfg.world_root);
        paths
            .reset_runtime_state()
            .context("reset local runtime state")?;
        let module_cache = paths.module_cache_dir();
        fs::create_dir_all(&module_cache)
            .with_context(|| format!("create cache dir {}", module_cache.display()))?;

        let store = Arc::new(FsCas::open_with_paths(&paths).context("open local CAS")?);
        let mut assets = load_manifest_assets(store.clone(), cfg.assets_root, cfg.import_roots)?;

        if cfg.module_patches.is_empty() {
            anyhow::bail!("EvalHostConfig.module_patches must not be empty");
        }

        let mut build_cache = HashMap::<String, HashRef>::new();
        for patch in cfg.module_patches {
            let cache_key = module_build_cache_key(&patch.build);
            let wasm_hash_ref = if let Some(existing) = build_cache.get(&cache_key) {
                existing.clone()
            } else {
                let wasm_bytes = match &patch.build {
                    EvalModuleBuild::CargoBin { package, bin } => {
                        compile_wasm_bin(cfg.workspace_root, package, bin, &module_cache)
                            .with_context(|| {
                                format!("compile {} --bin {} for eval workflow patch", package, bin)
                            })?
                    }
                    EvalModuleBuild::CargoManifestLib {
                        manifest_path,
                        artifact_stem,
                    } => compile_wasm_manifest_lib(
                        cfg.workspace_root,
                        manifest_path,
                        artifact_stem,
                        &module_cache,
                    )
                    .with_context(|| {
                        format!(
                            "compile manifest {} for eval workflow patch",
                            manifest_path.display()
                        )
                    })?,
                };

                let wasm_hash = store
                    .put_blob(&wasm_bytes)
                    .context("store eval workflow wasm")?;
                let wasm_hash_ref =
                    HashRef::new(wasm_hash.to_hex()).context("hash eval workflow wasm")?;
                build_cache.insert(cache_key, wasm_hash_ref.clone());
                wasm_hash_ref
            };

            patch_module_hash(&mut assets.loaded, patch.module_name, &wasm_hash_ref)?;
        }

        let paths = local_state_paths(cfg.world_root);
        resolve_placeholder_modules(
            &mut assets.loaded,
            store.as_ref(),
            cfg.world_root,
            &paths,
            None,
            None,
        )?;

        let kernel_config = kernel_config(cfg.world_root)?;
        let bundle = WorldBundle::from_loaded_assets(assets.loaded, assets.secrets);
        let host = build_world_harness_from_bundle(
            Arc::clone(&store),
            bundle,
            Some(&paths),
            cfg.world_config,
            cfg.adapter_config,
            kernel_config,
            EffectMode::Scripted,
        )?;

        Ok(Self {
            host,
            workflow_name: cfg.workflow_name.to_string(),
            event_schema: cfg.event_schema.to_string(),
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
        self.host.run_until_kernel_idle().context("drain kernel")?;
        Ok(())
    }

    pub fn read_state_for_session<T: DeserializeOwned>(&self, session_id: &str) -> Result<T> {
        let mut matched: Option<Vec<u8>> = None;
        for entry in self.host.list_cells(&self.workflow_name)? {
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
            .state_bytes(&self.workflow_name, Some(&key_bytes))
            .ok_or_else(|| anyhow!("missing keyed workflow state bytes"))?;
        serde_cbor::from_slice(&bytes).context("decode keyed workflow state")
    }

    pub fn kernel_mut(&mut self) -> LocalKernelGuard<'_> {
        self.host
            .kernel_mut()
            .expect("embedded eval harness kernel_mut")
    }

    pub fn store(&self) -> Arc<FsCas> {
        self.host.store()
    }

    pub fn with_kernel_mut<R>(
        &mut self,
        f: impl FnOnce(&mut Kernel<FsCas>) -> Result<R, aos_kernel::KernelError>,
    ) -> Result<R> {
        self.host.with_kernel_mut(f).map_err(Into::into)
    }

    pub fn execute_batch_routed(
        &mut self,
        intents: Vec<(aos_effects::EffectIntent, String)>,
    ) -> Result<Vec<aos_effects::EffectReceipt>> {
        self.host.execute_batch_routed(intents).map_err(Into::into)
    }

    pub fn execute_single_routed(
        &mut self,
        intent: aos_effects::EffectIntent,
        route_id: String,
    ) -> Result<aos_effects::EffectReceipt> {
        self.host
            .execute_single_routed(intent, route_id)
            .map_err(Into::into)
    }
}

fn module_build_cache_key(build: &EvalModuleBuild<'_>) -> String {
    match build {
        EvalModuleBuild::CargoBin { package, bin } => format!("bin:{package}:{bin}"),
        EvalModuleBuild::CargoManifestLib {
            manifest_path,
            artifact_stem,
        } => format!("lib:{}:{artifact_stem}", manifest_path.display()),
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
        cell_cache_size: aos_kernel::world::DEFAULT_CELL_CACHE_SIZE,
        universe_id: uuid::Uuid::nil(),
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

fn compile_wasm_manifest_lib(
    workspace_root: &Path,
    manifest_path: &Path,
    artifact_stem: &str,
    cache_dir: &Path,
) -> Result<Vec<u8>> {
    fs::create_dir_all(cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;

    let status = Command::new("cargo")
        .current_dir(workspace_root)
        .args([
            "build",
            "--manifest-path",
            manifest_path
                .to_str()
                .ok_or_else(|| anyhow!("manifest path is not valid UTF-8"))?,
            "--target",
            "wasm32-unknown-unknown",
        ])
        .env("CARGO_TARGET_DIR", cache_dir)
        .status()
        .map_err(|err| anyhow!("failed to spawn cargo build: {err}"))?;

    if !status.success() {
        anyhow::bail!(
            "cargo build --manifest-path {} failed with status {status}",
            manifest_path.display()
        );
    }

    let artifact = cache_dir
        .join("wasm32-unknown-unknown")
        .join("debug")
        .join(format!("{artifact_stem}.wasm"));
    fs::read(&artifact).with_context(|| format!("read wasm artifact {}", artifact.display()))
}

fn patch_module_hash(
    loaded: &mut LoadedManifest,
    module_name: &str,
    wasm_hash: &HashRef,
) -> Result<()> {
    let patched = patch_modules(loaded, wasm_hash, |name, _| name == module_name);
    if patched == 0 {
        anyhow::bail!("module '{module_name}' missing from manifest");
    }
    Ok(())
}

fn load_manifest_assets<S: Store + 'static>(
    store: Arc<S>,
    assets_root: &Path,
    import_roots: &[PathBuf],
) -> Result<manifest_loader::LoadedAssets> {
    manifest_loader::load_from_assets_with_imports_and_defs(store, assets_root, import_roots)
        .context("load manifest from eval assets")?
        .ok_or_else(|| anyhow!("eval manifest missing at {}", assets_root.display()))
}
