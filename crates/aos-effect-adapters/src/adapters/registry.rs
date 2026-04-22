use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use aos_effects::EffectIntent;
use log::warn;

use super::traits::{AdapterStartContext, AsyncEffectAdapter, EffectUpdateSender};

pub struct AdapterRegistry {
    adapters: HashMap<String, Arc<dyn AsyncEffectAdapter>>,
    routes: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterStartError {
    MissingRoute {
        route_id: String,
    },
    MissingAdapter {
        route_id: String,
        adapter_kind: String,
    },
}

impl AdapterRegistry {
    pub fn new() -> Self {
        Self {
            adapters: HashMap::new(),
            routes: HashMap::new(),
        }
    }

    pub fn register(&mut self, adapter: Box<dyn AsyncEffectAdapter>) {
        let adapter: Arc<dyn AsyncEffectAdapter> = Arc::from(adapter);
        let kind = adapter.kind().to_string();
        self.adapters.insert(kind.clone(), adapter);
        self.routes.insert(kind.clone(), kind);
    }

    pub fn register_route(&mut self, route_id: impl Into<String>, adapter_kind: &str) -> bool {
        if !self.adapters.contains_key(adapter_kind) {
            return false;
        }
        self.routes
            .insert(route_id.into(), adapter_kind.to_string());
        true
    }

    pub fn get(&self, kind: &str) -> Option<&dyn AsyncEffectAdapter> {
        self.adapters.get(kind).map(|a| a.as_ref())
    }

    pub fn get_route(&self, route_id: &str) -> Option<&dyn AsyncEffectAdapter> {
        let kind = self.routes.get(route_id)?;
        self.get(kind)
    }

    pub fn has_route(&self, route_id: &str) -> bool {
        self.routes.contains_key(route_id)
    }

    pub fn route_ids(&self) -> Vec<String> {
        let mut routes: Vec<String> = self.routes.keys().cloned().collect();
        routes.sort();
        routes
    }

    pub fn route_mappings(&self) -> BTreeMap<String, String> {
        self.routes
            .iter()
            .map(|(route_id, adapter_kind)| (route_id.clone(), adapter_kind.clone()))
            .collect()
    }

    pub fn ensure_started(
        &self,
        intent: EffectIntent,
        updates: EffectUpdateSender,
    ) -> Result<(), AdapterStartError> {
        let route_id = intent.effect.clone();
        self.ensure_started_routed(intent, route_id, updates)
    }

    pub fn ensure_started_routed(
        &self,
        intent: EffectIntent,
        route_id: String,
        updates: EffectUpdateSender,
    ) -> Result<(), AdapterStartError> {
        self.ensure_started_routed_with_context(intent, route_id, None, updates)
    }

    pub fn ensure_started_routed_with_context(
        &self,
        intent: EffectIntent,
        route_id: String,
        context: Option<AdapterStartContext>,
        updates: EffectUpdateSender,
    ) -> Result<(), AdapterStartError> {
        let adapter_kind =
            self.routes
                .get(&route_id)
                .cloned()
                .ok_or_else(|| AdapterStartError::MissingRoute {
                    route_id: route_id.clone(),
                })?;
        let adapter = self.adapters.get(&adapter_kind).cloned().ok_or_else(|| {
            AdapterStartError::MissingAdapter {
                route_id: route_id.clone(),
                adapter_kind: adapter_kind.clone(),
            }
        })?;
        tokio::spawn(async move {
            if let Err(err) = adapter
                .ensure_started_with_context(intent, context, updates)
                .await
            {
                warn!("route '{route_id}' failed after start: {err:#}");
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_effects::{EffectReceipt, ReceiptStatus};
    use async_trait::async_trait;
    use tokio::sync::mpsc;

    struct MismatchedHashAdapter;

    #[async_trait]
    impl AsyncEffectAdapter for MismatchedHashAdapter {
        fn kind(&self) -> &str {
            "mismatched"
        }

        async fn run_terminal(
            &self,
            _intent: &EffectIntent,
        ) -> anyhow::Result<aos_effects::EffectReceipt> {
            Ok(EffectReceipt {
                intent_hash: [9; 32],
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

        async fn run_terminal(
            &self,
            _intent: &EffectIntent,
        ) -> anyhow::Result<aos_effects::EffectReceipt> {
            panic!("boom");
        }
    }

    fn test_intent(effect: &str) -> EffectIntent {
        EffectIntent::from_raw_params(
            effect,
            serde_cbor::to_vec(&serde_json::json!({ "ok": true })).expect("params"),
            [3; 32],
        )
        .expect("intent")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ensure_started_routed_emits_adapter_updates() {
        let mut registry = AdapterRegistry::new();
        registry.register(Box::new(MismatchedHashAdapter));
        assert!(registry.register_route("host.llm.test", "mismatched"));
        let intent = test_intent("llm.generate");
        let (tx, mut rx) = mpsc::channel(4);

        registry
            .ensure_started_routed(intent, "host.llm.test".into(), tx)
            .expect("start adapter");

        let update = rx.recv().await.expect("receipt update");
        let super::super::traits::EffectUpdate::Receipt(receipt) = update else {
            panic!("expected terminal receipt");
        };
        assert_eq!(receipt.intent_hash, [9; 32]);
        assert_eq!(receipt.status, ReceiptStatus::Ok);
        assert_eq!(receipt.payload_cbor, vec![1, 2, 3]);
        assert_eq!(receipt.cost_cents, Some(7));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ensure_started_routed_accepts_panicking_adapter_start() {
        let mut registry = AdapterRegistry::new();
        registry.register(Box::new(PanicAdapter));
        assert!(registry.register_route("host.llm.test", "panic"));
        let intent = test_intent("llm.generate");
        let (tx, mut rx) = mpsc::channel(4);

        registry
            .ensure_started_routed(intent, "host.llm.test".into(), tx)
            .expect("start adapter");

        assert!(
            rx.recv().await.is_none(),
            "panic should close update channel"
        );
    }
}
