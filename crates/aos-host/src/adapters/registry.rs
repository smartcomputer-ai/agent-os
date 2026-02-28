use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use tokio::time::timeout;

use super::traits::AsyncEffectAdapter;

#[derive(Clone)]
pub struct AdapterRegistryConfig {
    pub effect_timeout: Duration,
}

impl Default for AdapterRegistryConfig {
    fn default() -> Self {
        Self {
            effect_timeout: Duration::from_secs(30),
        }
    }
}

pub struct AdapterRegistry {
    adapters: HashMap<String, Arc<dyn AsyncEffectAdapter>>,
    routes: HashMap<String, String>,
    config: AdapterRegistryConfig,
}

impl AdapterRegistry {
    pub fn new(config: AdapterRegistryConfig) -> Self {
        Self {
            adapters: HashMap::new(),
            routes: HashMap::new(),
            config,
        }
    }

    pub fn register(&mut self, adapter: Box<dyn AsyncEffectAdapter>) {
        let adapter: Arc<dyn AsyncEffectAdapter> = Arc::from(adapter);
        let kind = adapter.kind().to_string();
        self.adapters.insert(kind.clone(), adapter);
        self.routes.insert(kind.clone(), kind);
    }

    pub fn register_route(&mut self, adapter_id: impl Into<String>, adapter_kind: &str) -> bool {
        if !self.adapters.contains_key(adapter_kind) {
            return false;
        }
        self.routes
            .insert(adapter_id.into(), adapter_kind.to_string());
        true
    }

    pub fn get(&self, kind: &str) -> Option<&dyn AsyncEffectAdapter> {
        self.adapters.get(kind).map(|a| a.as_ref())
    }

    pub fn get_route(&self, adapter_id: &str) -> Option<&dyn AsyncEffectAdapter> {
        let kind = self.routes.get(adapter_id)?;
        self.get(kind)
    }

    pub fn has_route(&self, adapter_id: &str) -> bool {
        self.routes.contains_key(adapter_id)
    }

    pub fn route_ids(&self) -> Vec<String> {
        let mut routes: Vec<String> = self.routes.keys().cloned().collect();
        routes.sort();
        routes
    }

    pub fn route_mappings(&self) -> BTreeMap<String, String> {
        self.routes
            .iter()
            .map(|(adapter_id, adapter_kind)| (adapter_id.clone(), adapter_kind.clone()))
            .collect()
    }

    pub async fn execute_batch(&self, intents: Vec<EffectIntent>) -> Vec<EffectReceipt> {
        let routed: Vec<(EffectIntent, String)> = intents
            .into_iter()
            .map(|intent| {
                let route = intent.kind.as_str().to_string();
                (intent, route)
            })
            .collect();
        self.execute_batch_routed(routed).await
    }

    pub async fn execute_batch_routed(
        &self,
        intents: Vec<(EffectIntent, String)>,
    ) -> Vec<EffectReceipt> {
        let mut receipts = Vec::with_capacity(intents.len());

        for (intent, adapter_id) in intents {
            let receipt = match self.get_route(&adapter_id) {
                Some(adapter) => {
                    match timeout(self.config.effect_timeout, adapter.execute(&intent)).await {
                        Ok(Ok(receipt)) => receipt,
                        Ok(Err(_err)) => EffectReceipt {
                            intent_hash: intent.intent_hash,
                            adapter_id: adapter_id.clone(),
                            status: ReceiptStatus::Error,
                            payload_cbor: vec![],
                            cost_cents: None,
                            signature: vec![],
                        },
                        Err(_) => EffectReceipt {
                            intent_hash: intent.intent_hash,
                            adapter_id: adapter_id.clone(),
                            status: ReceiptStatus::Timeout,
                            payload_cbor: vec![],
                            cost_cents: None,
                            signature: vec![],
                        },
                    }
                }
                None => EffectReceipt {
                    intent_hash: intent.intent_hash,
                    adapter_id: "adapter.missing".into(),
                    status: ReceiptStatus::Error,
                    payload_cbor: vec![],
                    cost_cents: None,
                    signature: vec![],
                },
            };
            receipts.push(receipt);
        }

        receipts
    }
}
