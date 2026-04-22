use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use aos_air_types::OpKind;
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
    pub strict_op_routes: bool,
    pub compatibility_fallback_enabled: bool,
    pub world_requires: BTreeMap<String, String>,
    pub host_provides: BTreeMap<String, String>,
    pub compatibility_fallback_ops: Vec<String>,
}

impl From<&EffectRouteDiagnostics> for aos_kernel::TraceRouteDiagnostics {
    fn from(value: &EffectRouteDiagnostics) -> Self {
        Self {
            strict_op_routes: value.strict_op_routes,
            compatibility_fallback_enabled: value.compatibility_fallback_enabled,
            world_requires: value.world_requires.clone(),
            host_provides: value.host_provides.clone(),
            compatibility_fallback_ops: value.compatibility_fallback_ops.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct EffectExecutionRoute {
    class: EffectExecutionClass,
    route_id: Option<String>,
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
    effect_routes: HashMap<String, EffectExecutionRoute>,
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
        strict_op_routes: bool,
        continuation_tx: mpsc::Sender<EffectRuntimeEvent<W>>,
    ) -> Result<Self, RuntimeError> {
        let shared = SharedEffectRuntime::new(store, adapter_config, continuation_tx);
        Self::from_loaded_manifest_with_shared(shared, loaded, strict_op_routes)
    }

    pub fn from_loaded_manifest_with_shared(
        shared: SharedEffectRuntime<W>,
        loaded: &LoadedManifest,
        strict_op_routes: bool,
    ) -> Result<Self, RuntimeError> {
        let effect_routes = collect_effect_routes(loaded, shared.adapter_registry.as_ref())?;
        let route_diagnostics = preflight_external_effect_routes(
            loaded,
            &effect_routes,
            shared.adapter_registry.as_ref(),
            strict_op_routes,
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
        strict_op_routes: bool,
        continuation_tx: mpsc::Sender<EffectRuntimeEvent<W>>,
    ) -> Self {
        let shared = SharedEffectRuntime::new(store, adapter_config, continuation_tx);
        Self::from_effect_routes_with_shared(shared, effect_routes, strict_op_routes)
    }

    pub fn from_effect_routes_with_shared(
        shared: SharedEffectRuntime<W>,
        effect_routes: HashMap<String, String>,
        strict_op_routes: bool,
    ) -> Self {
        let host_provides = shared.host_route_mappings();
        let world_requires: BTreeMap<String, String> = effect_routes
            .iter()
            .map(|(op, route_id)| (op.clone(), route_id.clone()))
            .collect();
        let effect_routes = effect_routes
            .into_iter()
            .map(|(op, route_id)| {
                (
                    op,
                    EffectExecutionRoute {
                        class: EffectExecutionClass::ExternalAsync,
                        route_id: Some(route_id),
                    },
                )
            })
            .collect();
        Self {
            shared,
            effect_routes,
            route_diagnostics: EffectRouteDiagnostics {
                strict_op_routes: strict_op_routes,
                compatibility_fallback_enabled: !strict_op_routes,
                world_requires,
                host_provides,
                compatibility_fallback_ops: Vec::new(),
            },
        }
    }

    pub fn route_diagnostics(&self) -> &EffectRouteDiagnostics {
        &self.route_diagnostics
    }

    pub fn classify_intent(&self, intent: &EffectIntent) -> EffectExecutionClass {
        self.effect_routes
            .get(intent.effect_op.as_str())
            .map(|route| route.class)
            .unwrap_or_else(|| classify_effect_op_identity(intent))
    }

    pub fn resolve_effect_route_id(&self, intent: &EffectIntent) -> Result<String, RuntimeError> {
        let effect_op = intent.effect_op.as_str();
        let route = self.effect_routes.get(effect_op).ok_or_else(|| {
            RuntimeError::Route(format!(
                "missing execution route for external effect op '{effect_op}'"
            ))
        })?;
        route.route_id.clone().ok_or_else(|| {
            RuntimeError::ExecutionClass(format!(
                "effect op '{effect_op}' is not an external async route"
            ))
        })
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
                    "internal effect op '{}' must run inline after append",
                    intent.effect_op.as_str()
                )));
            }
            EffectExecutionClass::OwnerLocalTimer => {
                return Err(RuntimeError::ExecutionClass(format!(
                    "timer effect op '{}' is owner-local and must not use shared async effect runtime",
                    intent.effect_op.as_str()
                )));
            }
            EffectExecutionClass::ExternalAsync => {}
        }

        let route_id = self.resolve_effect_route_id(&intent)?;
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
                frame.origin_workflow_op_hash = context.origin_workflow_op_hash.clone();
                frame.origin_instance_key = context.origin_instance_key.clone();
                frame.effect_op = context.effect_op.clone();
                frame.effect_op_hash = context.effect_op_hash.clone();
                frame.executor_module = context.executor_module.clone();
                frame.executor_module_hash = context.executor_module_hash.clone();
                frame.executor_entrypoint = context.executor_entrypoint.clone();
                frame.emitted_at_seq = context.emitted_at_seq;
            }
        }
    }
    update
}

fn adapter_start_error_receipt(intent_hash: [u8; 32], route_id: &str) -> EffectReceipt {
    let _ = route_id;
    EffectReceipt {
        intent_hash,
        status: ReceiptStatus::Error,
        payload_cbor: Vec::new(),
        cost_cents: None,
        signature: Vec::new(),
    }
}

pub fn classify_effect_op_identity(intent: &EffectIntent) -> EffectExecutionClass {
    match intent.effect_op.as_str() {
        aos_effects::effect_ops::TIMER_SET => EffectExecutionClass::OwnerLocalTimer,
        _ => {
            if intent.executor_module.as_deref() == Some("sys/builtin_effects@1")
                && intent.executor_entrypoint.is_some()
            {
                EffectExecutionClass::InlineInternal
            } else {
                EffectExecutionClass::ExternalAsync
            }
        }
    }
}

fn collect_effect_routes(
    loaded: &LoadedManifest,
    registry: &AdapterRegistry,
) -> Result<HashMap<String, EffectExecutionRoute>, RuntimeError> {
    let mut routes = HashMap::new();
    for op in loaded
        .ops
        .values()
        .filter(|op| op.op_kind == OpKind::Effect)
    {
        let class = if op.name == "sys/timer.set@1" {
            EffectExecutionClass::OwnerLocalTimer
        } else if registry.has_route(&op.implementation.entrypoint) {
            EffectExecutionClass::ExternalAsync
        } else if op.implementation.module == "sys/builtin_effects@1" {
            EffectExecutionClass::InlineInternal
        } else {
            return Err(RuntimeError::Route(format!(
                "effect op '{}' has executor entrypoint '{}' but no adapter route",
                op.name, op.implementation.entrypoint
            )));
        };
        let route_id = if matches!(class, EffectExecutionClass::ExternalAsync) {
            Some(op.implementation.entrypoint.clone())
        } else {
            None
        };
        routes.insert(op.name.clone(), EffectExecutionRoute { class, route_id });
    }
    Ok(routes)
}

fn preflight_external_effect_routes(
    loaded: &LoadedManifest,
    effect_routes: &HashMap<String, EffectExecutionRoute>,
    registry: &AdapterRegistry,
    strict_op_routes: bool,
) -> Result<EffectRouteDiagnostics, RuntimeError> {
    let mut required_ops: BTreeSet<String> = BTreeSet::new();
    for op in loaded
        .ops
        .values()
        .filter(|op| op.op_kind == OpKind::Effect)
    {
        if matches!(
            effect_routes.get(op.name.as_str()).map(|route| route.class),
            Some(EffectExecutionClass::ExternalAsync)
        ) {
            required_ops.insert(op.name.clone());
        }
    }

    let host_provides = registry.route_mappings();
    if required_ops.is_empty() {
        return Ok(EffectRouteDiagnostics {
            strict_op_routes: strict_op_routes,
            compatibility_fallback_enabled: false,
            world_requires: BTreeMap::new(),
            host_provides,
            compatibility_fallback_ops: Vec::new(),
        });
    }

    let mut origins: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for op in loaded
        .ops
        .values()
        .filter(|op| op.op_kind == OpKind::Workflow)
    {
        let Some(workflow) = op.workflow.as_ref() else {
            continue;
        };
        for declared in &workflow.effects_emitted {
            if matches!(
                effect_routes
                    .get(declared.as_str())
                    .map(|route| route.class),
                Some(EffectExecutionClass::ExternalAsync)
            ) {
                origins
                    .entry(declared.to_string())
                    .or_default()
                    .push(op.name.clone());
            }
        }
    }
    for modules in origins.values_mut() {
        modules.sort();
        modules.dedup();
    }

    let mut world_requires = BTreeMap::new();
    for op_name in required_ops {
        if let Some(route) = effect_routes.get(&op_name) {
            let Some(route_id) = route.route_id.as_ref() else {
                continue;
            };
            if !registry.has_route(route_id.as_str()) {
                return Err(RuntimeError::Route(format!(
                    "effect op '{op_name}' is bound to missing route '{route_id}'"
                )));
            }
            world_requires.insert(op_name, route_id.clone());
            continue;
        }

        if strict_op_routes {
            let emitted_by = origins
                .get(&op_name)
                .cloned()
                .unwrap_or_default()
                .join(", ");
            return Err(RuntimeError::Route(format!(
                "effect op '{op_name}' is emitted by [{emitted_by}] but has no execution route"
            )));
        }

        return Err(RuntimeError::Route(format!(
            "effect op '{op_name}' has no execution route"
        )));
    }

    Ok(EffectRouteDiagnostics {
        strict_op_routes: strict_op_routes,
        compatibility_fallback_enabled: false,
        world_requires,
        host_provides,
        compatibility_fallback_ops: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_effect_adapters::config::EffectAdapterConfig;
    use aos_effects::effect_ops;
    use aos_kernel::MemStore;
    use std::collections::HashMap;
    use tokio::time::{Duration, timeout};

    #[tokio::test(flavor = "current_thread")]
    async fn ensure_started_routes_external_effects_to_world_input_receipts() {
        let (tx, mut rx) = mpsc::channel(4);
        let routes =
            HashMap::from([("sys/llm.generate@1".to_string(), "llm.generate".to_string())]);
        let runtime = EffectRuntime::from_effect_routes(
            Arc::new(MemStore::default()),
            &EffectAdapterConfig::default(),
            routes,
            false,
            tx,
        );
        let mut intent = EffectIntent::from_raw_params(
            effect_ops::LLM_GENERATE,
            serde_cbor::to_vec(&serde_json::json!({
                "provider": "stub",
                "model": "stub",
                "messages": [{"role": "user", "content": "hello"}]
            }))
            .expect("params"),
            [7; 32],
        )
        .expect("intent");
        intent.effect_op = "sys/llm.generate@1".into();

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
        let mut workspace =
            EffectIntent::from_raw_params("workspace.write_bytes", Vec::new(), [0; 32])
                .expect("intent");
        workspace.effect_op = "sys/workspace.write_bytes@1".into();
        workspace.executor_module = Some("sys/builtin_effects@1".into());
        workspace.executor_entrypoint = Some("workspace.write_bytes".into());
        assert_eq!(
            classify_effect_op_identity(&workspace),
            EffectExecutionClass::InlineInternal
        );
        let mut timer = EffectIntent::from_raw_params(effect_ops::TIMER_SET, Vec::new(), [0; 32])
            .expect("intent");
        timer.effect_op = "sys/timer.set@1".into();
        assert_eq!(
            classify_effect_op_identity(&timer),
            EffectExecutionClass::OwnerLocalTimer
        );
        let mut http = EffectIntent::from_raw_params(effect_ops::HTTP_REQUEST, Vec::new(), [0; 32])
            .expect("intent");
        http.effect_op = "sys/http.request@1".into();
        http.executor_module = Some("sys/builtin_effects@1".into());
        http.executor_entrypoint = Some("http.request".into());
        assert_eq!(
            classify_effect_op_identity(&http),
            EffectExecutionClass::InlineInternal
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
        let routes =
            HashMap::from([("sys/llm.generate@1".to_string(), "llm.generate".to_string())]);
        let runtime_a =
            EffectRuntime::from_effect_routes_with_shared(shared.clone(), routes.clone(), false);
        let runtime_b = EffectRuntime::from_effect_routes_with_shared(shared, routes, false);
        let mut intent = EffectIntent::from_raw_params(
            effect_ops::LLM_GENERATE,
            serde_cbor::to_vec(&serde_json::json!({
                "provider": "stub",
                "model": "stub",
                "messages": [{"role": "user", "content": "hello"}]
            }))
            .expect("params"),
            [9; 32],
        )
        .expect("intent");
        intent.effect_op = "sys/llm.generate@1".into();

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
            origin_workflow_op_hash: Some("workflow-hash".to_string()),
            origin_instance_key: Some(vec![1, 2, 3]),
            effect_op: "sys/host.exec@1".to_string(),
            effect_op_hash: Some("effect-hash".to_string()),
            executor_module: Some("sys/Host@1".to_string()),
            executor_module_hash: Some("module-hash".to_string()),
            executor_entrypoint: Some(effect_ops::HOST_EXEC.to_string()),
            emitted_at_seq: 42,
        };
        let update = EffectUpdate::StreamFrame(aos_effects::EffectStreamFrame {
            intent_hash: [1; 32],
            origin_module_id: "placeholder".to_string(),
            origin_workflow_op_hash: None,
            origin_instance_key: None,
            effect_op: "placeholder".to_string(),
            effect_op_hash: None,
            executor_module: None,
            executor_module_hash: None,
            executor_entrypoint: None,
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
        assert_eq!(
            frame.origin_workflow_op_hash,
            context.origin_workflow_op_hash
        );
        assert_eq!(frame.origin_instance_key, context.origin_instance_key);
        assert_eq!(frame.effect_op, context.effect_op);
        assert_eq!(frame.effect_op_hash, context.effect_op_hash);
        assert_eq!(frame.executor_module, context.executor_module);
        assert_eq!(frame.executor_module_hash, context.executor_module_hash);
        assert_eq!(frame.executor_entrypoint, context.executor_entrypoint);
        assert_eq!(frame.emitted_at_seq, context.emitted_at_seq);
        assert_eq!(frame.seq, 7);
        assert_eq!(frame.kind, "host.exec.progress");
    }
}
