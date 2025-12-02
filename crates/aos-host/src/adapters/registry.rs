use std::collections::HashMap;
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
    adapters: HashMap<String, Box<dyn AsyncEffectAdapter>>, 
    config: AdapterRegistryConfig,
}

impl AdapterRegistry {
    pub fn new(config: AdapterRegistryConfig) -> Self {
        Self {
            adapters: HashMap::new(),
            config,
        }
    }

    pub fn register(&mut self, adapter: Box<dyn AsyncEffectAdapter>) {
        self.adapters.insert(adapter.kind().to_string(), adapter);
    }

    pub fn get(&self, kind: &str) -> Option<&dyn AsyncEffectAdapter> {
        self.adapters.get(kind).map(|b| b.as_ref())
    }

    pub async fn execute_batch(&self, intents: Vec<EffectIntent>) -> Vec<EffectReceipt> {
        let mut receipts = Vec::with_capacity(intents.len());

        for intent in intents {
            let receipt = match self.get(intent.kind.as_str()) {
                Some(adapter) => match timeout(self.config.effect_timeout, adapter.execute(&intent)).await
                {
                    Ok(Ok(receipt)) => receipt,
                    Ok(Err(_err)) => EffectReceipt {
                        intent_hash: intent.intent_hash,
                        adapter_id: adapter.kind().to_string(),
                        status: ReceiptStatus::Error,
                        payload_cbor: vec![],
                        cost_cents: None,
                        signature: vec![],
                    },
                    Err(_) => EffectReceipt {
                        intent_hash: intent.intent_hash,
                        adapter_id: adapter.kind().to_string(),
                        status: ReceiptStatus::Timeout,
                        payload_cbor: vec![],
                        cost_cents: None,
                        signature: vec![],
                    },
                },
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
