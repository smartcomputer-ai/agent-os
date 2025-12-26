use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_host::config::HostConfig;
use aos_host::host::WorldHost;
use aos_host::manifest_loader;
use aos_host::testhost::TestHost;
use aos_host::util::{is_placeholder_hash, patch_modules};
use aos_host::util::reset_journal;
use aos_kernel::LoadedManifest;
use aos_kernel::cell_index::CellIndex;
use aos_kernel::journal::OwnedJournalEntry;
use aos_kernel::journal::mem::MemJournal;
use aos_kernel::{Kernel, KernelConfig};
use aos_store::{FsStore, Store};
use serde::Serialize;
use serde::de::DeserializeOwned;
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
    kernel_config: KernelConfig,
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

        let assets_root = cfg.assets_root.unwrap_or(cfg.example_root).to_path_buf();

        let mut loaded_host = load_and_patch(
            store.clone(),
            &assets_root,
            cfg.reducer_name,
            &wasm_hash_ref,
        )?;
        let mut loaded_replay = load_and_patch(
            store.clone(),
            &assets_root,
            cfg.reducer_name,
            &wasm_hash_ref,
        )?;

        maybe_patch_object_catalog(cfg.example_root, store.clone(), &mut loaded_host)?;
        maybe_patch_object_catalog(cfg.example_root, store.clone(), &mut loaded_replay)?;

        let mut sys_module_cache = HashMap::new();
        maybe_patch_sys_enforcers(
            cfg.example_root,
            store.clone(),
            &mut loaded_host,
            &mut sys_module_cache,
        )?;
        maybe_patch_sys_enforcers(
            cfg.example_root,
            store.clone(),
            &mut loaded_replay,
            &mut sys_module_cache,
        )?;

        let host_config = HostConfig::default();
        let kernel_config = util::kernel_config(cfg.example_root)?;
        let world_host = WorldHost::from_loaded_manifest(
            store.clone(),
            loaded_host,
            cfg.example_root,
            host_config,
            kernel_config.clone(),
        )?;
        let host = TestHost::from_world_host(world_host);
        let runtime = Builder::new_current_thread().enable_all().build()?;

        Ok(Self {
            host,
            reducer_name: cfg.reducer_name.to_string(),
            event_schema: cfg.event_schema.to_string(),
            store,
            loaded: loaded_replay,
            wasm_hash: wasm_hash_ref,
            kernel_config: kernel_config.clone(),
            runtime,
        })
    }

    pub fn send_event<T: Serialize>(&mut self, event: &T) -> Result<()> {
        let schema = self.event_schema.clone();
        self.send_event_as(&schema, event)
    }

    pub fn send_event_as<T: Serialize>(&mut self, schema: &str, event: &T) -> Result<()> {
        let cbor = serde_cbor::to_vec(event)?;
        self.host
            .send_event_cbor(schema, cbor)
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
        self.finish_with_keyed_samples(None, &[])
    }

    pub fn finish_with_keyed_samples(
        self,
        keyed_reducer: Option<&str>,
        keys: &[Vec<u8>],
    ) -> Result<ReplayHandle> {
        let final_state_bytes = self
            .host
            .state_bytes(&self.reducer_name)
            .unwrap_or_else(|| Vec::new());
        let journal_entries = self.host.kernel().dump_journal()?;
        let mut keyed_states = Vec::new();
        if let Some(name) = keyed_reducer {
            if let Some(root) = self.host.kernel().reducer_index_root(name) {
                let index = CellIndex::new(self.store.as_ref());
                for meta in index.iter(root) {
                    let meta = meta?;
                    let state_hash = Hash::from_bytes(&meta.state_hash)
                        .unwrap_or_else(|_| Hash::of_bytes(&meta.state_hash));
                    let state = self.store.get_blob(state_hash)?;
                    keyed_states.push((meta.key_bytes.clone(), state));
                }
            } else {
                // fallback to explicit keys if no root (should not happen)
                for key in keys {
                    if let Some(bytes) = self.host.kernel().reducer_state_bytes(name, Some(key))? {
                        keyed_states.push((key.clone(), bytes));
                    }
                }
            }
        }
        Ok(ReplayHandle {
            store: self.store,
            loaded: self.loaded,
            final_state_bytes,
            journal_entries,
            reducer_name: self.reducer_name,
            kernel_config: self.kernel_config,
            keyed_reducer: keyed_reducer.map(str::to_string),
            keyed_states,
        })
    }
}

pub struct ReplayHandle {
    store: Arc<FsStore>,
    loaded: LoadedManifest,
    final_state_bytes: Vec<u8>,
    journal_entries: Vec<OwnedJournalEntry>,
    reducer_name: String,
    kernel_config: KernelConfig,
    keyed_reducer: Option<String>,
    keyed_states: Vec<(Vec<u8>, Vec<u8>)>,
}

impl ReplayHandle {
    pub fn verify_replay(self) -> Result<Vec<u8>> {
        let mut kernel = Kernel::from_loaded_manifest_with_config(
            self.store.clone(),
            self.loaded,
            Box::new(MemJournal::from_entries(&self.journal_entries)),
            self.kernel_config,
        )?;
        kernel.tick_until_idle()?;
        if !self.final_state_bytes.is_empty() {
            let replay_bytes = kernel
                .reducer_state(&self.reducer_name)
                .ok_or_else(|| anyhow!("missing replay state"))?;
            if replay_bytes != self.final_state_bytes {
                return Err(anyhow!("replay mismatch: reducer state diverged"));
            }
            let state_hash = Hash::of_bytes(&self.final_state_bytes).to_hex();
            println!("   replay check: OK (state hash {state_hash})");
        } else {
            println!("   replay check: no reducer state captured");
        }

        if let Some(name) = &self.keyed_reducer {
            for (key, bytes) in &self.keyed_states {
                let replayed = kernel
                    .reducer_state_bytes(name, Some(key))?
                    .ok_or_else(|| anyhow!("missing keyed state for replay"))?;
                if replayed != *bytes {
                    return Err(anyhow!("replay mismatch for keyed reducer {name}"));
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
    reducer_name: &str,
    wasm_hash: &HashRef,
) -> Result<()> {
    let patched = patch_modules(loaded, wasm_hash, |name, _| name == reducer_name);
    if patched == 0 {
        anyhow::bail!("module '{reducer_name}' missing from manifest");
    }
    Ok(())
}

fn load_and_patch(
    store: Arc<FsStore>,
    assets_root: &Path,
    reducer_name: &str,
    wasm_hash: &HashRef,
) -> Result<LoadedManifest> {
    let mut loaded = manifest_loader::load_from_assets(store, assets_root)
        .context("load manifest from assets")?
        .ok_or_else(|| anyhow!("example manifest missing at {}", assets_root.display()))?;
    patch_module_hash(&mut loaded, reducer_name, wasm_hash)?;
    Ok(loaded)
}

fn maybe_patch_object_catalog(
    example_root: &Path,
    store: Arc<FsStore>,
    loaded: &mut LoadedManifest,
) -> Result<()> {
    let needs_patch = loaded.modules.iter().any(|(name, module)| {
        name == "sys/ObjectCatalog@1" && aos_host::util::is_placeholder_hash(module)
    });
    if !needs_patch {
        return Ok(());
    }
    let cache_dir = example_root.join(".aos").join("cache").join("modules");
    let wasm_bytes = util::compile_wasm_bin(
        crate::workspace_root(),
        "aos-sys",
        "object_catalog",
        &cache_dir,
    )?;
    let wasm_hash = store
        .put_blob(&wasm_bytes)
        .context("store object_catalog wasm blob")?;
    let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).context("hash object catalog")?;
    let patched = patch_modules(loaded, &wasm_hash_ref, |name, _| {
        name == "sys/ObjectCatalog@1"
    });
    if patched == 0 {
        anyhow::bail!("object catalog module missing in manifest");
    }
    Ok(())
}

fn maybe_patch_sys_enforcers(
    example_root: &Path,
    store: Arc<FsStore>,
    loaded: &mut LoadedManifest,
    cache: &mut HashMap<&'static str, HashRef>,
) -> Result<()> {
    for (module_name, bin_name) in [
        ("sys/CapEnforceHttpOut@1", "cap_enforce_http_out"),
        ("sys/CapEnforceLlmBasic@1", "cap_enforce_llm_basic"),
    ] {
        maybe_patch_sys_module(
            example_root,
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
    example_root: &Path,
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
        let cache_dir = example_root.join(".aos").join("cache").join("modules");
        let wasm_bytes = util::compile_wasm_bin(
            crate::workspace_root(),
            "aos-sys",
            bin_name,
            &cache_dir,
        )?;
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
