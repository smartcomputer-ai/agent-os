use std::path::Path;
use std::sync::Arc;

use aos_kernel::KernelConfig;
use aos_store::Store;

use crate::config::HostConfig;
use crate::error::HostError;
use crate::host::{ExternalEvent, RunMode, WorldHost};

/// Thin test-only wrapper over WorldHost with convenience helpers.
pub struct TestHost<S: Store + 'static> {
    host: WorldHost<S>,
}

impl<S: Store + 'static> TestHost<S> {
    pub fn open(store: Arc<S>, manifest_path: &Path) -> Result<Self, HostError> {
        let host = WorldHost::open(store, manifest_path, HostConfig::default(), KernelConfig::default())?;
        Ok(Self { host })
    }

    pub fn send_event(&mut self, schema: &str, json_value: serde_json::Value) -> Result<(), HostError> {
        let cbor = serde_cbor::to_vec(&json_value).map_err(|e| HostError::External(e.to_string()))?;
        self.host.enqueue_external(ExternalEvent::DomainEvent {
            schema: schema.to_string(),
            value: cbor,
        })
    }

    pub fn inject_receipt(&mut self, receipt: aos_effects::EffectReceipt) -> Result<(), HostError> {
        self.host.enqueue_external(ExternalEvent::Receipt(receipt))
    }

    pub async fn run_cycle_batch(&mut self) -> Result<(), HostError> {
        self.host.run_cycle(RunMode::Batch).await.map(|_| ())
    }

    pub fn snapshot(&mut self) -> Result<(), HostError> {
        self.host.snapshot()
    }

    pub fn state_bytes(&self, reducer: &str) -> Option<&Vec<u8>> {
        self.host.state(reducer, None)
    }
}
