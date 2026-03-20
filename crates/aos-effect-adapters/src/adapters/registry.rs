use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use log::warn;
use tokio::task::JoinHandle;
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

fn normalize_receipt_intent_hash(
    mut receipt: EffectReceipt,
    expected_intent_hash: [u8; 32],
    adapter_id: &str,
) -> EffectReceipt {
    if receipt.intent_hash != expected_intent_hash {
        warn!(
            "adapter '{adapter_id}' returned receipt intent_hash {} but dispatch expected {}; rewriting receipt to claimed intent",
            hex::encode(receipt.intent_hash),
            hex::encode(expected_intent_hash),
        );
        receipt.intent_hash = expected_intent_hash;
    }
    receipt
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
        if intents.is_empty() {
            return Vec::new();
        }

        let mut receipts = vec![None; intents.len()];
        let mut handles: Vec<(usize, [u8; 32], String, JoinHandle<EffectReceipt>)> = Vec::new();

        for (idx, (intent, adapter_id)) in intents.into_iter().enumerate() {
            let Some(adapter_kind) = self.routes.get(&adapter_id).cloned() else {
                receipts[idx] = Some(EffectReceipt {
                    intent_hash: intent.intent_hash,
                    adapter_id: "adapter.missing".into(),
                    status: ReceiptStatus::Error,
                    payload_cbor: vec![],
                    cost_cents: None,
                    signature: vec![],
                });
                continue;
            };
            let Some(adapter) = self.adapters.get(&adapter_kind).cloned() else {
                receipts[idx] = Some(EffectReceipt {
                    intent_hash: intent.intent_hash,
                    adapter_id: "adapter.missing".into(),
                    status: ReceiptStatus::Error,
                    payload_cbor: vec![],
                    cost_cents: None,
                    signature: vec![],
                });
                continue;
            };
            let effect_timeout = self.config.effect_timeout;
            handles.push((
                idx,
                intent.intent_hash,
                adapter_id.clone(),
                tokio::spawn(async move {
                    match timeout(effect_timeout, adapter.execute(&intent)).await {
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
                }),
            ));
        }

        for (idx, expected_intent_hash, adapter_id, handle) in handles {
            let receipt = match handle.await {
                Ok(receipt) => {
                    normalize_receipt_intent_hash(receipt, expected_intent_hash, &adapter_id)
                }
                Err(_) => EffectReceipt {
                    intent_hash: expected_intent_hash,
                    adapter_id: "adapter.join.error".into(),
                    status: ReceiptStatus::Error,
                    payload_cbor: vec![],
                    cost_cents: None,
                    signature: vec![],
                },
            };
            receipts[idx] = Some(receipt);
        }

        receipts
            .into_iter()
            .map(|receipt| receipt.expect("receipt for each routed effect"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct MismatchedHashAdapter;

    #[async_trait]
    impl AsyncEffectAdapter for MismatchedHashAdapter {
        fn kind(&self) -> &str {
            "mismatched"
        }

        async fn execute(&self, _intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
            Ok(EffectReceipt {
                intent_hash: [9; 32],
                adapter_id: "adapter.mismatched".into(),
                status: ReceiptStatus::Ok,
                payload_cbor: vec![1, 2, 3],
                cost_cents: Some(7),
                signature: vec![4; 64],
            })
        }
    }

    struct PanicAdapter;

    #[async_trait]
    impl AsyncEffectAdapter for PanicAdapter {
        fn kind(&self) -> &str {
            "panic"
        }

        async fn execute(&self, _intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
            panic!("boom");
        }
    }

    fn test_intent(effect_kind: &str) -> EffectIntent {
        EffectIntent::from_raw_params(
            effect_kind.into(),
            effect_kind,
            serde_cbor::to_vec(&serde_json::json!({ "ok": true })).expect("params"),
            [3; 32],
        )
        .expect("intent")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_batch_routed_rewrites_mismatched_adapter_receipt_hash() {
        let mut registry = AdapterRegistry::new(AdapterRegistryConfig::default());
        registry.register(Box::new(MismatchedHashAdapter));
        assert!(registry.register_route("host.llm.test", "mismatched"));
        let intent = test_intent("llm.generate");

        let receipts = registry
            .execute_batch_routed(vec![(intent.clone(), "host.llm.test".into())])
            .await;

        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].intent_hash, intent.intent_hash);
        assert_eq!(receipts[0].adapter_id, "adapter.mismatched");
        assert_eq!(receipts[0].status, ReceiptStatus::Ok);
        assert_eq!(receipts[0].payload_cbor, vec![1, 2, 3]);
        assert_eq!(receipts[0].cost_cents, Some(7));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_batch_routed_preserves_claimed_hash_on_join_error() {
        let mut registry = AdapterRegistry::new(AdapterRegistryConfig::default());
        registry.register(Box::new(PanicAdapter));
        assert!(registry.register_route("host.llm.test", "panic"));
        let intent = test_intent("llm.generate");

        let receipts = registry
            .execute_batch_routed(vec![(intent.clone(), "host.llm.test".into())])
            .await;

        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].intent_hash, intent.intent_hash);
        assert_eq!(receipts[0].adapter_id, "adapter.join.error");
        assert_eq!(receipts[0].status, ReceiptStatus::Error);
    }
}
