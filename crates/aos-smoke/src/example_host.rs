use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use aos_air_types::HashRef;
use aos_authoring::{
    WorldBundle, build_world_harness_from_bundle, local_state_paths, resolve_placeholder_modules,
};
use aos_cbor::Hash;
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_kernel::Kernel;
use aos_kernel::LoadedManifest;
use aos_kernel::Store;
use aos_node::FsCas;
use aos_node::{EmbeddedWorldHarness, LocalKernelGuard};
use aos_runtime::manifest_loader;
use aos_runtime::util::patch_modules;
use aos_runtime::{CycleOutcome, EffectMode, QuiescenceStatus, WorldConfig};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::util;

pub struct HarnessConfig<'a> {
    pub example_root: &'a Path,
    pub assets_root: Option<&'a Path>,
    pub workflow_name: &'a str,
    pub event_schema: &'a str,
    pub module_crate: &'a str,
}

#[derive(Debug, Clone)]
pub struct ExampleHostConfig {
    pub world: WorldConfig,
    pub adapters: EffectAdapterConfig,
    pub effect_mode: EffectMode,
}

impl Default for ExampleHostConfig {
    fn default() -> Self {
        Self {
            world: WorldConfig::default(),
            adapters: EffectAdapterConfig::default(),
            effect_mode: EffectMode::Scripted,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EventDispatchTiming {
    pub encode: Duration,
    pub submit: Duration,
    pub drain: Duration,
}

impl EventDispatchTiming {
    pub fn total(self) -> Duration {
        self.encode + self.submit + self.drain
    }
}

/// Host-backed example driver built on the shared world harness.
pub struct ExampleHost {
    host: EmbeddedWorldHarness,
    workflow_name: String,
    event_schema: String,
    store: Arc<FsCas>,
    wasm_hash: HashRef,
}

impl ExampleHost {
    pub fn prepare(cfg: HarnessConfig<'_>) -> Result<Self> {
        Self::prepare_with_imports_and_host_config(cfg, &[], None)
    }

    pub fn prepare_with_imports(cfg: HarnessConfig<'_>, import_roots: &[PathBuf]) -> Result<Self> {
        Self::prepare_with_imports_and_host_config(cfg, import_roots, None)
    }

    pub fn prepare_with_host_config(
        cfg: HarnessConfig<'_>,
        host_config: ExampleHostConfig,
    ) -> Result<Self> {
        Self::prepare_with_imports_and_host_config(cfg, &[], Some(host_config))
    }

    pub fn prepare_with_imports_and_host_config(
        cfg: HarnessConfig<'_>,
        import_roots: &[PathBuf],
        host_config_override: Option<ExampleHostConfig>,
    ) -> Result<Self> {
        util::reset_runtime_state(cfg.example_root)?;
        let wasm_bytes = util::compile_workflow(cfg.module_crate)?;
        Self::prepare_with_wasm_bytes(cfg, import_roots, host_config_override, wasm_bytes)
    }

    pub fn prepare_with_imports_host_config_and_module_bin(
        cfg: HarnessConfig<'_>,
        import_roots: &[PathBuf],
        host_config_override: Option<ExampleHostConfig>,
        package: &str,
        bin: &str,
    ) -> Result<Self> {
        util::reset_runtime_state(cfg.example_root)?;
        let cache_dir = util::local_state_paths(cfg.example_root).module_cache_dir();
        let wasm_bytes = util::compile_wasm_bin(crate::workspace_root(), package, bin, &cache_dir)
            .with_context(|| format!("compile {package} --bin {bin} for workflow patch"))?;
        Self::prepare_with_wasm_bytes(cfg, import_roots, host_config_override, wasm_bytes)
    }

    fn prepare_with_wasm_bytes(
        cfg: HarnessConfig<'_>,
        import_roots: &[PathBuf],
        host_config_override: Option<ExampleHostConfig>,
        wasm_bytes: Vec<u8>,
    ) -> Result<Self> {
        let paths = local_state_paths(cfg.example_root);
        paths.ensure_root().context("create local state root")?;
        let store = Arc::new(FsCas::open_with_paths(&paths).context("open local CAS")?);
        let wasm_hash = store
            .put_blob(&wasm_bytes)
            .context("store workflow wasm blob")?;
        let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).context("hash workflow wasm")?;

        let assets_root = cfg.assets_root.unwrap_or(cfg.example_root).to_path_buf();

        let mut assets_host = load_and_patch_assets(
            store.clone(),
            &assets_root,
            import_roots,
            cfg.workflow_name,
            &wasm_hash_ref,
        )?;
        let paths = local_state_paths(cfg.example_root);
        resolve_placeholder_modules(
            &mut assets_host.loaded,
            store.as_ref(),
            cfg.example_root,
            &paths,
            None,
            None,
        )?;

        let host_config = host_config_override.unwrap_or_default();
        let kernel_config = util::kernel_config(cfg.example_root)?;
        let bundle_host = WorldBundle::from_loaded_assets(assets_host.loaded, assets_host.secrets);
        let host = build_world_harness_from_bundle(
            Arc::clone(&store),
            bundle_host,
            Some(&paths),
            host_config.world,
            host_config.adapters,
            kernel_config.clone(),
            host_config.effect_mode,
        )?;

        Ok(Self {
            host,
            workflow_name: cfg.workflow_name.to_string(),
            event_schema: cfg.event_schema.to_string(),
            store,
            wasm_hash: wasm_hash_ref,
        })
    }

    pub fn send_event<T: Serialize>(&mut self, event: &T) -> Result<()> {
        let schema = self.event_schema.clone();
        self.send_event_as(&schema, event)
    }

    pub fn send_event_timed<T: Serialize>(&mut self, event: &T) -> Result<EventDispatchTiming> {
        let schema = self.event_schema.clone();
        self.send_event_as_timed(&schema, event)
    }

    pub fn send_event_as<T: Serialize>(&mut self, schema: &str, event: &T) -> Result<()> {
        self.send_event_as_timed(schema, event).map(|_| ())
    }

    pub fn send_event_as_timed<T: Serialize>(
        &mut self,
        schema: &str,
        event: &T,
    ) -> Result<EventDispatchTiming> {
        let encode_start = Instant::now();
        let cbor = serde_cbor::to_vec(event)?;
        let encode = encode_start.elapsed();

        let submit_start = Instant::now();
        self.host
            .send_event_cbor(schema, cbor)
            .context("send event")?;
        let submit = submit_start.elapsed();

        // Mirror the previous harness behavior: advance workflow immediately.
        let drain_start = Instant::now();
        self.host
            .run_until_kernel_idle()
            .context("drain after event")?;
        let drain = drain_start.elapsed();

        Ok(EventDispatchTiming {
            encode,
            submit,
            drain,
        })
    }

    pub fn run_cycle_batch(&mut self) -> Result<CycleOutcome> {
        let outcome = self.host.run_cycle_batch().context("run cycle batch")?;
        Ok(outcome)
    }

    pub fn run_cycle_with_timers(&mut self) -> Result<CycleOutcome> {
        let outcome = self
            .host
            .run_cycle_with_timers()
            .context("run cycle with timers")?;
        Ok(outcome)
    }

    pub fn quiescence_status(&self) -> QuiescenceStatus {
        self.host
            .quiescence_status()
            .expect("embedded world harness quiescence")
    }

    pub fn time_set(&mut self, now_ns: u64) -> u64 {
        self.host
            .time_set(now_ns)
            .expect("embedded world harness time_set")
    }

    pub fn time_jump_next_due(&mut self) -> Result<Option<u64>> {
        self.host
            .time_jump_next_due()
            .context("jump next due timer")
    }

    pub fn drain_effects(&mut self) -> Result<Vec<aos_effects::EffectIntent>> {
        self.host.pull_effects().context("drain effects")
    }

    pub fn apply_receipt(&mut self, receipt: aos_effects::EffectReceipt) -> Result<()> {
        self.host.apply_receipt(receipt).context("apply receipt")
    }

    pub fn read_state<T: DeserializeOwned>(&self) -> Result<T> {
        if let Ok(state) = self.host.state(&self.workflow_name, None) {
            return Ok(state);
        }
        self.read_single_keyed_state()
    }

    pub fn single_keyed_cell_key(&self) -> Result<Vec<u8>> {
        let cells = self.host.list_cells(&self.workflow_name)?;
        let mut iter = cells.into_iter();
        let first = iter
            .next()
            .ok_or_else(|| anyhow!("missing keyed state for workflow '{}'", self.workflow_name))?;
        if iter.next().is_some() {
            anyhow::bail!(
                "workflow '{}' has multiple keyed cells; expected exactly one",
                self.workflow_name
            );
        }
        Ok(first.key_bytes)
    }

    pub fn kernel_mut(&mut self) -> LocalKernelGuard<'_> {
        self.host
            .kernel_mut()
            .expect("embedded world harness kernel_mut")
    }

    pub fn with_kernel_mut<R>(
        &mut self,
        f: impl FnOnce(&mut Kernel<FsCas>) -> Result<R, aos_kernel::KernelError>,
    ) -> Result<R> {
        self.host.with_kernel_mut(f).map_err(Into::into)
    }

    pub fn with_kernel<R>(&self, f: impl FnOnce(&Kernel<FsCas>) -> Result<R>) -> Result<R> {
        self.host
            .with_kernel(|kernel| {
                f(kernel).map_err(|err| aos_runtime::HostError::External(err.to_string()))
            })
            .map_err(Into::into)
    }

    pub fn store(&self) -> Arc<FsCas> {
        self.host.store()
    }

    pub fn wasm_hash(&self) -> &HashRef {
        &self.wasm_hash
    }

    pub fn finish(self) -> Result<ReplayHandle> {
        self.finish_with_keyed_samples(None, &[])
    }

    pub fn finish_with_keyed_samples(
        self,
        keyed_workflow: Option<&str>,
        keys: &[Vec<u8>],
    ) -> Result<ReplayHandle> {
        let final_state_bytes = self
            .host
            .state_bytes(&self.workflow_name, None)
            .unwrap_or_else(|| Vec::new());
        let mut keyed_states = Vec::new();
        if let Some(name) = keyed_workflow {
            let cells = self.host.list_cells(name)?;
            if cells.is_empty() {
                for key in keys {
                    if let Some(bytes) = self.host.state_bytes(name, Some(key)) {
                        keyed_states.push((key.clone(), bytes));
                    }
                }
            } else {
                for meta in cells {
                    if let Some(state) = self.host.state_bytes(name, Some(&meta.key_bytes)) {
                        keyed_states.push((meta.key_bytes, state));
                    }
                }
            }
        }
        Ok(ReplayHandle {
            host: self.host,
            final_state_bytes,
            workflow_name: self.workflow_name,
            keyed_workflow: keyed_workflow.map(str::to_string),
            keyed_states,
        })
    }
}

impl ExampleHost {
    fn read_single_keyed_state<T: DeserializeOwned>(&self) -> Result<T> {
        let key = self.single_keyed_cell_key()?;
        let bytes = self
            .host
            .state_bytes(&self.workflow_name, Some(&key))
            .ok_or_else(|| anyhow!("missing keyed state for workflow '{}'", self.workflow_name))?;
        serde_cbor::from_slice(&bytes).context("decode keyed workflow state")
    }
}

pub struct ReplayHandle {
    host: EmbeddedWorldHarness,
    final_state_bytes: Vec<u8>,
    workflow_name: String,
    keyed_workflow: Option<String>,
    keyed_states: Vec<(Vec<u8>, Vec<u8>)>,
}

impl ReplayHandle {
    pub fn verify_replay(self) -> Result<Vec<u8>> {
        if !self.final_state_bytes.is_empty() {
            let replay_bytes = self
                .host
                .replay_state_bytes(&self.workflow_name, None)
                .context("replay open world for state check")?
                .ok_or_else(|| anyhow!("missing replay state"))?;
            if replay_bytes != self.final_state_bytes {
                return Err(anyhow!("replay mismatch: workflow state diverged"));
            }
            let state_hash = Hash::of_bytes(&self.final_state_bytes).to_hex();
            println!("   replay check: OK (state hash {state_hash})");
        } else {
            println!("   replay check: no workflow state captured");
        }

        if let Some(name) = &self.keyed_workflow {
            for (key, bytes) in &self.keyed_states {
                let replayed = self
                    .host
                    .replay_state_bytes(name, Some(key))
                    .context("replay open world for keyed state check")?
                    .ok_or_else(|| anyhow!("missing keyed state for replay"))?;
                if replayed != *bytes {
                    return Err(anyhow!("replay mismatch for keyed workflow {name}"));
                }
            }
            println!(
                "   replay check (keyed {name}): OK ({} cells)",
                self.keyed_states.len()
            );
        }

        println!();
        Ok(self.final_state_bytes)
    }
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

fn load_and_patch_assets<S: Store + 'static>(
    store: Arc<S>,
    assets_root: &Path,
    import_roots: &[PathBuf],
    workflow_name: &str,
    wasm_hash: &HashRef,
) -> Result<manifest_loader::LoadedAssets> {
    let mut assets =
        manifest_loader::load_from_assets_with_imports_and_defs(store, assets_root, import_roots)
            .context("load manifest from assets")?
            .ok_or_else(|| anyhow!("example manifest missing at {}", assets_root.display()))?;
    patch_module_hash(&mut assets.loaded, workflow_name, wasm_hash)?;
    Ok(assets)
}
