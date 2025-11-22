use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_air_types::{HashRef, Name};
use aos_cbor::Hash;
use aos_kernel::journal::fs::FsJournal;
use aos_kernel::{Kernel, KernelConfig, LoadedManifest};
use aos_store::{FsStore, Store};
use serde::{Serialize, de::DeserializeOwned};

use crate::support::manifest_loader;
use crate::support::util;

pub struct HarnessConfig<'a> {
    pub example_root: &'a Path,
    pub reducer_name: &'a str,
    pub event_schema: &'a str,
    pub module_crate: &'a str,
}

pub struct ExampleReducerHarness {
    example_root: PathBuf,
    reducer_name: Name,
    event_schema: Name,
    store: Arc<FsStore>,
    wasm_hash: HashRef,
    kernel_config: KernelConfig,
}

impl ExampleReducerHarness {
    pub fn prepare(cfg: HarnessConfig<'_>) -> Result<Self> {
        util::reset_journal(cfg.example_root)?;
        let wasm_bytes = util::compile_reducer(cfg.module_crate)?;
        let store = Arc::new(FsStore::open(cfg.example_root).context("open FsStore")?);
        let wasm_hash = store
            .put_blob(&wasm_bytes)
            .context("store reducer wasm blob")?;
        let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).context("hash reducer wasm")?;

        // Eagerly load and patch once so we fail fast if assets are missing.
        {
            let mut loaded = manifest_loader::load_from_assets(store.clone(), cfg.example_root)?
                .ok_or_else(|| {
                    anyhow!("example manifest missing at {}", cfg.example_root.display())
                })?;
            patch_module_hash(&mut loaded, cfg.reducer_name, &wasm_hash_ref)?;
        }

        let kernel_config = util::kernel_config(cfg.example_root)?;

        Ok(Self {
            example_root: cfg.example_root.to_path_buf(),
            reducer_name: cfg.reducer_name.to_string(),
            event_schema: cfg.event_schema.to_string(),
            store,
            wasm_hash: wasm_hash_ref,
            kernel_config,
        })
    }

    pub fn start(&self) -> Result<ExampleRun<'_>> {
        let loaded = self.load_manifest()?;
        let journal = Box::new(FsJournal::open(&self.example_root)?);
        let kernel = Kernel::from_loaded_manifest_with_config(
            self.store.clone(),
            loaded,
            journal,
            self.kernel_config.clone(),
        )?;
        Ok(ExampleRun {
            harness: self,
            kernel,
        })
    }

    pub fn store(&self) -> Arc<FsStore> {
        self.store.clone()
    }

    pub fn wasm_hash(&self) -> &HashRef {
        &self.wasm_hash
    }

    pub fn patch_module_hash(&self, loaded: &mut LoadedManifest) -> Result<()> {
        patch_module_hash(loaded, self.reducer_name(), &self.wasm_hash)
    }

    fn load_manifest(&self) -> Result<LoadedManifest> {
        let mut loaded = manifest_loader::load_from_assets(self.store.clone(), &self.example_root)?
            .ok_or_else(|| {
                anyhow!(
                    "example manifest missing at {}",
                    self.example_root.display()
                )
            })?;
        patch_module_hash(&mut loaded, &self.reducer_name, &self.wasm_hash)?;
        Ok(loaded)
    }

    fn reducer_name(&self) -> &str {
        &self.reducer_name
    }

    fn event_schema(&self) -> &str {
        &self.event_schema
    }

    fn example_root(&self) -> &Path {
        &self.example_root
    }

    fn kernel_config(&self) -> KernelConfig {
        self.kernel_config.clone()
    }
}

pub struct ExampleRun<'h> {
    harness: &'h ExampleReducerHarness,
    kernel: Kernel<FsStore>,
}

impl<'h> ExampleRun<'h> {
    pub fn submit_event<T: Serialize>(&mut self, event: &T) -> Result<()> {
        let payload = serde_cbor::to_vec(event)?;
        self.kernel
            .submit_domain_event(self.harness.event_schema().to_string(), payload);
        self.kernel.tick_until_idle()?;
        Ok(())
    }

    pub fn read_state<T: DeserializeOwned>(&self) -> Result<T> {
        let bytes = self
            .kernel
            .reducer_state(self.harness.reducer_name())
            .cloned()
            .ok_or_else(|| anyhow!("missing reducer state"))?;
        let state = serde_cbor::from_slice(&bytes)?;
        Ok(state)
    }

    pub fn kernel_mut(&mut self) -> &mut Kernel<FsStore> {
        &mut self.kernel
    }

    pub fn finish(self) -> Result<ReplayHandle<'h>> {
        let final_state_bytes = self
            .kernel
            .reducer_state(self.harness.reducer_name())
            .cloned()
            .ok_or_else(|| anyhow!("missing reducer state"))?;
        Ok(ReplayHandle {
            harness: self.harness,
            final_state_bytes,
        })
    }
}

pub struct ReplayHandle<'h> {
    harness: &'h ExampleReducerHarness,
    final_state_bytes: Vec<u8>,
}

impl<'h> ReplayHandle<'h> {
    pub fn verify_replay(self) -> Result<Vec<u8>> {
        let loaded = self.harness.load_manifest()?;
        let journal = Box::new(FsJournal::open(self.harness.example_root())?);
        let mut kernel = Kernel::from_loaded_manifest_with_config(
            self.harness.store(),
            loaded,
            journal,
            self.harness.kernel_config(),
        )?;
        kernel.tick_until_idle()?;
        let replay_bytes = kernel
            .reducer_state(self.harness.reducer_name())
            .cloned()
            .ok_or_else(|| anyhow!("missing replay state"))?;
        if replay_bytes != self.final_state_bytes {
            return Err(anyhow!("replay mismatch: reducer state diverged"));
        }
        let state_hash = Hash::of_bytes(&self.final_state_bytes).to_hex();
        println!("   replay check: OK (state hash {state_hash})\n");
        Ok(self.final_state_bytes)
    }
}

fn patch_module_hash(
    loaded: &mut LoadedManifest,
    reducer_name: &str,
    wasm_hash: &HashRef,
) -> Result<()> {
    let module = loaded
        .modules
        .get_mut(reducer_name)
        .ok_or_else(|| anyhow!("module '{reducer_name}' missing from manifest"))?;
    module.wasm_hash = wasm_hash.clone();
    Ok(())
}
