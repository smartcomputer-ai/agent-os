use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use aos_air_types::HashRef;
use aos_authoring::{
    local_state_paths, manifest_loader, patch_modules, resolve_placeholder_modules,
};
use aos_cbor::Hash;
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_effects::builtins::{TimerSetParams, TimerSetReceipt};
use aos_effects::{EffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::journal::{Journal, OwnedJournalEntry};
use aos_kernel::{Kernel, KernelConfig, KernelQuiescence, LoadedManifest, Store, WorldInput};
use aos_node::{EffectExecutionClass, EffectRuntime, EffectRuntimeEvent, FsCas, WorldConfig};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};
use tokio::sync::mpsc;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EffectMode {
    #[default]
    Scripted,
    Twin,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrainStatus {
    pub idle: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CycleOutcome {
    pub effects_dispatched: usize,
    pub receipts_applied: usize,
    pub final_drain: DrainStatus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QuiescenceStatus {
    pub kernel_idle: bool,
    pub runtime_quiescent: bool,
    pub timers_pending: usize,
    pub next_timer_deadline_ns: Option<u64>,
}

#[derive(Debug, Clone)]
struct PendingTimer {
    intent_hash: [u8; 32],
    params: TimerSetParams,
}

pub struct LocalKernelGuard<'a> {
    kernel: &'a mut Kernel<FsCas>,
}

impl<'a> Deref for LocalKernelGuard<'a> {
    type Target = Kernel<FsCas>;

    fn deref(&self) -> &Self::Target {
        self.kernel
    }
}

impl<'a> DerefMut for LocalKernelGuard<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.kernel
    }
}

/// Scripted example driver backed directly by a kernel.
pub struct ExampleHost {
    kernel: Kernel<FsCas>,
    loaded: LoadedManifest,
    kernel_config: KernelConfig,
    workflow_name: String,
    event_schema: String,
    store: Arc<FsCas>,
    wasm_hash: HashRef,
    logical_now_ns: u64,
    pending_timers: Vec<PendingTimer>,
    effect_mode: EffectMode,
    effect_runtime: Option<EffectRuntime<()>>,
    effect_event_rx: Option<mpsc::Receiver<EffectRuntimeEvent<()>>>,
    async_runtime: Option<Runtime>,
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
        let host_config = host_config_override.unwrap_or_default();
        let paths = local_state_paths(cfg.example_root);
        paths.ensure_root().context("create local state root")?;
        let store = Arc::new(FsCas::open_with_paths(&paths).context("open local CAS")?);
        let wasm_hash = store
            .put_blob(&wasm_bytes)
            .context("store workflow wasm blob")?;
        let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).context("hash workflow wasm")?;

        let assets_root = cfg.assets_root.unwrap_or(cfg.example_root).to_path_buf();
        let mut assets = load_and_patch_assets(
            store.clone(),
            &assets_root,
            import_roots,
            cfg.workflow_name,
            &wasm_hash_ref,
        )?;
        resolve_placeholder_modules(
            &mut assets.loaded,
            store.as_ref(),
            cfg.example_root,
            &paths,
            None,
            None,
        )?;

        let kernel_config = host_config
            .world
            .apply_kernel_defaults(util::kernel_config(cfg.example_root)?);
        let loaded = assets.loaded.clone();
        let (effect_runtime, effect_event_rx, async_runtime) =
            if matches!(host_config.effect_mode, EffectMode::Twin) {
                let (tx, rx) = mpsc::channel(32);
                let async_runtime = RuntimeBuilder::new_current_thread()
                    .enable_all()
                    .build()
                    .context("build example host async runtime")?;
                let effect_runtime = EffectRuntime::from_loaded_manifest(
                    store.clone(),
                    &host_config.adapters,
                    &loaded,
                    host_config.world.strict_effect_bindings,
                    tx,
                )
                .context("build example host effect runtime")?;
                (Some(effect_runtime), Some(rx), Some(async_runtime))
            } else {
                (None, None, None)
            };
        let kernel = Kernel::from_loaded_manifest_with_config(
            store.clone(),
            assets.loaded,
            Journal::new(),
            kernel_config.clone(),
        )
        .context("boot example kernel")?;

        Ok(Self {
            kernel,
            loaded,
            kernel_config,
            workflow_name: cfg.workflow_name.to_string(),
            event_schema: cfg.event_schema.to_string(),
            store,
            wasm_hash: wasm_hash_ref,
            logical_now_ns: 0,
            pending_timers: Vec::new(),
            effect_mode: host_config.effect_mode,
            effect_runtime,
            effect_event_rx,
            async_runtime,
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
        self.kernel
            .submit_domain_event_result(schema.to_string(), cbor)
            .context("send event")?;
        let submit = submit_start.elapsed();

        let drain_start = Instant::now();
        self.kernel.tick_until_idle().context("drain after event")?;
        let drain = drain_start.elapsed();

        Ok(EventDispatchTiming {
            encode,
            submit,
            drain,
        })
    }

    pub fn run_cycle_batch(&mut self) -> Result<CycleOutcome> {
        self.run_cycle(false)
    }

    pub fn run_cycle_with_timers(&mut self) -> Result<CycleOutcome> {
        self.run_cycle(true)
    }

    pub fn quiescence_status(&self) -> QuiescenceStatus {
        let kernel = self.kernel.quiescence_status();
        QuiescenceStatus {
            kernel_idle: kernel.kernel_idle,
            runtime_quiescent: kernel.runtime_quiescent && self.pending_timers.is_empty(),
            timers_pending: self.pending_timers.len(),
            next_timer_deadline_ns: self
                .pending_timers
                .iter()
                .map(|timer| timer.params.deliver_at_ns)
                .min(),
        }
    }

    pub fn time_set(&mut self, now_ns: u64) -> u64 {
        self.logical_now_ns = self.kernel.set_logical_time_ns(now_ns);
        self.logical_now_ns
    }

    pub fn time_jump_next_due(&mut self) -> Result<Option<u64>> {
        let Some(next_due) = self
            .pending_timers
            .iter()
            .map(|timer| timer.params.deliver_at_ns)
            .min()
        else {
            return Ok(None);
        };

        self.time_set(next_due);
        self.fire_due_timers()?;
        Ok(Some(next_due))
    }

    pub fn drain_effects(&mut self) -> Result<Vec<aos_effects::EffectIntent>> {
        self.kernel.drain_effects().context("drain effects")
    }

    pub fn apply_receipt(&mut self, receipt: aos_effects::EffectReceipt) -> Result<()> {
        self.kernel
            .handle_receipt(receipt)
            .context("apply receipt")?;
        self.kernel
            .tick_until_idle()
            .context("drain after receipt")?;
        Ok(())
    }

    pub fn read_state<T: DeserializeOwned>(&self) -> Result<T> {
        if let Some(state) = self.kernel.workflow_state(&self.workflow_name) {
            return serde_cbor::from_slice(&state).context("decode workflow state");
        }
        self.read_single_keyed_state()
    }

    pub fn single_keyed_cell_key(&self) -> Result<Vec<u8>> {
        let cells = self.kernel.list_cells(&self.workflow_name)?;
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
        LocalKernelGuard {
            kernel: &mut self.kernel,
        }
    }

    pub fn with_kernel_mut<R>(
        &mut self,
        f: impl FnOnce(&mut Kernel<FsCas>) -> Result<R, aos_kernel::KernelError>,
    ) -> Result<R> {
        f(&mut self.kernel).map_err(Into::into)
    }

    pub fn with_kernel<R>(&self, f: impl FnOnce(&Kernel<FsCas>) -> Result<R>) -> Result<R> {
        f(&self.kernel)
    }

    pub fn store(&self) -> Arc<FsCas> {
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
        keyed_workflow: Option<&str>,
        keys: &[Vec<u8>],
    ) -> Result<ReplayHandle> {
        let final_state_bytes = self
            .kernel
            .workflow_state_bytes(&self.workflow_name, None)?
            .unwrap_or_default();
        let mut keyed_states = Vec::new();
        if let Some(name) = keyed_workflow {
            let cells = self.kernel.list_cells(name)?;
            if cells.is_empty() {
                for key in keys {
                    if let Some(bytes) = self.kernel.workflow_state_bytes(name, Some(key))? {
                        keyed_states.push((key.clone(), bytes));
                    }
                }
            } else {
                for meta in cells {
                    if let Some(state) = self
                        .kernel
                        .workflow_state_bytes(name, Some(&meta.key_bytes))?
                    {
                        keyed_states.push((meta.key_bytes, state));
                    }
                }
            }
        }

        Ok(ReplayHandle {
            store: self.store,
            loaded: self.loaded,
            kernel_config: self.kernel_config,
            journal_entries: self.kernel.dump_journal()?,
            final_state_bytes,
            workflow_name: self.workflow_name,
            keyed_workflow: keyed_workflow.map(str::to_string),
            keyed_states,
        })
    }

    fn run_cycle(&mut self, process_timers: bool) -> Result<CycleOutcome> {
        let intents = self.kernel.drain_effects().context("drain effect batch")?;
        let mut effects_dispatched = 0usize;
        let mut receipts_applied = 0usize;
        let mut runtime_inputs_applied = false;

        for intent in intents {
            if let Some(receipt) = self.kernel.handle_internal_intent(&intent)? {
                self.kernel
                    .handle_receipt(receipt)
                    .context("apply internal receipt")?;
                receipts_applied = receipts_applied.saturating_add(1);
                continue;
            }

            if intent.kind.as_str() == EffectKind::TIMER_SET {
                let params: TimerSetParams =
                    serde_cbor::from_slice(&intent.params_cbor).context("decode timer.set")?;
                self.pending_timers.push(PendingTimer {
                    intent_hash: intent.intent_hash,
                    params,
                });
                effects_dispatched = effects_dispatched.saturating_add(1);
                runtime_inputs_applied = true;
                continue;
            }

            if matches!(self.effect_mode, EffectMode::Twin)
                && matches!(
                    self.classify_effect_intent(&intent),
                    EffectExecutionClass::ExternalAsync
                )
            {
                effects_dispatched = effects_dispatched.saturating_add(1);
                receipts_applied =
                    receipts_applied.saturating_add(self.dispatch_external_intent(intent)?);
                runtime_inputs_applied = true;
            }
        }

        if receipts_applied > 0 || runtime_inputs_applied {
            self.kernel
                .tick_until_idle()
                .context("drain after internal work")?;
        }
        if process_timers {
            receipts_applied = receipts_applied.saturating_add(self.fire_due_timers()?);
        }

        Ok(CycleOutcome {
            effects_dispatched,
            receipts_applied,
            final_drain: DrainStatus {
                idle: self.kernel.quiescence_status().kernel_idle,
            },
        })
    }

    fn fire_due_timers(&mut self) -> Result<usize> {
        let mut ready = Vec::new();
        let mut pending = Vec::new();
        for timer in self.pending_timers.drain(..) {
            if timer.params.deliver_at_ns <= self.logical_now_ns {
                ready.push(timer);
            } else {
                pending.push(timer);
            }
        }
        self.pending_timers = pending;

        for timer in &ready {
            let receipt = EffectReceipt {
                intent_hash: timer.intent_hash,
                adapter_id: "timer.default".into(),
                status: ReceiptStatus::Ok,
                payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
                    delivered_at_ns: timer.params.deliver_at_ns,
                    key: timer.params.key.clone(),
                })?,
                cost_cents: Some(0),
                signature: vec![0; 64],
            };
            self.kernel
                .handle_receipt(receipt)
                .context("apply timer receipt")?;
        }
        if !ready.is_empty() {
            self.kernel
                .tick_until_idle()
                .context("drain after timer fire")?;
        }
        Ok(ready.len())
    }

    fn read_single_keyed_state<T: DeserializeOwned>(&self) -> Result<T> {
        let key = self.single_keyed_cell_key()?;
        let bytes = self
            .kernel
            .workflow_state_bytes(&self.workflow_name, Some(&key))?
            .ok_or_else(|| anyhow!("missing keyed state for workflow '{}'", self.workflow_name))?;
        serde_cbor::from_slice(&bytes).context("decode keyed workflow state")
    }

    fn classify_effect_intent(&self, intent: &aos_effects::EffectIntent) -> EffectExecutionClass {
        aos_node::classify_effect_kind(intent.kind.as_str())
    }

    fn dispatch_external_intent(&mut self, intent: aos_effects::EffectIntent) -> Result<usize> {
        let effect_runtime = self
            .effect_runtime
            .as_ref()
            .ok_or_else(|| anyhow!("example host twin mode missing effect runtime"))?;
        let async_runtime = self
            .async_runtime
            .as_ref()
            .ok_or_else(|| anyhow!("example host twin mode missing async runtime"))?;
        let mut effect_event_rx = self
            .effect_event_rx
            .take()
            .ok_or_else(|| anyhow!("example host twin mode missing effect receiver"))?;

        let result = async_runtime.block_on(async {
            effect_runtime
                .ensure_started((), intent)
                .context("start external effect")?;

            let mut inputs = Vec::new();
            loop {
                let event = effect_event_rx
                    .recv()
                    .await
                    .ok_or_else(|| anyhow!("effect runtime closed without terminal receipt"))?;
                let EffectRuntimeEvent::WorldInput { world_id: _, input } = event;
                let terminal = matches!(input, WorldInput::Receipt(_));
                inputs.push(input);
                if terminal {
                    break;
                }
            }

            Ok::<Vec<WorldInput>, anyhow::Error>(inputs)
        });
        self.effect_event_rx = Some(effect_event_rx);

        let mut receipts_applied = 0usize;
        for input in result? {
            match input {
                WorldInput::Receipt(receipt) => {
                    self.kernel
                        .handle_receipt(receipt)
                        .context("apply external effect receipt")?;
                    receipts_applied = receipts_applied.saturating_add(1);
                }
                WorldInput::StreamFrame(frame) => {
                    self.kernel
                        .accept(WorldInput::StreamFrame(frame))
                        .context("apply external effect stream frame")?;
                }
                WorldInput::DomainEvent(event) => {
                    self.kernel
                        .accept(WorldInput::DomainEvent(event))
                        .context("apply external effect domain event")?;
                }
            }
        }

        Ok(receipts_applied)
    }
}

pub struct ReplayHandle {
    store: Arc<FsCas>,
    loaded: LoadedManifest,
    kernel_config: KernelConfig,
    journal_entries: Vec<OwnedJournalEntry>,
    final_state_bytes: Vec<u8>,
    workflow_name: String,
    keyed_workflow: Option<String>,
    keyed_states: Vec<(Vec<u8>, Vec<u8>)>,
}

impl ReplayHandle {
    pub fn verify_replay(self) -> Result<Vec<u8>> {
        let replay_kernel = Kernel::from_loaded_manifest_with_config(
            self.store,
            self.loaded,
            Journal::from_entries(&self.journal_entries).context("seed replay journal")?,
            self.kernel_config,
        )
        .context("replay example kernel")?;

        if !self.final_state_bytes.is_empty() {
            let replay_bytes = replay_kernel
                .workflow_state_bytes(&self.workflow_name, None)?
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
                let replayed = replay_kernel
                    .workflow_state_bytes(name, Some(key))?
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

#[cfg(test)]
mod tests {
    use super::*;
    use aos_effect_adapters::adapters::mock::{MockHttpHarness, MockHttpResponse};
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct SummarizerStateView {
        last_summary: Option<String>,
    }

    #[derive(serde::Serialize)]
    struct SummarizerStartEvent {
        #[serde(rename = "Start")]
        start: SummarizerStartPayload,
    }

    #[derive(serde::Serialize)]
    struct SummarizerStartPayload {
        url: String,
    }

    #[test]
    fn twin_mode_executes_external_llm_effects() {
        let fixture_root =
            crate::workspace_root().join("crates/aos-smoke/fixtures/07-llm-summarizer");
        let mut host = ExampleHost::prepare_with_host_config(
            HarnessConfig {
                example_root: &fixture_root,
                assets_root: None,
                workflow_name: "demo/LlmSummarizer@1",
                event_schema: "demo/LlmSummarizerEvent@1",
                module_crate: "crates/aos-smoke/fixtures/07-llm-summarizer/workflow",
            },
            ExampleHostConfig {
                effect_mode: EffectMode::Twin,
                ..ExampleHostConfig::default()
            },
        )
        .expect("prepare twin mode host");

        host.send_event(&SummarizerStartEvent {
            start: SummarizerStartPayload {
                url: "https://example.com/story.txt".into(),
            },
        })
        .expect("send summarizer start event");

        let mut http = MockHttpHarness::new();
        let requests = http
            .collect_requests(&mut host.kernel_mut())
            .expect("collect http requests");
        assert_eq!(requests.len(), 1);
        let http_ctx = requests.into_iter().next().expect("http request");
        let body = "Summaries should stay deterministic.".to_string();
        let store = host.store();
        http.respond_with_body(
            &mut host.kernel_mut(),
            Some(store.as_ref()),
            http_ctx,
            MockHttpResponse::json(200, body),
        )
        .expect("apply http response");

        let outcome = host.run_cycle_batch().expect("run twin cycle");
        assert_eq!(outcome.effects_dispatched, 1);
        assert_eq!(outcome.receipts_applied, 1);

        let state: SummarizerStateView = host.read_state().expect("read summarizer state");
        assert!(
            state
                .last_summary
                .as_deref()
                .is_some_and(|value| value.starts_with("sha256:")),
            "unexpected summary ref: {:?}",
            state.last_summary
        );
    }
}
