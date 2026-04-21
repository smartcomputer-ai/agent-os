use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use aos_effect_adapters::config::EffectAdapterConfig;
use aos_effect_adapters::default_registry;
use aos_effect_adapters::registry::AdapterRegistry;
use aos_effect_adapters::traits::{AdapterStartContext, EffectUpdate};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use aos_kernel::{LoadedManifest, Store, WorldInput};
use serde::Serialize;
use tokio::sync::mpsc;

use super::error::RuntimeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectExecutionClass {
    InlineInternal,
    OwnerLocalTimer,
    ExternalAsync,
}

#[derive(Debug, Clone, Serialize)]
pub struct EffectRouteDiagnostics {
    pub strict_effect_bindings: bool,
    pub compatibility_fallback_enabled: bool,
    pub world_requires: BTreeMap<String, String>,
    pub host_provides: BTreeMap<String, String>,
    pub compatibility_fallback_kinds: Vec<String>,
}

impl From<&EffectRouteDiagnostics> for aos_kernel::TraceRouteDiagnostics {
    fn from(value: &EffectRouteDiagnostics) -> Self {
        Self {
            strict_effect_bindings: value.strict_effect_bindings,
            compatibility_fallback_enabled: value.compatibility_fallback_enabled,
            world_requires: value.world_requires.clone(),
            host_provides: value.host_provides.clone(),
            compatibility_fallback_kinds: value.compatibility_fallback_kinds.clone(),
        }
    }
}

#[derive(Debug)]
pub enum EffectRuntimeEvent<W> {
    WorldInput { world_id: W, input: WorldInput },
}

#[derive(Clone)]
pub struct SharedEffectRuntime<W> {
    adapter_registry: Arc<AdapterRegistry>,
    inflight: Arc<Mutex<HashSet<(W, [u8; 32])>>>,
    continuation_tx: mpsc::Sender<EffectRuntimeEvent<W>>,
}

pub struct EffectRuntime<W> {
    shared: SharedEffectRuntime<W>,
    effect_routes: HashMap<String, String>,
    route_diagnostics: EffectRouteDiagnostics,
}

impl<W> SharedEffectRuntime<W>
where
    W: Clone + Eq + Hash + Send + 'static,
{
    pub fn new<S: Store + 'static>(
        store: Arc<S>,
        adapter_config: &EffectAdapterConfig,
        continuation_tx: mpsc::Sender<EffectRuntimeEvent<W>>,
    ) -> Self {
        Self {
            adapter_registry: Arc::new(default_registry(store, adapter_config)),
            inflight: Arc::new(Mutex::new(HashSet::new())),
            continuation_tx,
        }
    }

    pub fn host_route_mappings(&self) -> BTreeMap<String, String> {
        self.adapter_registry.route_mappings()
    }

    pub fn ensure_started_routed(
        &self,
        world_id: W,
        route_id: String,
        intent: EffectIntent,
        start_context: Option<AdapterStartContext>,
    ) -> Result<bool, RuntimeError> {
        let key = (world_id.clone(), intent.intent_hash);
        {
            let mut inflight = self.inflight.lock().map_err(|_| {
                RuntimeError::Execution("effect runtime inflight set poisoned".into())
            })?;
            if !inflight.insert(key) {
                return Ok(false);
            }
        }

        let intent_hash = intent.intent_hash;
        let (update_tx, mut update_rx) = mpsc::channel(16);
        let registry = Arc::clone(&self.adapter_registry);
        let normalize_context = start_context.clone();
        if let Err(err) = registry.ensure_started_routed_with_context(
            intent,
            route_id.clone(),
            start_context,
            update_tx,
        ) {
            let continuation_tx = self.continuation_tx.clone();
            let inflight = Arc::clone(&self.inflight);
            let world_id = world_id.clone();
            tokio::spawn(async move {
                let _ = continuation_tx
                    .send(EffectRuntimeEvent::WorldInput {
                        world_id: world_id.clone(),
                        input: WorldInput::Receipt(adapter_start_error_receipt(
                            intent_hash,
                            &route_id,
                        )),
                    })
                    .await;
                if let Ok(mut guard) = inflight.lock() {
                    guard.remove(&(world_id, intent_hash));
                }
            });
            return Err(RuntimeError::Execution(format!(
                "failed to start adapter: {err:?}"
            )));
        }
        let inflight = Arc::clone(&self.inflight);
        let continuation_tx = self.continuation_tx.clone();
        let world_id_for_updates = world_id.clone();
        let route_id_for_updates = route_id.clone();
        tokio::spawn(async move {
            let mut terminal_seen = false;
            while let Some(update) = update_rx.recv().await {
                let input = match normalize_effect_update(
                    update,
                    intent_hash,
                    &route_id_for_updates,
                    normalize_context.as_ref(),
                ) {
                    EffectUpdate::Receipt(receipt) => {
                        terminal_seen = true;
                        WorldInput::Receipt(receipt)
                    }
                    EffectUpdate::StreamFrame(frame) => WorldInput::StreamFrame(frame),
                };
                let _ = continuation_tx
                    .send(EffectRuntimeEvent::WorldInput {
                        world_id: world_id_for_updates.clone(),
                        input,
                    })
                    .await;
            }
            if !terminal_seen {
                let _ = continuation_tx
                    .send(EffectRuntimeEvent::WorldInput {
                        world_id: world_id_for_updates.clone(),
                        input: WorldInput::Receipt(adapter_start_error_receipt(
                            intent_hash,
                            &route_id_for_updates,
                        )),
                    })
                    .await;
            }
            if let Ok(mut guard) = inflight.lock() {
                guard.remove(&(world_id_for_updates, intent_hash));
            }
        });
        Ok(true)
    }
}

impl<W> EffectRuntime<W>
where
    W: Clone + Eq + Hash + Send + 'static,
{
    pub fn from_loaded_manifest<S: Store + 'static>(
        store: Arc<S>,
        adapter_config: &EffectAdapterConfig,
        loaded: &LoadedManifest,
        strict_effect_bindings: bool,
        continuation_tx: mpsc::Sender<EffectRuntimeEvent<W>>,
    ) -> Result<Self, RuntimeError> {
        let shared = SharedEffectRuntime::new(store, adapter_config, continuation_tx);
        Self::from_loaded_manifest_with_shared(shared, loaded, strict_effect_bindings)
    }

    pub fn from_loaded_manifest_with_shared(
        shared: SharedEffectRuntime<W>,
        loaded: &LoadedManifest,
        strict_effect_bindings: bool,
    ) -> Result<Self, RuntimeError> {
        let effect_routes = collect_effect_routes(loaded);
        let route_diagnostics = preflight_external_effect_routes(
            loaded,
            &effect_routes,
            shared.adapter_registry.as_ref(),
            strict_effect_bindings,
        )?;
        Ok(Self {
            shared,
            effect_routes,
            route_diagnostics,
        })
    }

    pub fn from_effect_routes<S: Store + 'static>(
        store: Arc<S>,
        adapter_config: &EffectAdapterConfig,
        effect_routes: HashMap<String, String>,
        strict_effect_bindings: bool,
        continuation_tx: mpsc::Sender<EffectRuntimeEvent<W>>,
    ) -> Self {
        let shared = SharedEffectRuntime::new(store, adapter_config, continuation_tx);
        Self::from_effect_routes_with_shared(shared, effect_routes, strict_effect_bindings)
    }

    pub fn from_effect_routes_with_shared(
        shared: SharedEffectRuntime<W>,
        effect_routes: HashMap<String, String>,
        strict_effect_bindings: bool,
    ) -> Self {
        let host_provides = shared.host_route_mappings();
        let world_requires: BTreeMap<String, String> = effect_routes
            .iter()
            .map(|(kind, adapter_id)| (kind.clone(), adapter_id.clone()))
            .collect();
        Self {
            shared,
            effect_routes,
            route_diagnostics: EffectRouteDiagnostics {
                strict_effect_bindings,
                compatibility_fallback_enabled: !strict_effect_bindings,
                world_requires,
                host_provides,
                compatibility_fallback_kinds: Vec::new(),
            },
        }
    }

    pub fn route_diagnostics(&self) -> &EffectRouteDiagnostics {
        &self.route_diagnostics
    }

    pub fn classify_intent(&self, intent: &EffectIntent) -> EffectExecutionClass {
        classify_effect_kind(intent.kind.as_str())
    }

    pub fn resolve_effect_route_id(&self, effect_kind: &str) -> Result<String, RuntimeError> {
        if let Some(route_id) = self.effect_routes.get(effect_kind) {
            return Ok(route_id.clone());
        }
        if self.route_diagnostics.strict_effect_bindings {
            return Err(RuntimeError::Route(format!(
                "missing manifest.effect_bindings route for external kind '{effect_kind}'"
            )));
        }
        Ok(effect_kind.to_string())
    }

    pub fn ensure_started(&self, world_id: W, intent: EffectIntent) -> Result<bool, RuntimeError> {
        self.ensure_started_with_context(world_id, intent, None)
    }

    pub fn ensure_started_with_context(
        &self,
        world_id: W,
        intent: EffectIntent,
        start_context: Option<AdapterStartContext>,
    ) -> Result<bool, RuntimeError> {
        match self.classify_intent(&intent) {
            EffectExecutionClass::InlineInternal => {
                return Err(RuntimeError::ExecutionClass(format!(
                    "internal effect '{}' must run inline after append",
                    intent.kind.as_str()
                )));
            }
            EffectExecutionClass::OwnerLocalTimer => {
                return Err(RuntimeError::ExecutionClass(format!(
                    "timer effect '{}' is owner-local and must not use shared async effect runtime",
                    intent.kind.as_str()
                )));
            }
            EffectExecutionClass::ExternalAsync => {}
        }

        let route_id = self.resolve_effect_route_id(intent.kind.as_str())?;
        self.shared
            .ensure_started_routed(world_id, route_id, intent, start_context)
    }
}

fn normalize_effect_update(
    mut update: EffectUpdate,
    expected_intent_hash: [u8; 32],
    route_id: &str,
    context: Option<&AdapterStartContext>,
) -> EffectUpdate {
    match &mut update {
        EffectUpdate::Receipt(receipt) => {
            if receipt.intent_hash != expected_intent_hash {
                tracing::warn!(
                    "adapter route '{route_id}' returned receipt intent_hash {} but runtime expected {}; rewriting receipt to claimed intent",
                    hex::encode(receipt.intent_hash),
                    hex::encode(expected_intent_hash),
                );
                receipt.intent_hash = expected_intent_hash;
            }
        }
        EffectUpdate::StreamFrame(frame) => {
            if frame.intent_hash != expected_intent_hash {
                tracing::warn!(
                    "adapter route '{route_id}' returned stream frame intent_hash {} but runtime expected {}; rewriting frame to claimed intent",
                    hex::encode(frame.intent_hash),
                    hex::encode(expected_intent_hash),
                );
                frame.intent_hash = expected_intent_hash;
            }
            if let Some(context) = context {
                frame.origin_module_id = context.origin_module_id.clone();
                frame.origin_instance_key = context.origin_instance_key.clone();
                frame.effect_kind = context.effect_kind.clone();
                frame.emitted_at_seq = context.emitted_at_seq;
            }
        }
    }
    update
}

fn adapter_start_error_receipt(intent_hash: [u8; 32], route_id: &str) -> EffectReceipt {
    EffectReceipt {
        intent_hash,
        adapter_id: format!("runtime.{route_id}"),
        status: ReceiptStatus::Error,
        payload_cbor: Vec::new(),
        cost_cents: None,
        signature: Vec::new(),
    }
}

pub fn classify_effect_kind(kind: &str) -> EffectExecutionClass {
    if kind == aos_effects::EffectKind::TIMER_SET {
        return EffectExecutionClass::OwnerLocalTimer;
    }
    if kind.starts_with("workspace.")
        || kind.starts_with("introspect.")
        || kind.starts_with("governance.")
        || kind.starts_with("portal.")
    {
        return EffectExecutionClass::InlineInternal;
    }
    EffectExecutionClass::ExternalAsync
}

fn collect_effect_routes(loaded: &LoadedManifest) -> HashMap<String, String> {
    loaded
        .manifest
        .effect_bindings
        .iter()
        .map(|binding| {
            (
                binding.kind.as_str().to_string(),
                binding.adapter_id.clone(),
            )
        })
        .collect()
}

fn preflight_external_effect_routes(
    loaded: &LoadedManifest,
    effect_routes: &HashMap<String, String>,
    registry: &AdapterRegistry,
    strict_effect_bindings: bool,
) -> Result<EffectRouteDiagnostics, RuntimeError> {
    let mut required_kinds: BTreeSet<String> = BTreeSet::new();
    for effect in loaded.effects.values() {
        let kind = effect.kind.as_str();
        if matches!(
            classify_effect_kind(kind),
            EffectExecutionClass::ExternalAsync
        ) {
            required_kinds.insert(kind.to_string());
        }
    }

    let host_provides = registry.route_mappings();
    if required_kinds.is_empty() {
        return Ok(EffectRouteDiagnostics {
            strict_effect_bindings,
            compatibility_fallback_enabled: !strict_effect_bindings,
            world_requires: BTreeMap::new(),
            host_provides,
            compatibility_fallback_kinds: Vec::new(),
        });
    }

    let mut origins: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for module in loaded.modules.values() {
        let Some(workflow_abi) = module.abi.workflow.as_ref() else {
            continue;
        };
        for kind in &workflow_abi.effects_emitted {
            if matches!(
                classify_effect_kind(kind.as_str()),
                EffectExecutionClass::ExternalAsync
            ) {
                origins
                    .entry(kind.as_str().to_string())
                    .or_default()
                    .push(module.name.clone());
            }
        }
    }
    for modules in origins.values_mut() {
        modules.sort();
        modules.dedup();
    }

    let mut compatibility_fallback_kinds = Vec::new();
    let mut world_requires = BTreeMap::new();
    for kind in required_kinds {
        if let Some(adapter_id) = effect_routes.get(&kind) {
            if !registry.has_route(adapter_id) {
                return Err(RuntimeError::Route(format!(
                    "effect kind '{kind}' is bound to missing adapter route '{adapter_id}'"
                )));
            }
            world_requires.insert(kind, adapter_id.clone());
            continue;
        }

        if strict_effect_bindings {
            let emitted_by = origins.get(&kind).cloned().unwrap_or_default().join(", ");
            return Err(RuntimeError::Route(format!(
                "effect kind '{kind}' is emitted by [{emitted_by}] but has no manifest.effect_bindings entry"
            )));
        }

        if !registry.has_route(&kind) {
            return Err(RuntimeError::Route(format!(
                "effect kind '{kind}' has no manifest route and no direct adapter route"
            )));
        }

        compatibility_fallback_kinds.push(kind.clone());
        world_requires.insert(kind.clone(), kind);
    }

    Ok(EffectRouteDiagnostics {
        strict_effect_bindings,
        compatibility_fallback_enabled: !strict_effect_bindings,
        world_requires,
        host_provides,
        compatibility_fallback_kinds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::EffectKind;
    use aos_effect_adapters::config::EffectAdapterConfig;
    use aos_kernel::MemStore;
    use std::collections::HashMap;
    use tokio::time::{Duration, timeout};

    #[tokio::test(flavor = "current_thread")]
    async fn ensure_started_routes_external_effects_to_world_input_receipts() {
        let (tx, mut rx) = mpsc::channel(4);
        let runtime = EffectRuntime::from_effect_routes(
            Arc::new(MemStore::default()),
            &EffectAdapterConfig::default(),
            HashMap::new(),
            false,
            tx,
        );
        let intent = EffectIntent::from_raw_params(
            EffectKind::llm_generate(),
            serde_cbor::to_vec(&serde_json::json!({
                "provider": "stub",
                "model": "stub",
                "messages": [{"role": "user", "content": "hello"}]
            }))
            .expect("params"),
            [7; 32],
        )
        .expect("intent");

        assert!(
            runtime
                .ensure_started("world-1".to_string(), intent.clone())
                .expect("spawned")
        );

        let event = timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timely receipt")
            .expect("runtime event");
        match event {
            EffectRuntimeEvent::WorldInput { world_id, input } => {
                assert_eq!(world_id, "world-1");
                match input {
                    WorldInput::Receipt(receipt) => {
                        assert_eq!(receipt.intent_hash, intent.intent_hash);
                    }
                    other => panic!("unexpected runtime continuation: {other:?}"),
                }
            }
        }
    }

    #[test]
    fn classify_execution_classes_matches_architecture_split() {
        assert_eq!(
            classify_effect_kind("workspace.write_bytes"),
            EffectExecutionClass::InlineInternal
        );
        assert_eq!(
            classify_effect_kind(aos_effects::EffectKind::TIMER_SET),
            EffectExecutionClass::OwnerLocalTimer
        );
        assert_eq!(
            classify_effect_kind("http.request"),
            EffectExecutionClass::ExternalAsync
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shared_runtime_dedupes_same_world_intent_across_wrappers() {
        let (tx, mut rx) = mpsc::channel(4);
        let shared = SharedEffectRuntime::new(
            Arc::new(MemStore::default()),
            &EffectAdapterConfig::default(),
            tx,
        );
        let runtime_a =
            EffectRuntime::from_effect_routes_with_shared(shared.clone(), HashMap::new(), false);
        let runtime_b =
            EffectRuntime::from_effect_routes_with_shared(shared, HashMap::new(), false);
        let intent = EffectIntent::from_raw_params(
            EffectKind::llm_generate(),
            serde_cbor::to_vec(&serde_json::json!({
                "provider": "stub",
                "model": "stub",
                "messages": [{"role": "user", "content": "hello"}]
            }))
            .expect("params"),
            [9; 32],
        )
        .expect("intent");

        assert!(
            runtime_a
                .ensure_started("world-1".to_string(), intent.clone())
                .expect("spawned")
        );
        assert!(
            !runtime_b
                .ensure_started("world-1".to_string(), intent)
                .expect("deduped")
        );

        let _ = timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timely event")
            .expect("runtime event");
    }

    #[test]
    fn normalize_stream_frame_fills_runtime_origin_context() {
        let context = AdapterStartContext {
            origin_module_id: "com.acme/Workflow@1".to_string(),
            origin_instance_key: Some(vec![1, 2, 3]),
            effect_kind: EffectKind::HOST_EXEC.to_string(),
            emitted_at_seq: 42,
        };
        let update = EffectUpdate::StreamFrame(aos_effects::EffectStreamFrame {
            intent_hash: [1; 32],
            adapter_id: "host.exec.fabric".to_string(),
            origin_module_id: "placeholder".to_string(),
            origin_instance_key: None,
            effect_kind: "placeholder".to_string(),
            emitted_at_seq: 0,
            seq: 7,
            kind: "host.exec.progress".to_string(),
            payload_cbor: Vec::new(),
            payload_ref: None,
            signature: vec![0; 64],
        });

        let normalized =
            normalize_effect_update(update, [2; 32], "host.exec.fabric", Some(&context));
        let EffectUpdate::StreamFrame(frame) = normalized else {
            panic!("expected stream frame");
        };

        assert_eq!(frame.intent_hash, [2; 32]);
        assert_eq!(frame.origin_module_id, context.origin_module_id);
        assert_eq!(frame.origin_instance_key, context.origin_instance_key);
        assert_eq!(frame.effect_kind, context.effect_kind);
        assert_eq!(frame.emitted_at_seq, context.emitted_at_seq);
        assert_eq!(frame.seq, 7);
        assert_eq!(frame.kind, "host.exec.progress");
    }
}
