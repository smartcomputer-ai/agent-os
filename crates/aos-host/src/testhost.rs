use std::path::Path;
use std::sync::Arc;

use aos_effects::{EffectIntent, EffectReceipt};
use aos_kernel::{Kernel, KernelConfig, LoadedManifest};
use aos_store::Store;
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use serde_cbor;

use crate::adapters::registry::AdapterRegistry;
use crate::adapters::traits::AsyncEffectAdapter;
use crate::adapters::timer::TimerScheduler;
use crate::config::HostConfig;
use crate::error::HostError;
use crate::host::{CycleOutcome, ExternalEvent, RunMode, WorldHost};

/// Thin test-only wrapper over WorldHost with convenience helpers.
///
/// `TestHost` provides a convenient interface for writing integration tests
/// that exercise the full host stack (kernel + adapters + effect dispatch).
pub struct TestHost<S: Store + 'static> {
    host: WorldHost<S>,
}

impl<S: Store + 'static> TestHost<S> {
    /// Wrap an existing WorldHost (escape hatch for custom setup in examples/tests).
    pub fn from_world_host(host: WorldHost<S>) -> Self {
        Self { host }
    }

    /// Open a TestHost from a manifest file path.
    pub fn open(store: Arc<S>, manifest_path: &Path) -> Result<Self, HostError> {
        let host = WorldHost::open(
            store,
            manifest_path,
            HostConfig::default(),
            KernelConfig::default(),
        )?;
        Ok(Self { host })
    }

    /// Open a TestHost with custom config.
    pub fn open_with_config(
        store: Arc<S>,
        manifest_path: &Path,
        host_config: HostConfig,
        kernel_config: KernelConfig,
    ) -> Result<Self, HostError> {
        let host = WorldHost::open(store, manifest_path, host_config, kernel_config)?;
        Ok(Self { host })
    }

    /// Create a TestHost from a pre-loaded manifest (in-memory journal).
    pub fn from_loaded_manifest(store: Arc<S>, loaded: LoadedManifest) -> Result<Self, HostError> {
        Self::from_loaded_manifest_with_config(store, loaded, HostConfig::default())
    }

    /// Create a TestHost from a pre-loaded manifest with custom host config.
    pub fn from_loaded_manifest_with_config(
        store: Arc<S>,
        loaded: LoadedManifest,
        host_config: HostConfig,
    ) -> Result<Self, HostError> {
        let kernel = Kernel::from_loaded_manifest(
            store.clone(),
            loaded,
            Box::new(aos_kernel::journal::mem::MemJournal::new()),
        )?;
        let host = WorldHost::from_kernel(kernel, store, host_config);
        Ok(Self { host })
    }

    /// Send a domain event to the host.
    pub fn send_event(
        &mut self,
        schema: &str,
        json_value: serde_json::Value,
    ) -> Result<(), HostError> {
        let cbor =
            serde_cbor::to_vec(&json_value).map_err(|e| HostError::External(e.to_string()))?;
        self.host.enqueue_external(ExternalEvent::DomainEvent {
            schema: schema.to_string(),
            value: cbor,
        })
    }

    /// Send a domain event with CBOR-encoded value.
    pub fn send_event_cbor(&mut self, schema: &str, value: Vec<u8>) -> Result<(), HostError> {
        self.host.enqueue_external(ExternalEvent::DomainEvent {
            schema: schema.to_string(),
            value,
        })
    }

    /// Inject an effect receipt.
    pub fn inject_receipt(&mut self, receipt: EffectReceipt) -> Result<(), HostError> {
        self.host.enqueue_external(ExternalEvent::Receipt(receipt))
    }

    /// Run a batch cycle.
    ///
    /// In batch mode, all effects (including timers) are dispatched via the
    /// adapter registry. Timers fire immediately via StubTimerAdapter.
    pub async fn run_cycle_batch(&mut self) -> Result<CycleOutcome, HostError> {
        self.host.run_cycle(RunMode::Batch).await
    }

    /// Run a daemon-style cycle where timers are scheduled then fired immediately for tests.
    ///
    /// This mirrors the daemon path (`RunMode::Daemon`) but immediately fires due timers so tests
    /// stay deterministic without wall-clock sleeps.
    pub async fn run_cycle_with_timers(&mut self) -> Result<CycleOutcome, HostError> {
        let mut scheduler = TimerScheduler::new();
        let mut cycle = self
            .host
            .run_cycle(RunMode::Daemon {
                scheduler: &mut scheduler,
            })
            .await?;

        // Immediately build receipts for any due timers and drain.
        let fired = self.host.fire_due_timers(&mut scheduler)?;
        if fired > 0 {
            let final_drain = self.host.drain()?;
            cycle.receipts_applied += fired;
            cycle.final_drain = final_drain;
        }
        Ok(cycle)
    }

    /// Drain the kernel until idle without dispatching effects (pure tick loop).
    pub fn run_to_idle(&mut self) -> Result<(), HostError> {
        self.host.drain().map(|_| ())
    }

    /// Drain effects then dispatch them through the adapter registry, applying receipts.
    ///
    /// Useful for tests that want to observe intents before dispatching.
    pub async fn drain_and_dispatch(&mut self) -> Result<CycleOutcome, HostError> {
        let intents = self.host.kernel_mut().drain_effects();
        let effects_dispatched = intents.len();
        let receipts = self.host.adapter_registry_mut().execute_batch(intents).await;
        let receipts_applied = receipts.len();
        for receipt in receipts {
            self.host.kernel_mut().handle_receipt(receipt)?;
        }
        let final_drain = self.host.drain()?;
        Ok(CycleOutcome {
            initial_drain: Default::default(),
            effects_dispatched,
            receipts_applied,
            final_drain,
        })
    }

    /// Create a snapshot.
    pub fn snapshot(&mut self) -> Result<(), HostError> {
        self.host.snapshot()
    }

    /// Get reducer state as raw bytes.
    pub fn state_bytes(&self, reducer: &str) -> Option<&Vec<u8>> {
        self.host.state(reducer, None)
    }

    /// Get reducer state deserialized to type T.
    pub fn state<T: DeserializeOwned>(&self, reducer: &str) -> Result<T, HostError> {
        let bytes = self
            .host
            .state(reducer, None)
            .ok_or_else(|| HostError::External(format!("reducer '{reducer}' has no state")))?;
        serde_cbor::from_slice(bytes).map_err(|e| HostError::External(e.to_string()))
    }

    /// Get reducer state decoded to JSON for quick assertions/logging.
    pub fn state_json(&self, reducer: &str) -> Result<JsonValue, HostError> {
        let bytes = self
            .host
            .state(reducer, None)
            .ok_or_else(|| HostError::External(format!("reducer '{reducer}' has no state")))?;
        let cbor_value: serde_cbor::Value =
            serde_cbor::from_slice(bytes).map_err(|e| HostError::External(e.to_string()))?;
        serde_json::to_value(cbor_value).map_err(|e| HostError::External(e.to_string()))
    }

    /// Drain pending effects from the kernel.
    pub fn drain_effects(&mut self) -> Vec<EffectIntent> {
        self.host.kernel_mut().drain_effects()
    }

    /// Apply a receipt directly (bypassing adapter execution).
    pub fn apply_receipt(&mut self, receipt: EffectReceipt) -> Result<(), HostError> {
        self.host.enqueue_external(ExternalEvent::Receipt(receipt))
    }

    /// Register a custom adapter.
    pub fn register_adapter(&mut self, adapter: Box<dyn AsyncEffectAdapter>) {
        self.host.adapter_registry_mut().register(adapter);
    }

    /// Access the adapter registry.
    pub fn adapter_registry(&self) -> &AdapterRegistry {
        self.host.adapter_registry()
    }

    /// Mutably access the adapter registry.
    pub fn adapter_registry_mut(&mut self) -> &mut AdapterRegistry {
        self.host.adapter_registry_mut()
    }

    /// Access the underlying kernel (escape hatch for advanced tests).
    pub fn kernel(&self) -> &Kernel<S> {
        self.host.kernel()
    }

    /// Mutably access the underlying kernel (escape hatch for advanced tests).
    pub fn kernel_mut(&mut self) -> &mut Kernel<S> {
        self.host.kernel_mut()
    }

    /// Access the underlying WorldHost.
    pub fn host(&self) -> &WorldHost<S> {
        &self.host
    }

    /// Mutably access the underlying WorldHost.
    pub fn host_mut(&mut self) -> &mut WorldHost<S> {
        &mut self.host
    }

    /// Get kernel heights (journal position).
    pub fn heights(&self) -> aos_kernel::KernelHeights {
        self.host.heights()
    }
}
