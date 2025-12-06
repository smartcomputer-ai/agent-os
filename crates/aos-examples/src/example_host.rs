use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_host::manifest_loader;
use aos_host::testhost::TestHost;
use aos_host::util::patch_modules;
use aos_host::util::reset_journal;
use aos_kernel::LoadedManifest;
use aos_kernel::journal::mem::MemJournal;
use aos_kernel::Kernel;
use aos_store::{FsStore, Store};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::runtime::{Builder, Runtime};

use crate::util;

pub struct HarnessConfig<'a> {
    pub example_root: &'a Path,
    pub assets_root: Option<&'a Path>,
    pub reducer_name: &'a str,
    pub event_schema: &'a str,
    pub module_crate: &'a str,
}

/// Host-backed example driver built on TestHost, keeping explicit control flow.
pub struct ExampleHost {
    host: TestHost<FsStore>,
    reducer_name: String,
    event_schema: String,
    store: Arc<FsStore>,
    loaded: LoadedManifest,
    wasm_hash: HashRef,
    runtime: Runtime,
}

impl ExampleHost {
    pub fn prepare(cfg: HarnessConfig<'_>) -> Result<Self> {
        reset_journal(cfg.example_root)?;
        let wasm_bytes = util::compile_reducer(cfg.module_crate)?;

        let store = Arc::new(FsStore::open(cfg.example_root).context("open FsStore")?);
        let wasm_hash = store
            .put_blob(&wasm_bytes)
            .context("store reducer wasm blob")?;
        let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).context("hash reducer wasm")?;

        let assets_root = cfg.assets_root.unwrap_or(cfg.example_root);
        let mut loaded = manifest_loader::load_from_assets(store.clone(), assets_root)
            .context("load manifest from assets")?
            .ok_or_else(|| anyhow!("example manifest missing at {}", assets_root.display()))?;
        patch_module_hash(&mut loaded, cfg.reducer_name, &wasm_hash_ref)?;

        let host = TestHost::from_loaded_manifest(store.clone(), loaded.clone())?;
        let runtime = Builder::new_current_thread().enable_all().build()?;

        Ok(Self {
            host,
            reducer_name: cfg.reducer_name.to_string(),
            event_schema: cfg.event_schema.to_string(),
            store,
            loaded,
            wasm_hash: wasm_hash_ref,
            runtime,
        })
    }

    pub fn send_event<T: Serialize>(&mut self, event: &T) -> Result<()> {
        let cbor = serde_cbor::to_vec(event)?;
        self.host
            .send_event_cbor(&self.event_schema, cbor)
            .context("send event")?;
        // Mirror the previous harness behavior: advance reducer immediately.
        self.host.run_to_idle().context("drain after event")
    }

    pub fn run_cycle_batch(&mut self) -> Result<()> {
        self.runtime
            .block_on(self.host.run_cycle_batch())
            .context("run cycle batch")?;
        Ok(())
    }

    pub fn run_cycle_with_timers(&mut self) -> Result<()> {
        self.runtime
            .block_on(self.host.run_cycle_with_timers())
            .context("run cycle with timers")?;
        Ok(())
    }

    pub fn drain_effects(&mut self) -> Vec<aos_effects::EffectIntent> {
        self.host.drain_effects()
    }

    pub fn apply_receipt(&mut self, receipt: aos_effects::EffectReceipt) -> Result<()> {
        self.host.apply_receipt(receipt).context("apply receipt")
    }

    pub fn read_state<T: DeserializeOwned>(&self) -> Result<T> {
        self.host
            .state(&self.reducer_name)
            .context("read reducer state")
    }

    pub fn kernel_mut(&mut self) -> &mut Kernel<FsStore> {
        self.host.kernel_mut()
    }

    pub fn store(&self) -> Arc<FsStore> {
        self.store.clone()
    }

    pub fn wasm_hash(&self) -> &HashRef {
        &self.wasm_hash
    }

    pub fn finish(self) -> Result<ReplayHandle> {
        let final_state_bytes = self
            .host
            .state_bytes(&self.reducer_name)
            .cloned()
            .ok_or_else(|| anyhow!("missing reducer state"))?;
        let journal_entries = self.host.kernel().dump_journal()?;
        Ok(ReplayHandle {
            store: self.store,
            loaded: self.loaded,
            final_state_bytes,
            journal_entries,
            reducer_name: self.reducer_name,
        })
    }
}

pub struct ReplayHandle {
    store: Arc<FsStore>,
    loaded: LoadedManifest,
    final_state_bytes: Vec<u8>,
    journal_entries: Vec<aos_kernel::journal::OwnedJournalEntry>,
    reducer_name: String,
}

impl ReplayHandle {
    pub fn verify_replay(self) -> Result<Vec<u8>> {
        let mut kernel = Kernel::from_loaded_manifest(
            self.store.clone(),
            self.loaded,
            Box::new(MemJournal::from_entries(&self.journal_entries)),
        )?;
        kernel.tick_until_idle()?;
        let replay_bytes = kernel
            .reducer_state(&self.reducer_name)
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
    let patched = patch_modules(loaded, wasm_hash, |name, _| name == reducer_name);
    if patched == 0 {
        anyhow::bail!("module '{reducer_name}' missing from manifest");
    }
    Ok(())
}
