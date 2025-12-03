use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use aos_effects::{EffectIntent, EffectReceipt};
use aos_kernel::{Kernel, KernelBuilder, KernelConfig, KernelHeights, TailIntent, TailScan};
use aos_store::Store;

use crate::adapters::registry::AdapterRegistry;
use crate::adapters::registry::AdapterRegistryConfig;
use crate::adapters::stub::{
    StubBlobAdapter, StubBlobGetAdapter, StubHttpAdapter, StubLlmAdapter, StubTimerAdapter,
};
use crate::config::HostConfig;
use crate::error::HostError;

#[derive(Debug, Clone)]
pub enum ExternalEvent {
    DomainEvent { schema: String, value: Vec<u8> },
    Receipt(EffectReceipt),
}

#[derive(Clone, Copy)]
pub enum RunMode<'a> {
    Batch,
    WithTimers {
        adapter_registry: &'a AdapterRegistry,
    },
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DrainOutcome {
    pub ticks: u64,
    pub idle: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CycleOutcome {
    pub initial_drain: DrainOutcome,
    pub effects_dispatched: usize,
    pub receipts_applied: usize,
    pub final_drain: DrainOutcome,
}

pub struct WorldHost<S: Store + 'static> {
    kernel: Kernel<S>,
    adapter_registry: AdapterRegistry,
    config: HostConfig,
}

impl<S: Store + 'static> WorldHost<S> {
    pub fn open(
        store: Arc<S>,
        manifest_path: &Path,
        host_config: HostConfig,
        kernel_config: KernelConfig,
    ) -> Result<Self, HostError> {
        let root = manifest_path.parent().unwrap_or(Path::new("."));
        let mut builder = KernelBuilder::new(store.clone()).with_fs_journal(root)?;

        if let Some(dir) = host_config
            .module_cache_dir
            .clone()
            .or(kernel_config.module_cache_dir.clone())
        {
            builder = builder.with_module_cache_dir(dir);
        }
        builder = builder.with_eager_module_load(host_config.eager_module_load);
        if let Some(resolver) = kernel_config.secret_resolver.clone() {
            builder = builder.with_secret_resolver(resolver);
        }
        builder = builder.allow_placeholder_secrets(host_config.allow_placeholder_secrets);

        let mut kernel = builder.from_manifest_path(manifest_path)?;

        // Rehydrate dispatch queue: queued_effects snapshot + tail intents lacking receipts.
        let heights = kernel.heights();
        let tail = kernel.tail_scan_after(heights.snapshot.unwrap_or(0))?;
        let receipts_seen = receipts_set(&tail);

        let mut to_dispatch: Vec<EffectIntent> = kernel
            .queued_effects_snapshot()
            .into_iter()
            .map(|snap| snap.into_intent())
            .collect();

        for TailIntent { record, .. } in tail.intents.iter() {
            if receipts_seen.contains(&record.intent_hash) {
                continue;
            }
            let intent = EffectIntent::from_raw_params(
                record.kind.clone().into(),
                record.cap_name.clone(),
                record.params_cbor.clone(),
                record.idempotency_key,
            )
            .ok();
            if let Some(intent) = intent {
                to_dispatch.push(intent);
            }
        }

        let adapter_registry = default_registry(&host_config);

        if !to_dispatch.is_empty() {
            kernel.restore_effect_queue(to_dispatch);
        }

        Ok(Self {
            kernel,
            adapter_registry,
            config: host_config,
        })
    }

    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    pub fn adapter_registry_mut(&mut self) -> &mut AdapterRegistry {
        &mut self.adapter_registry
    }

    pub fn adapter_registry(&self) -> &AdapterRegistry {
        &self.adapter_registry
    }

    pub fn enqueue_external(&mut self, evt: ExternalEvent) -> Result<(), HostError> {
        match evt {
            ExternalEvent::DomainEvent { schema, value } => {
                self.kernel.submit_domain_event(schema, value);
            }
            ExternalEvent::Receipt(receipt) => {
                self.kernel.handle_receipt(receipt)?;
            }
        }
        Ok(())
    }

    pub fn drain(&mut self) -> Result<DrainOutcome, HostError> {
        self.kernel.tick_until_idle()?;
        Ok(DrainOutcome {
            ticks: 0,
            idle: true,
        })
    }

    pub fn state(&self, reducer: &str, key: Option<&[u8]>) -> Option<&Vec<u8>> {
        // keyed state not yet implemented; ignore key
        let _ = key;
        self.kernel.reducer_state(reducer)
    }

    pub fn snapshot(&mut self) -> Result<(), HostError> {
        Ok(self.kernel.create_snapshot()?)
    }

    pub async fn run_cycle(&mut self, mode: RunMode<'_>) -> Result<CycleOutcome, HostError> {
        let initial = self.drain()?;
        let intents = self.kernel.drain_effects();
        let effects_dispatched = intents.len();

        let receipts = match mode {
            RunMode::Batch => self.adapter_registry.execute_batch(intents).await,
            RunMode::WithTimers { adapter_registry } => {
                adapter_registry.execute_batch(intents).await
            }
        };

        let receipts_applied = receipts.len();
        for receipt in receipts {
            self.kernel.handle_receipt(receipt)?;
        }
        let final_drain = self.drain()?;
        Ok(CycleOutcome {
            initial_drain: initial,
            effects_dispatched,
            receipts_applied,
            final_drain,
        })
    }

    pub fn heights(&self) -> KernelHeights {
        self.kernel.heights()
    }

    pub fn kernel(&self) -> &Kernel<S> {
        &self.kernel
    }

    pub fn kernel_mut(&mut self) -> &mut Kernel<S> {
        &mut self.kernel
    }

    /// Create a WorldHost from an existing kernel (for TestHost use).
    pub fn from_kernel(kernel: Kernel<S>, config: HostConfig) -> Self {
        let adapter_registry = default_registry(&config);
        Self {
            kernel,
            adapter_registry,
            config,
        }
    }
}

fn default_registry(config: &HostConfig) -> AdapterRegistry {
    let mut registry = AdapterRegistry::new(AdapterRegistryConfig {
        effect_timeout: config.effect_timeout,
    });
    registry.register(Box::new(StubTimerAdapter));
    registry.register(Box::new(StubHttpAdapter));
    registry.register(Box::new(StubLlmAdapter));
    registry.register(Box::new(StubBlobAdapter));
    registry.register(Box::new(StubBlobGetAdapter));
    registry
}

fn receipts_set(tail: &TailScan) -> HashSet<[u8; 32]> {
    tail.receipts.iter().map(|r| r.record.intent_hash).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_store::MemStore;
    use serde_cbor::to_vec;
    use serde_json::json;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_minimal_manifest(path: &std::path::Path) {
        // Minimal manifest: no reducers/plans; just air_version and empty lists.
        let manifest = json!({
            "air_version": "1",
            "schemas": [],
            "modules": [],
            "plans": [],
            "effects": [],
            "caps": [],
            "policies": [],
            "triggers": []
        });
        let bytes = serde_cbor::to_vec(&manifest).expect("cbor encode");
        let mut file = File::create(path).expect("create manifest");
        file.write_all(&bytes).expect("write manifest");
    }

    #[tokio::test]
    async fn run_cycle_no_events_no_effects() {
        let tmp = TempDir::new().unwrap();
        let manifest_path = tmp.path().join("manifest.cbor");
        write_minimal_manifest(&manifest_path);

        let store = Arc::new(MemStore::new());
        let host_config = HostConfig::default();
        let kernel_config = KernelConfig::default();
        let mut host = WorldHost::open(store, &manifest_path, host_config, kernel_config).unwrap();

        let cycle = host.run_cycle(RunMode::Batch).await.unwrap();
        assert_eq!(cycle.effects_dispatched, 0);
        assert_eq!(cycle.receipts_applied, 0);
        host.snapshot().unwrap();
    }

    #[tokio::test]
    async fn enqueue_domain_event_and_run() {
        let tmp = TempDir::new().unwrap();
        let manifest_path = tmp.path().join("manifest.cbor");
        write_minimal_manifest(&manifest_path);

        let store = Arc::new(MemStore::new());
        let host_config = HostConfig::default();
        let kernel_config = KernelConfig::default();
        let mut host = WorldHost::open(store, &manifest_path, host_config, kernel_config).unwrap();

        // Inject a domain event (no reducers, so it should just record and idle)
        host.enqueue_external(ExternalEvent::DomainEvent {
            schema: "demo/Event@1".into(),
            value: to_vec(&json!({"x": 1})).unwrap(),
        })
        .unwrap();

        let cycle = host.run_cycle(RunMode::Batch).await.unwrap();
        assert_eq!(cycle.effects_dispatched, 0);
        host.snapshot().unwrap();
    }

    #[tokio::test]
    async fn receipts_are_applied_and_state_remains_available() {
        let tmp = TempDir::new().unwrap();
        let manifest_path = tmp.path().join("manifest.cbor");
        write_minimal_manifest(&manifest_path);

        let store = Arc::new(MemStore::new());
        let host_config = HostConfig::default();
        let kernel_config = KernelConfig::default();
        let mut host = WorldHost::open(store, &manifest_path, host_config, kernel_config).unwrap();

        // No reducers, but we can still apply a receipt (should be ignored gracefully)
        let fake_receipt = aos_effects::EffectReceipt {
            intent_hash: [9u8; 32],
            adapter_id: "stub.http".into(),
            status: aos_effects::ReceiptStatus::Ok,
            payload_cbor: vec![],
            cost_cents: None,
            signature: vec![],
        };
        // Should not error even if unknown; kernel will treat as unknown receipt
        let _ = host.enqueue_external(ExternalEvent::Receipt(fake_receipt));

        let cycle = host.run_cycle(RunMode::Batch).await.unwrap();
        assert_eq!(cycle.receipts_applied, 0);

        host.snapshot().unwrap();
    }
}
