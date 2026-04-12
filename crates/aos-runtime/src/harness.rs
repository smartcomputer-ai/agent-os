use std::path::{Path, PathBuf};
use std::sync::Arc;

use aos_cbor::to_canonical_cbor;
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_effect_adapters::registry::AdapterRegistry;
use aos_effects::builtins::{
    BlobGetReceipt, BlobPutReceipt, HttpRequestReceipt, LlmGenerateReceipt, RequestTimings,
    TimerSetReceipt,
};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use aos_kernel::cell_index::CellMeta;
use aos_kernel::journal::{Journal, OwnedJournalEntry};
use aos_kernel::{
    Consistency, Kernel, KernelConfig, LoadedManifest, ManifestLoader, StateReader, Store,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};

use crate::WorldConfig;
use crate::error::HostError;
use crate::host::{CycleOutcome, QuiescenceStatus, RunMode, WorldHost};
use crate::testhost::TestHost;
use crate::timer::TimerScheduler;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessBackend {
    Ephemeral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectMode {
    Scripted,
    Twin,
    Live,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessEvidence {
    pub backend: &'static str,
    pub effect_mode: &'static str,
    pub logical_time_ns: u64,
    pub heights_head: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heights_snapshot: Option<u64>,
    pub cycles_run: u64,
    pub effects_dispatched: u64,
    pub receipts_applied: u64,
    pub quiescence: QuiescenceStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessArtifacts {
    pub evidence: HarnessEvidence,
    pub trace_summary: JsonValue,
    pub journal_entries: Vec<OwnedJournalEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessReplayReport {
    pub ok: bool,
    pub mismatches: Vec<String>,
}

#[derive(Debug, Clone)]
enum HarnessManifestSource {
    LoadedManifest(LoadedManifest),
    ManifestCbor(Vec<u8>),
    ManifestPath(PathBuf),
}

pub trait HarnessBackendHooks<S: Store + 'static>: Send + Sync {
    fn snapshot(&self, host: &mut TestHost<S>) -> Result<(), HostError> {
        host.snapshot()
    }

    fn reopen(
        &self,
        _host: &TestHost<S>,
        _world_config: &WorldConfig,
        _adapter_config: &EffectAdapterConfig,
        _kernel_config: &KernelConfig,
    ) -> Result<Option<TestHost<S>>, HostError> {
        Ok(None)
    }
}

pub struct HarnessBuilder<S: Store + 'static> {
    store: Arc<S>,
    loaded: Option<LoadedManifest>,
    manifest_path: Option<Box<Path>>,
    journal: Option<Journal>,
    backend_hooks: Option<Arc<dyn HarnessBackendHooks<S>>>,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
    backend: HarnessBackend,
    effect_mode: EffectMode,
}

impl<S: Store + 'static> HarnessBuilder<S> {
    pub fn ephemeral(store: Arc<S>, loaded: LoadedManifest) -> Self {
        Self {
            store,
            loaded: Some(loaded),
            manifest_path: None,
            journal: None,
            backend_hooks: None,
            world_config: WorldConfig::default(),
            adapter_config: EffectAdapterConfig::default(),
            kernel_config: KernelConfig::default(),
            backend: HarnessBackend::Ephemeral,
            effect_mode: EffectMode::Scripted,
        }
    }

    pub fn from_manifest_path(store: Arc<S>, manifest_path: impl AsRef<Path>) -> Self {
        Self {
            store,
            loaded: None,
            manifest_path: Some(manifest_path.as_ref().to_path_buf().into_boxed_path()),
            journal: None,
            backend_hooks: None,
            world_config: WorldConfig::default(),
            adapter_config: EffectAdapterConfig::default(),
            kernel_config: KernelConfig::default(),
            backend: HarnessBackend::Ephemeral,
            effect_mode: EffectMode::Scripted,
        }
    }

    pub fn backend(mut self, backend: HarnessBackend) -> Self {
        self.backend = backend;
        self
    }

    pub fn effect_mode(mut self, effect_mode: EffectMode) -> Self {
        self.effect_mode = effect_mode;
        self
    }

    pub fn journal(mut self, journal: Journal) -> Self {
        self.journal = Some(journal);
        self
    }

    pub fn backend_hooks(mut self, backend_hooks: Arc<dyn HarnessBackendHooks<S>>) -> Self {
        self.backend_hooks = Some(backend_hooks);
        self
    }

    pub fn world_config(mut self, world_config: WorldConfig) -> Self {
        self.world_config = world_config;
        self
    }

    pub fn adapter_config(mut self, adapter_config: EffectAdapterConfig) -> Self {
        self.adapter_config = adapter_config;
        self
    }

    pub fn kernel_config(mut self, kernel_config: KernelConfig) -> Self {
        self.kernel_config = kernel_config;
        self
    }

    pub fn build(self) -> Result<HarnessCore<S>, HostError> {
        let HarnessBuilder {
            store,
            loaded,
            manifest_path,
            journal,
            backend_hooks,
            world_config,
            adapter_config,
            kernel_config,
            backend,
            effect_mode,
        } = self;

        let (host, manifest_source) = match (loaded, manifest_path, journal) {
            (Some(loaded), _, journal) => {
                let manifest_source = HarnessManifestSource::LoadedManifest(loaded.clone());
                (
                    TestHost::from_loaded_manifest_with_kernel_config_and_journal(
                        store,
                        loaded,
                        world_config.clone(),
                        adapter_config.clone(),
                        kernel_config.clone(),
                        journal.unwrap_or_else(aos_kernel::journal::Journal::new),
                    )?,
                    manifest_source,
                )
            }
            (None, Some(manifest_path), Some(journal)) => {
                let loaded = ManifestLoader::load_from_path(store.as_ref(), &manifest_path)?;
                (
                    TestHost::from_loaded_manifest_with_kernel_config_and_journal(
                        store,
                        loaded,
                        world_config.clone(),
                        adapter_config.clone(),
                        kernel_config.clone(),
                        journal,
                    )?,
                    HarnessManifestSource::ManifestPath(manifest_path.into()),
                )
            }
            (None, Some(manifest_path), None) => (
                TestHost::open_with_config(
                    store,
                    &manifest_path,
                    world_config.clone(),
                    adapter_config.clone(),
                    kernel_config.clone(),
                )?,
                HarnessManifestSource::ManifestPath(manifest_path.into()),
            ),
            (None, None, _) => {
                return Err(HostError::External(
                    "harness builder requires either a loaded manifest or a manifest path".into(),
                ));
            }
        };

        HarnessCore::from_test_host_with_config(
            host,
            backend_hooks,
            backend,
            effect_mode,
            manifest_source,
            world_config,
            adapter_config,
            kernel_config,
        )
    }

    pub fn build_world(self) -> Result<WorldHarness<S>, HostError> {
        Ok(WorldHarness {
            core: self.build()?,
        })
    }

    pub fn build_workflow(
        self,
        workflow: impl Into<String>,
    ) -> Result<WorkflowHarness<S>, HostError> {
        Ok(WorkflowHarness {
            core: self.build()?,
            workflow: workflow.into(),
        })
    }
}

pub struct HarnessCore<S: Store + 'static> {
    host: TestHost<S>,
    runtime: Runtime,
    timer_scheduler: TimerScheduler,
    backend_hooks: Option<Arc<dyn HarnessBackendHooks<S>>>,
    manifest_source: HarnessManifestSource,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
    backend: HarnessBackend,
    effect_mode: EffectMode,
    cycles_run: u64,
    effects_dispatched: u64,
    receipts_applied: u64,
}

impl<S: Store + 'static> HarnessCore<S> {
    fn encode_replay_snapshot<T: Serialize>(value: &T) -> Result<Vec<u8>, HostError> {
        to_canonical_cbor(value)
            .map_err(|err| HostError::External(format!("encode replay snapshot: {err}")))
    }

    pub fn from_world_host(
        host: WorldHost<S>,
        backend: HarnessBackend,
        effect_mode: EffectMode,
        world_config: WorldConfig,
        adapter_config: EffectAdapterConfig,
        kernel_config: KernelConfig,
    ) -> Result<Self, HostError> {
        Self::from_world_host_with_hooks(
            host,
            backend,
            effect_mode,
            world_config,
            adapter_config,
            kernel_config,
            None,
        )
    }

    pub fn from_world_host_with_hooks(
        host: WorldHost<S>,
        backend: HarnessBackend,
        effect_mode: EffectMode,
        world_config: WorldConfig,
        adapter_config: EffectAdapterConfig,
        kernel_config: KernelConfig,
        backend_hooks: Option<Arc<dyn HarnessBackendHooks<S>>>,
    ) -> Result<Self, HostError> {
        let manifest = host
            .kernel()
            .get_manifest(Consistency::Head)
            .map_err(HostError::from)?
            .value;
        let manifest_cbor = to_canonical_cbor(&manifest)
            .map_err(|err| HostError::Manifest(format!("encode manifest: {err}")))?;
        Self::from_test_host_with_config(
            TestHost::from_world_host(host),
            backend_hooks,
            backend,
            effect_mode,
            HarnessManifestSource::ManifestCbor(manifest_cbor),
            world_config,
            adapter_config,
            kernel_config,
        )
    }

    fn from_test_host_with_config(
        mut host: TestHost<S>,
        backend_hooks: Option<Arc<dyn HarnessBackendHooks<S>>>,
        backend: HarnessBackend,
        effect_mode: EffectMode,
        manifest_source: HarnessManifestSource,
        world_config: WorldConfig,
        adapter_config: EffectAdapterConfig,
        kernel_config: KernelConfig,
    ) -> Result<Self, HostError> {
        let mut timer_scheduler = TimerScheduler::new();
        timer_scheduler.rehydrate_daemon_state(host.kernel_mut());
        Ok(Self {
            host,
            runtime: RuntimeBuilder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| HostError::External(format!("build harness runtime: {err}")))?,
            timer_scheduler,
            backend_hooks,
            manifest_source,
            world_config,
            adapter_config,
            kernel_config,
            backend,
            effect_mode,
            cycles_run: 0,
            effects_dispatched: 0,
            receipts_applied: 0,
        })
    }

    pub fn backend(&self) -> HarnessBackend {
        self.backend
    }

    pub fn effect_mode(&self) -> EffectMode {
        self.effect_mode
    }

    pub fn host(&self) -> &TestHost<S> {
        &self.host
    }

    pub fn host_mut(&mut self) -> &mut TestHost<S> {
        &mut self.host
    }

    pub fn kernel(&self) -> &Kernel<S> {
        self.host.kernel()
    }

    pub fn kernel_mut(&mut self) -> &mut Kernel<S> {
        self.host.kernel_mut()
    }

    pub fn adapter_registry(&self) -> &AdapterRegistry {
        self.host.adapter_registry()
    }

    pub fn adapter_registry_mut(&mut self) -> &mut AdapterRegistry {
        self.host.adapter_registry_mut()
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub fn store(&self) -> Arc<S> {
        self.host.host().store_arc()
    }

    pub fn send_event(&mut self, schema: &str, json_value: JsonValue) -> Result<(), HostError> {
        self.host.send_event(schema, json_value)
    }

    pub fn send_event_cbor(&mut self, schema: &str, value: Vec<u8>) -> Result<(), HostError> {
        self.host.send_event_cbor(schema, value)
    }

    pub fn send_command(&mut self, schema: &str, json_value: JsonValue) -> Result<(), HostError> {
        self.send_event(schema, json_value)
    }

    pub fn run_until_kernel_idle(&mut self) -> Result<QuiescenceStatus, HostError> {
        self.host.run_to_idle()?;
        Ok(self.quiescence_status())
    }

    pub fn run_cycle_batch(&mut self) -> Result<CycleOutcome, HostError> {
        let cycle = self
            .runtime
            .block_on(self.host.host_mut().run_cycle(RunMode::Batch))?;
        self.record_cycle(cycle);
        Ok(cycle)
    }

    pub fn run_cycle_with_timers(&mut self) -> Result<CycleOutcome, HostError> {
        let mut cycle = self
            .runtime
            .block_on(self.host.host_mut().run_cycle(RunMode::Daemon {
                scheduler: &mut self.timer_scheduler,
            }))?;
        let fired = self
            .host
            .host_mut()
            .fire_due_timers(&mut self.timer_scheduler)?;
        if fired > 0 {
            let final_drain = self.host.host_mut().drain()?;
            cycle.receipts_applied += fired;
            cycle.final_drain = final_drain;
        }
        self.record_cycle(cycle);
        Ok(cycle)
    }

    pub fn run_until_runtime_quiescent(&mut self) -> Result<QuiescenceStatus, HostError> {
        self.host.run_to_idle()?;
        if self.host.host().has_pending_effects() {
            match self.effect_mode {
                EffectMode::Scripted => {}
                EffectMode::Twin | EffectMode::Live => {
                    let _ = self.run_cycle_with_timers()?;
                }
            }
        }
        Ok(self.quiescence_status())
    }

    pub fn quiescence_status(&self) -> QuiescenceStatus {
        self.host
            .host()
            .quiescence_status(Some(&self.timer_scheduler))
    }

    pub fn pull_effects(&mut self) -> Result<Vec<EffectIntent>, HostError> {
        self.host.drain_effects()
    }

    pub fn apply_receipt(&mut self, receipt: EffectReceipt) -> Result<(), HostError> {
        self.host.apply_receipt(receipt)
    }

    pub fn snapshot(&mut self) -> Result<(), HostError> {
        if let Some(hooks) = &self.backend_hooks {
            hooks.snapshot(&mut self.host)
        } else {
            self.host.snapshot()
        }
    }

    pub fn journal_entries(&self) -> Result<Vec<OwnedJournalEntry>, HostError> {
        self.host.kernel().dump_journal().map_err(HostError::from)
    }

    pub fn state_bytes(&self, workflow: &str, key: Option<&[u8]>) -> Option<Vec<u8>> {
        self.host.state_bytes_for_key(workflow, key)
    }

    pub fn state<T: DeserializeOwned>(
        &self,
        workflow: &str,
        key: Option<&[u8]>,
    ) -> Result<T, HostError> {
        self.host.state_for_key(workflow, key)
    }

    pub fn state_json(&self, workflow: &str, key: Option<&[u8]>) -> Result<JsonValue, HostError> {
        self.host.state_json_for_key(workflow, key)
    }

    pub fn list_cells(&self, workflow: &str) -> Result<Vec<CellMeta>, HostError> {
        self.host.host().list_cells(workflow)
    }

    pub fn trace_summary(&self) -> Result<JsonValue, HostError> {
        self.host.trace_summary()
    }

    pub fn time_get(&self) -> u64 {
        self.host.logical_time_now_ns()
    }

    pub fn time_set(&mut self, now_ns: u64) -> u64 {
        self.host.set_logical_time_ns(now_ns)
    }

    pub fn time_advance(&mut self, delta_ns: u64) -> u64 {
        self.host.advance_logical_time_ns(delta_ns)
    }

    pub fn time_jump_next_due(&mut self) -> Result<Option<u64>, HostError> {
        let Some(next_due_at_ns) = self.timer_scheduler.next_due_at_ns() else {
            return Ok(None);
        };
        self.time_set(next_due_at_ns);
        let fired = self
            .host
            .host_mut()
            .fire_due_timers(&mut self.timer_scheduler)?;
        if fired > 0 {
            self.host.run_to_idle()?;
            self.receipts_applied += fired as u64;
        }
        Ok(Some(next_due_at_ns))
    }

    pub fn export_artifacts(&self) -> Result<HarnessArtifacts, HostError> {
        Ok(HarnessArtifacts {
            evidence: self.evidence(),
            trace_summary: self.trace_summary()?,
            journal_entries: self.journal_entries()?,
        })
    }

    pub fn reopen(&self) -> Result<Self, HostError> {
        let logical_time_ns = self.time_get();
        let host = if let Some(hooks) = &self.backend_hooks {
            if let Some(host) = hooks.reopen(
                &self.host,
                &self.world_config,
                &self.adapter_config,
                &self.kernel_config,
            )? {
                host
            } else {
                let loaded = self.reload_manifest()?;
                let journal = aos_kernel::journal::Journal::from_entries(&self.journal_entries()?)
                    .map_err(|err| HostError::External(err.to_string()))?;
                TestHost::from_loaded_manifest_with_kernel_config_and_journal(
                    self.store(),
                    loaded,
                    self.world_config.clone(),
                    self.adapter_config.clone(),
                    self.kernel_config.clone(),
                    journal,
                )?
            }
        } else {
            let loaded = self.reload_manifest()?;
            let journal = aos_kernel::journal::Journal::from_entries(&self.journal_entries()?)
                .map_err(|err| HostError::External(err.to_string()))?;
            TestHost::from_loaded_manifest_with_kernel_config_and_journal(
                self.store(),
                loaded,
                self.world_config.clone(),
                self.adapter_config.clone(),
                self.kernel_config.clone(),
                journal,
            )?
        };
        let mut reopened = Self::from_test_host_with_config(
            host,
            self.backend_hooks.clone(),
            self.backend,
            self.effect_mode,
            self.manifest_source.clone(),
            self.world_config.clone(),
            self.adapter_config.clone(),
            self.kernel_config.clone(),
        )?;
        reopened.time_set(logical_time_ns);
        reopened.cycles_run = self.cycles_run;
        reopened.effects_dispatched = self.effects_dispatched;
        reopened.receipts_applied = self.receipts_applied;
        Ok(reopened)
    }

    pub fn replay_check(&self) -> Result<HarnessReplayReport, HostError> {
        let reopened = self.reopen()?;
        let mut mismatches = Vec::new();
        if Self::encode_replay_snapshot(&self.kernel().workflow_instances_snapshot())?
            != Self::encode_replay_snapshot(&reopened.kernel().workflow_instances_snapshot())?
        {
            mismatches.push("workflow instances".into());
        }
        if Self::encode_replay_snapshot(&self.kernel().pending_workflow_receipts_snapshot())?
            != Self::encode_replay_snapshot(
                &reopened.kernel().pending_workflow_receipts_snapshot(),
            )?
        {
            mismatches.push("pending workflow receipts".into());
        }
        if Self::encode_replay_snapshot(&self.kernel().queued_effects_snapshot())?
            != Self::encode_replay_snapshot(&reopened.kernel().queued_effects_snapshot())?
        {
            mismatches.push("queued effects".into());
        }
        if self.quiescence_status().timers_pending != reopened.quiescence_status().timers_pending {
            mismatches.push("timer scheduler state".into());
        }
        if self.time_get().abs_diff(reopened.time_get()) > 10_000_000 {
            mismatches.push("logical time".into());
        }

        Ok(HarnessReplayReport {
            ok: mismatches.is_empty(),
            mismatches,
        })
    }

    pub fn receipt_ok<T: Serialize>(
        &self,
        intent_hash: [u8; 32],
        adapter_id: impl Into<String>,
        payload: &T,
    ) -> Result<EffectReceipt, HostError> {
        Self::build_receipt(intent_hash, adapter_id.into(), ReceiptStatus::Ok, payload)
    }

    pub fn receipt_error<T: Serialize>(
        &self,
        intent_hash: [u8; 32],
        adapter_id: impl Into<String>,
        payload: &T,
    ) -> Result<EffectReceipt, HostError> {
        Self::build_receipt(
            intent_hash,
            adapter_id.into(),
            ReceiptStatus::Error,
            payload,
        )
    }

    pub fn receipt_timeout<T: Serialize>(
        &self,
        intent_hash: [u8; 32],
        adapter_id: impl Into<String>,
        payload: &T,
    ) -> Result<EffectReceipt, HostError> {
        Self::build_receipt(
            intent_hash,
            adapter_id.into(),
            ReceiptStatus::Timeout,
            payload,
        )
    }

    pub fn receipt_timer_set_ok(
        &self,
        intent_hash: [u8; 32],
        delivered_at_ns: u64,
        key: Option<String>,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_ok(
            intent_hash,
            "adapter.timer.harness",
            &TimerSetReceipt {
                delivered_at_ns,
                key,
            },
        )
    }

    pub fn receipt_blob_put_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobPutReceipt,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_ok(intent_hash, "adapter.blob.put.harness", payload)
    }

    pub fn receipt_blob_get_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobGetReceipt,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_ok(intent_hash, "adapter.blob.get.harness", payload)
    }

    pub fn receipt_http_request_ok(
        &self,
        intent_hash: [u8; 32],
        status: i32,
        adapter_id: impl Into<String>,
    ) -> Result<EffectReceipt, HostError> {
        let adapter_id = adapter_id.into();
        self.receipt_ok(
            intent_hash,
            adapter_id.clone(),
            &HttpRequestReceipt {
                status,
                headers: Default::default(),
                body_ref: None,
                timings: RequestTimings {
                    start_ns: self.time_get(),
                    end_ns: self.time_get(),
                },
                adapter_id,
            },
        )
    }

    pub fn receipt_llm_generate_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &LlmGenerateReceipt,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_ok(intent_hash, payload.provider_id.clone(), payload)
    }

    pub fn evidence(&self) -> HarnessEvidence {
        let heights = self.host.heights();
        HarnessEvidence {
            backend: match self.backend {
                HarnessBackend::Ephemeral => "ephemeral",
            },
            effect_mode: match self.effect_mode {
                EffectMode::Scripted => "scripted",
                EffectMode::Twin => "twin",
                EffectMode::Live => "live",
            },
            logical_time_ns: self.time_get(),
            heights_head: heights.head,
            heights_snapshot: heights.snapshot,
            cycles_run: self.cycles_run,
            effects_dispatched: self.effects_dispatched,
            receipts_applied: self.receipts_applied,
            quiescence: self.quiescence_status(),
        }
    }

    fn record_cycle(&mut self, cycle: CycleOutcome) {
        self.cycles_run += 1;
        self.effects_dispatched += cycle.effects_dispatched as u64;
        self.receipts_applied += cycle.receipts_applied as u64;
    }

    fn reload_manifest(&self) -> Result<LoadedManifest, HostError> {
        match &self.manifest_source {
            HarnessManifestSource::LoadedManifest(loaded) => Ok(loaded.clone()),
            HarnessManifestSource::ManifestCbor(bytes) => {
                ManifestLoader::load_from_bytes(self.store().as_ref(), bytes)
                    .map_err(HostError::from)
            }
            HarnessManifestSource::ManifestPath(path) => {
                ManifestLoader::load_from_path(self.store().as_ref(), path).map_err(HostError::from)
            }
        }
    }

    fn build_receipt<T: Serialize>(
        intent_hash: [u8; 32],
        adapter_id: String,
        status: ReceiptStatus,
        payload: &T,
    ) -> Result<EffectReceipt, HostError> {
        Ok(EffectReceipt {
            intent_hash,
            adapter_id,
            status,
            payload_cbor: serde_cbor::to_vec(payload)
                .map_err(|err| HostError::External(format!("encode receipt payload: {err}")))?,
            cost_cents: Some(0),
            signature: vec![],
        })
    }
}

pub struct WorldHarness<S: Store + 'static> {
    core: HarnessCore<S>,
}

impl<S: Store + 'static> WorldHarness<S> {
    pub fn from_world_host(
        host: WorldHost<S>,
        backend: HarnessBackend,
        effect_mode: EffectMode,
        world_config: WorldConfig,
        adapter_config: EffectAdapterConfig,
        kernel_config: KernelConfig,
    ) -> Result<Self, HostError> {
        Ok(Self {
            core: HarnessCore::from_world_host(
                host,
                backend,
                effect_mode,
                world_config,
                adapter_config,
                kernel_config,
            )?,
        })
    }

    pub fn from_world_host_with_hooks(
        host: WorldHost<S>,
        backend: HarnessBackend,
        effect_mode: EffectMode,
        world_config: WorldConfig,
        adapter_config: EffectAdapterConfig,
        kernel_config: KernelConfig,
        backend_hooks: Option<Arc<dyn HarnessBackendHooks<S>>>,
    ) -> Result<Self, HostError> {
        Ok(Self {
            core: HarnessCore::from_world_host_with_hooks(
                host,
                backend,
                effect_mode,
                world_config,
                adapter_config,
                kernel_config,
                backend_hooks,
            )?,
        })
    }

    pub fn core(&self) -> &HarnessCore<S> {
        &self.core
    }

    pub fn core_mut(&mut self) -> &mut HarnessCore<S> {
        &mut self.core
    }

    pub fn into_core(self) -> HarnessCore<S> {
        self.core
    }

    pub fn send_event(&mut self, schema: &str, json_value: JsonValue) -> Result<(), HostError> {
        self.core.send_event(schema, json_value)
    }

    pub fn send_event_cbor(&mut self, schema: &str, value: Vec<u8>) -> Result<(), HostError> {
        self.core.send_event_cbor(schema, value)
    }

    pub fn send_command(&mut self, schema: &str, json_value: JsonValue) -> Result<(), HostError> {
        self.core.send_command(schema, json_value)
    }

    pub fn run_until_kernel_idle(&mut self) -> Result<QuiescenceStatus, HostError> {
        self.core.run_until_kernel_idle()
    }

    pub fn run_until_runtime_quiescent(&mut self) -> Result<QuiescenceStatus, HostError> {
        self.core.run_until_runtime_quiescent()
    }

    pub fn run_cycle_batch(&mut self) -> Result<CycleOutcome, HostError> {
        self.core.run_cycle_batch()
    }

    pub fn run_cycle_with_timers(&mut self) -> Result<CycleOutcome, HostError> {
        self.core.run_cycle_with_timers()
    }

    pub fn quiescence_status(&self) -> QuiescenceStatus {
        self.core.quiescence_status()
    }

    pub fn pull_effects(&mut self) -> Result<Vec<EffectIntent>, HostError> {
        self.core.pull_effects()
    }

    pub fn apply_receipt(&mut self, receipt: EffectReceipt) -> Result<(), HostError> {
        self.core.apply_receipt(receipt)
    }

    pub fn snapshot(&mut self) -> Result<(), HostError> {
        self.core.snapshot()
    }

    pub fn reopen(&self) -> Result<Self, HostError> {
        Ok(Self {
            core: self.core.reopen()?,
        })
    }

    pub fn replay_check(&self) -> Result<HarnessReplayReport, HostError> {
        self.core.replay_check()
    }

    pub fn state_bytes(&self, workflow: &str, key: Option<&[u8]>) -> Option<Vec<u8>> {
        self.core.state_bytes(workflow, key)
    }

    pub fn state<T: DeserializeOwned>(
        &self,
        workflow: &str,
        key: Option<&[u8]>,
    ) -> Result<T, HostError> {
        self.core.state(workflow, key)
    }

    pub fn state_json(&self, workflow: &str, key: Option<&[u8]>) -> Result<JsonValue, HostError> {
        self.core.state_json(workflow, key)
    }

    pub fn list_cells(&self, workflow: &str) -> Result<Vec<CellMeta>, HostError> {
        self.core.list_cells(workflow)
    }

    pub fn trace_summary(&self) -> Result<JsonValue, HostError> {
        self.core.trace_summary()
    }

    pub fn time_get(&self) -> u64 {
        self.core.time_get()
    }

    pub fn time_set(&mut self, now_ns: u64) -> u64 {
        self.core.time_set(now_ns)
    }

    pub fn time_advance(&mut self, delta_ns: u64) -> u64 {
        self.core.time_advance(delta_ns)
    }

    pub fn time_jump_next_due(&mut self) -> Result<Option<u64>, HostError> {
        self.core.time_jump_next_due()
    }

    pub fn evidence(&self) -> HarnessEvidence {
        self.core.evidence()
    }

    pub fn export_artifacts(&self) -> Result<HarnessArtifacts, HostError> {
        self.core.export_artifacts()
    }
}

pub struct WorkflowHarness<S: Store + 'static> {
    core: HarnessCore<S>,
    workflow: String,
}

impl<S: Store + 'static> WorkflowHarness<S> {
    pub fn workflow(&self) -> &str {
        &self.workflow
    }

    pub fn core(&self) -> &HarnessCore<S> {
        &self.core
    }

    pub fn core_mut(&mut self) -> &mut HarnessCore<S> {
        &mut self.core
    }

    pub fn send_event(&mut self, schema: &str, json_value: JsonValue) -> Result<(), HostError> {
        self.core.send_event(schema, json_value)
    }

    pub fn send_event_cbor(&mut self, schema: &str, value: Vec<u8>) -> Result<(), HostError> {
        self.core.send_event_cbor(schema, value)
    }

    pub fn send_command(&mut self, schema: &str, json_value: JsonValue) -> Result<(), HostError> {
        self.core.send_command(schema, json_value)
    }

    pub fn run_until_kernel_idle(&mut self) -> Result<QuiescenceStatus, HostError> {
        self.core.run_until_kernel_idle()
    }

    pub fn run_until_runtime_quiescent(&mut self) -> Result<QuiescenceStatus, HostError> {
        self.core.run_until_runtime_quiescent()
    }

    pub fn run_cycle_batch(&mut self) -> Result<CycleOutcome, HostError> {
        self.core.run_cycle_batch()
    }

    pub fn run_cycle_with_timers(&mut self) -> Result<CycleOutcome, HostError> {
        self.core.run_cycle_with_timers()
    }

    pub fn quiescence_status(&self) -> QuiescenceStatus {
        self.core.quiescence_status()
    }

    pub fn pull_effects(&mut self) -> Result<Vec<EffectIntent>, HostError> {
        self.core.pull_effects()
    }

    pub fn apply_receipt(&mut self, receipt: EffectReceipt) -> Result<(), HostError> {
        self.core.apply_receipt(receipt)
    }

    pub fn snapshot(&mut self) -> Result<(), HostError> {
        self.core.snapshot()
    }

    pub fn reopen(&self) -> Result<Self, HostError> {
        Ok(Self {
            core: self.core.reopen()?,
            workflow: self.workflow.clone(),
        })
    }

    pub fn replay_check(&self) -> Result<HarnessReplayReport, HostError> {
        self.core.replay_check()
    }

    pub fn state_bytes(&self, key: Option<&[u8]>) -> Option<Vec<u8>> {
        self.core.state_bytes(&self.workflow, key)
    }

    pub fn state<T: DeserializeOwned>(&self, key: Option<&[u8]>) -> Result<T, HostError> {
        self.core.state(&self.workflow, key)
    }

    pub fn state_json(&self, key: Option<&[u8]>) -> Result<JsonValue, HostError> {
        self.core.state_json(&self.workflow, key)
    }

    pub fn list_cells(&self) -> Result<Vec<CellMeta>, HostError> {
        self.core.list_cells(&self.workflow)
    }

    pub fn trace_summary(&self) -> Result<JsonValue, HostError> {
        self.core.trace_summary()
    }

    pub fn time_get(&self) -> u64 {
        self.core.time_get()
    }

    pub fn time_set(&mut self, now_ns: u64) -> u64 {
        self.core.time_set(now_ns)
    }

    pub fn time_advance(&mut self, delta_ns: u64) -> u64 {
        self.core.time_advance(delta_ns)
    }

    pub fn time_jump_next_due(&mut self) -> Result<Option<u64>, HostError> {
        self.core.time_jump_next_due()
    }

    pub fn evidence(&self) -> HarnessEvidence {
        self.core.evidence()
    }

    pub fn export_artifacts(&self) -> Result<HarnessArtifacts, HostError> {
        self.core.export_artifacts()
    }
}
