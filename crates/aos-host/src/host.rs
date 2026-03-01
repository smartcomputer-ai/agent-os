use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use aos_air_types::AirNode;
use aos_cbor::Hash;
use aos_effects::builtins::TimerSetReceipt;
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, EffectStreamFrame, ReceiptStatus};
use aos_kernel::{
    DefListing, Kernel, KernelBuilder, KernelConfig, KernelHeights, LoadedManifest, ManifestLoader,
    TailIntent, TailScan, cell_index::CellMeta,
};
use aos_store::{FsStore, Store};

use crate::adapters::blob_get::BlobGetAdapter;
use crate::adapters::blob_put::BlobPutAdapter;
use crate::adapters::host::make_host_adapter_set;
use crate::adapters::registry::AdapterRegistry;
use crate::adapters::registry::AdapterRegistryConfig;
#[cfg(not(feature = "adapter-http"))]
use crate::adapters::stub::StubHttpAdapter;
use crate::adapters::stub::{
    StubLlmAdapter, StubTimerAdapter, StubVaultPutAdapter, StubVaultRotateAdapter,
};
use crate::adapters::timer::TimerScheduler;
use crate::config::HostConfig;
use crate::error::HostError;
use crate::manifest_loader;
use aos_kernel::StateReader;
use serde::Serialize;

#[derive(Debug, Clone)]
pub enum ExternalEvent {
    DomainEvent {
        schema: String,
        value: Vec<u8>,
        key: Option<Vec<u8>>,
    },
    Receipt(EffectReceipt),
    StreamFrame(EffectStreamFrame),
}

/// Execution mode for `run_cycle`.
///
/// - `Batch`: All effects (including timers) go through the adapter registry.
///   Timers are handled by StubTimerAdapter and fire immediately (good for tests).
/// - `Daemon`: Timer intents are scheduled on the provided `TimerScheduler` instead
///   of being executed immediately. The daemon fires them later via `fire_due_timers`.
pub enum RunMode<'a> {
    /// Batch mode: all effects dispatched via adapter registry.
    Batch,
    /// Daemon mode: timer.set intents scheduled on scheduler, others via adapter registry.
    Daemon { scheduler: &'a mut TimerScheduler },
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

#[derive(Debug, Clone, Serialize)]
pub struct EffectRouteDiagnostics {
    pub strict_effect_bindings: bool,
    pub compatibility_fallback_enabled: bool,
    pub world_requires: BTreeMap<String, String>,
    pub host_provides: BTreeMap<String, String>,
    pub compatibility_fallback_kinds: Vec<String>,
}

pub struct WorldHost<S: Store + 'static> {
    kernel: Kernel<S>,
    adapter_registry: AdapterRegistry,
    effect_routes: HashMap<String, String>,
    route_diagnostics: EffectRouteDiagnostics,
    config: HostConfig,
    store: Arc<S>,
}

impl<S: Store + 'static> WorldHost<S> {
    pub fn open(
        store: Arc<S>,
        manifest_path: &Path,
        host_config: HostConfig,
        kernel_config: KernelConfig,
    ) -> Result<Self, HostError> {
        let root = manifest_path.parent().unwrap_or(Path::new("."));
        let loaded = ManifestLoader::load_from_path(store.as_ref(), manifest_path)?;
        let mut kernel_config = kernel_config;
        if kernel_config.secret_resolver.is_none() {
            if let Some(resolver) = crate::util::env_secret_resolver_from_manifest(&loaded) {
                kernel_config.secret_resolver = Some(resolver);
            }
        }
        let adapter_registry = default_registry(store.clone(), &host_config);
        let effect_routes = collect_effect_routes(&loaded);
        let route_diagnostics = preflight_external_effect_routes(
            &loaded,
            &effect_routes,
            &adapter_registry,
            host_config.strict_effect_bindings,
        )?;

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

        let mut kernel = builder.from_loaded_manifest(loaded)?;

        // Rehydrate dispatch queue: queued_effects snapshot + tail intents lacking receipts.
        let heights = kernel.heights();
        let tail = kernel.tail_scan_after(heights.snapshot.unwrap_or(0))?;
        let receipts_seen = receipts_set(&tail);

        let mut to_dispatch: Vec<EffectIntent> = kernel
            .queued_effects_snapshot()
            .into_iter()
            .map(|snap| snap.into_intent())
            .collect();
        let mut seen_intents: HashSet<[u8; 32]> =
            to_dispatch.iter().map(|i| i.intent_hash).collect();

        for TailIntent { record, .. } in tail.intents.iter() {
            if receipts_seen.contains(&record.intent_hash) {
                continue;
            }
            if seen_intents.contains(&record.intent_hash) {
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
                seen_intents.insert(intent.intent_hash);
                to_dispatch.push(intent);
            }
        }

        if !to_dispatch.is_empty() {
            kernel.restore_effect_queue(to_dispatch);
        }

        Ok(Self {
            kernel,
            adapter_registry,
            effect_routes,
            route_diagnostics,
            config: host_config,
            store,
        })
    }
}

impl WorldHost<FsStore> {
    /// Open a world from a directory containing AIR JSON assets.
    ///
    /// This method loads the manifest from `air/` subdirectories using the
    /// manifest_loader, which parses AIR JSON files and constructs a LoadedManifest.
    /// Use this for worlds defined via JSON assets rather than a pre-built CBOR manifest.
    pub fn open_dir(
        world_root: &Path,
        host_config: HostConfig,
        kernel_config: KernelConfig,
    ) -> Result<Self, HostError> {
        let store =
            Arc::new(FsStore::open(world_root).map_err(|e| HostError::Store(e.to_string()))?);

        let loaded = manifest_loader::load_from_assets(store.clone(), world_root)
            .map_err(|e| HostError::Manifest(e.to_string()))?
            .ok_or_else(|| {
                HostError::Manifest(format!(
                    "no manifest found in '{}' (expected air/ directory with AIR JSON files)",
                    world_root.display()
                ))
            })?;

        Self::from_loaded_manifest(store, loaded, world_root, host_config, kernel_config)
    }

    /// Open a world from the CAS manifest hash stored in the journal.
    pub fn open_from_manifest_hash(
        world_root: &Path,
        manifest_hash: Hash,
        host_config: HostConfig,
        kernel_config: KernelConfig,
    ) -> Result<Self, HostError> {
        let store =
            Arc::new(FsStore::open(world_root).map_err(|e| HostError::Store(e.to_string()))?);
        let loaded = ManifestLoader::load_from_hash(store.as_ref(), manifest_hash)
            .map_err(|e| HostError::Manifest(e.to_string()))?;
        Self::from_loaded_manifest(store, loaded, world_root, host_config, kernel_config)
    }

    /// Create a WorldHost from a pre-loaded manifest.
    pub fn from_loaded_manifest(
        store: Arc<FsStore>,
        loaded: LoadedManifest,
        world_root: &Path,
        host_config: HostConfig,
        kernel_config: KernelConfig,
    ) -> Result<Self, HostError> {
        let mut kernel_config = kernel_config;
        if kernel_config.secret_resolver.is_none() {
            if let Some(resolver) = crate::util::env_secret_resolver_from_manifest(&loaded) {
                kernel_config.secret_resolver = Some(resolver);
            }
        }
        let adapter_registry = default_registry(store.clone(), &host_config);
        let effect_routes = collect_effect_routes(&loaded);
        let route_diagnostics = preflight_external_effect_routes(
            &loaded,
            &effect_routes,
            &adapter_registry,
            host_config.strict_effect_bindings,
        )?;

        let mut builder = KernelBuilder::new(store.clone()).with_fs_journal(world_root)?;

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

        let mut kernel = builder.from_loaded_manifest(loaded)?;

        // Rehydrate dispatch queue: queued_effects snapshot + tail intents lacking receipts.
        let heights = kernel.heights();
        let tail = kernel.tail_scan_after(heights.snapshot.unwrap_or(0))?;
        let receipts_seen = receipts_set(&tail);

        let mut to_dispatch: Vec<EffectIntent> = kernel
            .queued_effects_snapshot()
            .into_iter()
            .map(|snap| snap.into_intent())
            .collect();
        let mut seen_intents: HashSet<[u8; 32]> =
            to_dispatch.iter().map(|i| i.intent_hash).collect();

        for TailIntent { record, .. } in tail.intents.iter() {
            if receipts_seen.contains(&record.intent_hash) {
                continue;
            }
            if seen_intents.contains(&record.intent_hash) {
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
                seen_intents.insert(intent.intent_hash);
                to_dispatch.push(intent);
            }
        }

        if !to_dispatch.is_empty() {
            kernel.restore_effect_queue(to_dispatch);
        }

        Ok(Self {
            kernel,
            adapter_registry,
            effect_routes,
            route_diagnostics,
            config: host_config,
            store,
        })
    }
}

impl<S: Store + 'static> WorldHost<S> {
    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    pub fn effect_route_diagnostics(&self) -> &EffectRouteDiagnostics {
        &self.route_diagnostics
    }

    /// Put a blob into the backing store and return its hex hash string.
    pub fn put_blob(&self, bytes: &[u8]) -> Result<String, HostError> {
        let hash = self
            .store
            .put_blob(bytes)
            .map_err(|e| HostError::Store(e.to_string()))?;
        Ok(hash.to_hex())
    }

    pub fn adapter_registry_mut(&mut self) -> &mut AdapterRegistry {
        &mut self.adapter_registry
    }

    pub fn adapter_registry(&self) -> &AdapterRegistry {
        &self.adapter_registry
    }

    /// Access underlying store (read-only).
    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn enqueue_external(&mut self, evt: ExternalEvent) -> Result<(), HostError> {
        match evt {
            ExternalEvent::DomainEvent { schema, value, key } => {
                if let Some(key) = key {
                    self.kernel
                        .submit_domain_event_with_key(schema, value, key)?;
                } else {
                    self.kernel.submit_domain_event(schema, value)?;
                }
            }
            ExternalEvent::Receipt(receipt) => {
                self.kernel.handle_receipt(receipt)?;
            }
            ExternalEvent::StreamFrame(frame) => {
                self.kernel.handle_stream_frame(frame)?;
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

    pub fn state(&self, workflow: &str, key: Option<&[u8]>) -> Option<Vec<u8>> {
        self.kernel
            .workflow_state_bytes(workflow, key)
            .unwrap_or(None)
    }

    /// Query workflow state with consistency metadata.
    pub fn query_state(
        &self,
        workflow: &str,
        key: Option<&[u8]>,
        consistency: aos_kernel::Consistency,
    ) -> Option<aos_kernel::StateRead<Option<Vec<u8>>>> {
        self.kernel
            .get_workflow_state(workflow, key, consistency)
            .ok()
    }

    /// List all cells for a keyed workflow. Returns empty if workflow is not keyed or has no cells.
    pub fn list_cells(&self, workflow: &str) -> Result<Vec<CellMeta>, HostError> {
        self.kernel.list_cells(workflow).map_err(HostError::from)
    }

    pub fn list_defs(
        &self,
        kinds: Option<&[String]>,
        prefix: Option<&str>,
    ) -> Result<Vec<DefListing>, HostError> {
        Ok(self.kernel.list_defs(kinds, prefix))
    }

    pub fn get_def(&self, name: &str) -> Result<AirNode, HostError> {
        self.kernel
            .get_def(name)
            .ok_or_else(|| HostError::Manifest(format!("definition '{name}' not found")))
    }

    pub fn snapshot(&mut self) -> Result<(), HostError> {
        Ok(self.kernel.create_snapshot()?)
    }

    pub async fn run_cycle(&mut self, mode: RunMode<'_>) -> Result<CycleOutcome, HostError> {
        let initial = self.drain()?;
        let intents = self.kernel.drain_effects()?;
        let effects_dispatched = intents.len();

        enum Slot {
            Internal(aos_effects::EffectReceipt),
            External, // position preserved via iterator order
            Timer,
        }

        let mut slots = Vec::with_capacity(intents.len());
        let mut external_intents: Vec<(EffectIntent, String)> = Vec::new();

        match mode {
            RunMode::Batch => {
                for intent in intents {
                    if let Some(receipt) = self.kernel.handle_internal_intent(&intent)? {
                        slots.push(Slot::Internal(receipt));
                    } else {
                        let route_id = self.resolve_effect_route_id(intent.kind.as_str());
                        slots.push(Slot::External);
                        external_intents.push((intent, route_id));
                    }
                }
            }
            RunMode::Daemon { scheduler } => {
                for intent in intents {
                    if let Some(receipt) = self.kernel.handle_internal_intent(&intent)? {
                        slots.push(Slot::Internal(receipt));
                        continue;
                    }
                    if intent.kind.as_str() == EffectKind::TIMER_SET {
                        scheduler.schedule(&intent)?;
                        slots.push(Slot::Timer);
                    } else {
                        let route_id = self.resolve_effect_route_id(intent.kind.as_str());
                        slots.push(Slot::External);
                        external_intents.push((intent, route_id));
                    }
                }
            }
        }

        let external_receipts = self
            .adapter_registry
            .execute_batch_routed(external_intents)
            .await;
        let mut external_iter = external_receipts.into_iter();

        let mut receipts = Vec::new();
        for slot in slots {
            match slot {
                Slot::Internal(r) => receipts.push(r),
                Slot::External => receipts.push(
                    external_iter
                        .next()
                        .expect("external receipt for each external slot"),
                ),
                Slot::Timer => {}
            }
        }

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

    /// Returns true if the kernel has pending effects that need processing.
    pub fn has_pending_effects(&self) -> bool {
        self.kernel.has_pending_effects()
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
    pub fn from_kernel(kernel: Kernel<S>, store: Arc<S>, config: HostConfig) -> Self {
        Self::from_kernel_with_effect_routes(kernel, store, config, HashMap::new())
    }

    /// Create a WorldHost from an existing kernel with explicit effect route map.
    pub fn from_kernel_with_effect_routes(
        kernel: Kernel<S>,
        store: Arc<S>,
        config: HostConfig,
        effect_routes: HashMap<String, String>,
    ) -> Self {
        let adapter_registry = default_registry(store.clone(), &config);
        let host_provides = adapter_registry.route_mappings();
        let world_requires: BTreeMap<String, String> = effect_routes
            .iter()
            .map(|(kind, adapter_id)| (kind.clone(), adapter_id.clone()))
            .collect();
        let route_diagnostics = EffectRouteDiagnostics {
            strict_effect_bindings: config.strict_effect_bindings,
            compatibility_fallback_enabled: !config.strict_effect_bindings,
            world_requires,
            host_provides,
            compatibility_fallback_kinds: Vec::new(),
        };
        Self {
            kernel,
            adapter_registry,
            effect_routes,
            route_diagnostics,
            config,
            store,
        }
    }

    pub(crate) fn resolve_effect_route_id(&self, effect_kind: &str) -> String {
        if let Some(route_id) = self.effect_routes.get(effect_kind) {
            return route_id.clone();
        }
        if self.config.strict_effect_bindings && !is_internal_effect_kind(effect_kind) {
            log::error!(
                "strict effect binding mode: missing manifest.effect_bindings route for external kind '{}'",
                effect_kind
            );
            return "adapter.missing.binding".to_string();
        }
        effect_kind.to_string()
    }

    /// Fire all due timers by building receipts and calling `handle_receipt`.
    ///
    /// This is the correct way to fire timers in daemon mode. The kernel will:
    /// 1. Remove context from `pending_workflow_receipts`
    /// 2. Record receipt in journal
    /// 3. Build a `sys/TimerFired@1` receipt event via `build_workflow_receipt_event()`
    /// 4. Route/wrap at dispatch and push workflow event to scheduler
    ///
    /// Uses kernel logical time for scheduling and receipt timestamps.
    ///
    /// Returns the number of timers fired.
    pub fn fire_due_timers(&mut self, scheduler: &mut TimerScheduler) -> Result<usize, HostError> {
        let now_ns = self.kernel.logical_time_now_ns();
        let due = scheduler.pop_due(now_ns);
        let count = due.len();

        for entry in due {
            // Build the receipt with actual delivery time
            let timer_receipt = TimerSetReceipt {
                delivered_at_ns: now_ns,
                key: entry.key,
            };

            // Serialize receipt payload
            let payload_cbor = serde_cbor::to_vec(&timer_receipt).map_err(|e| {
                HostError::Timer(format!("failed to encode TimerSetReceipt: {}", e))
            })?;

            // Build EffectReceipt and feed through handle_receipt
            let receipt = EffectReceipt {
                intent_hash: entry.intent_hash,
                adapter_id: "timer.set".into(),
                status: ReceiptStatus::Ok,
                payload_cbor,
                cost_cents: Some(0),
                signature: vec![0; 64], // TODO: real signing
            };

            // This triggers the full receipt flow in kernel
            self.kernel.handle_receipt(receipt)?;
        }

        Ok(count)
    }
}

/// Get current wall-clock time in nanoseconds (Unix epoch).
pub fn now_wallclock_ns() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
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

fn is_internal_effect_kind(kind: &str) -> bool {
    kind.starts_with("workspace.")
        || kind.starts_with("introspect.")
        || kind.starts_with("governance.")
}

fn preflight_external_effect_routes(
    loaded: &LoadedManifest,
    effect_routes: &HashMap<String, String>,
    registry: &AdapterRegistry,
    strict_effect_bindings: bool,
) -> Result<EffectRouteDiagnostics, HostError> {
    let mut required_kinds: BTreeSet<String> = BTreeSet::new();
    for effect in loaded.effects.values() {
        let kind = effect.kind.as_str();
        if !is_internal_effect_kind(kind) {
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
            origins
                .entry(kind.as_str().to_string())
                .or_default()
                .push(module.name.clone());
        }
    }
    for modules in origins.values_mut() {
        modules.sort();
        modules.dedup();
    }

    let mut world_requires: BTreeMap<String, String> = BTreeMap::new();
    let mut missing_bindings: Vec<String> = Vec::new();
    let mut compatibility_fallback_kinds: Vec<String> = Vec::new();
    let mut missing: Vec<(String, String)> = Vec::new();
    for kind in required_kinds {
        let route_id = if let Some(bound_route) = effect_routes.get(kind.as_str()) {
            bound_route.clone()
        } else if strict_effect_bindings {
            world_requires.insert(kind.clone(), "(missing-binding)".to_string());
            missing_bindings.push(kind.clone());
            continue;
        } else {
            let fallback = kind.clone();
            compatibility_fallback_kinds.push(kind.clone());
            fallback
        };

        world_requires.insert(kind.clone(), route_id.clone());
        if !registry.has_route(&route_id) {
            missing.push((kind, route_id));
        }
    }

    compatibility_fallback_kinds.sort();
    compatibility_fallback_kinds.dedup();
    if !compatibility_fallback_kinds.is_empty() {
        log::debug!(
            "using compatibility fallback routes for external effects without manifest.effect_bindings: {}",
            compatibility_fallback_kinds.join(", ")
        );
    }

    if !missing_bindings.is_empty() {
        let missing_kinds = missing_bindings.join(", ");
        return Err(HostError::Manifest(format!(
            "strict effect binding mode requires explicit manifest.effect_bindings for external kinds: {missing_kinds}; world_requires={}; host_provides={}",
            format_route_map(&world_requires),
            format_route_map(&host_provides),
        )));
    }

    if !missing.is_empty() {
        let mut details = Vec::new();
        for (kind, route_id) in &missing {
            let origin_modules = origins
                .get(kind.as_str())
                .map(|mods| mods.join(", "))
                .unwrap_or_else(|| "unknown".to_string());
            details.push(format!(
                "kind='{kind}' route='{route_id}' origins=[{origin_modules}]"
            ));
        }
        return Err(HostError::Manifest(format!(
            "missing adapter routes for external effects: {}; world_requires={}; host_provides={}",
            details.join("; "),
            format_route_map(&world_requires),
            format_route_map(&host_provides),
        )));
    }

    let diagnostics = EffectRouteDiagnostics {
        strict_effect_bindings,
        compatibility_fallback_enabled: !strict_effect_bindings,
        world_requires,
        host_provides,
        compatibility_fallback_kinds,
    };
    log::debug!(
        "effect route diagnostics: strict_effect_bindings={} world_requires={} host_provides={} compatibility_fallback_kinds={}",
        diagnostics.strict_effect_bindings,
        format_route_map(&diagnostics.world_requires),
        format_route_map(&diagnostics.host_provides),
        diagnostics.compatibility_fallback_kinds.join(", "),
    );
    Ok(diagnostics)
}

fn default_registry<S: Store + 'static>(store: Arc<S>, config: &HostConfig) -> AdapterRegistry {
    let mut registry = AdapterRegistry::new(AdapterRegistryConfig {
        effect_timeout: config.effect_timeout,
    });
    registry.register(Box::new(StubTimerAdapter));
    registry.register(Box::new(StubVaultPutAdapter));
    registry.register(Box::new(StubVaultRotateAdapter));
    registry.register(Box::new(BlobPutAdapter::new(store.clone())));
    registry.register(Box::new(BlobGetAdapter::new(store.clone())));
    let host_adapters = make_host_adapter_set(store.clone());
    registry.register(Box::new(host_adapters.session_open));
    registry.register(Box::new(host_adapters.exec));
    registry.register(Box::new(host_adapters.session_signal));
    registry.register(Box::new(host_adapters.fs_read_file));
    registry.register(Box::new(host_adapters.fs_write_file));
    registry.register(Box::new(host_adapters.fs_edit_file));
    registry.register(Box::new(host_adapters.fs_apply_patch));
    registry.register(Box::new(host_adapters.fs_grep));
    registry.register(Box::new(host_adapters.fs_glob));
    registry.register(Box::new(host_adapters.fs_stat));
    registry.register(Box::new(host_adapters.fs_exists));
    registry.register(Box::new(host_adapters.fs_list_dir));

    #[cfg(feature = "adapter-http")]
    {
        registry.register(Box::new(crate::adapters::http::HttpAdapter::new(
            store.clone(),
            config.http.clone(),
        )));
    }
    #[cfg(not(feature = "adapter-http"))]
    {
        registry.register(Box::new(StubHttpAdapter));
    }

    #[cfg(feature = "adapter-llm")]
    {
        if let Some(llm_cfg) = &config.llm {
            registry.register(Box::new(crate::adapters::llm::LlmAdapter::new(
                store,
                llm_cfg.clone(),
            )));
        } else {
            registry.register(Box::new(StubLlmAdapter));
        }
    }
    #[cfg(not(feature = "adapter-llm"))]
    {
        registry.register(Box::new(StubLlmAdapter));
    }

    // Register host profile route ids (`adapter_id`) to concrete adapter kinds.
    for (adapter_id, provider) in &config.adapter_routes {
        if !registry.register_route(adapter_id.as_str(), provider.adapter_kind.as_str()) {
            log::warn!(
                "host profile route '{}' targets unknown adapter kind '{}'",
                adapter_id,
                provider.adapter_kind
            );
        }
    }

    registry
}

fn format_route_map(map: &BTreeMap<String, String>) -> String {
    map.iter()
        .map(|(kind, adapter_id)| format!("{kind}->{adapter_id}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn receipts_set(tail: &TailScan) -> HashSet<[u8; 32]> {
    tail.receipts.iter().map(|r| r.record.intent_hash).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AdapterProviderSpec;
    use aos_air_types::catalog::EffectCatalog;
    use aos_air_types::{CURRENT_AIR_VERSION, EffectBinding, Manifest, NamedRef, builtins};
    use aos_kernel::LoadedManifest;
    use aos_store::FsStore;
    use aos_store::MemStore;
    use serde_cbor::to_vec;
    use serde_json::json;
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_minimal_manifest(path: &std::path::Path) {
        // Minimal manifest: no workflows/plans; just air_version and empty lists.
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

    fn loaded_manifest_with_effect_routes(
        effect_kinds: &[&str],
        effect_bindings: &[(&str, &str)],
    ) -> LoadedManifest {
        let mut effects = HashMap::new();
        let mut effect_refs = Vec::new();

        for kind in effect_kinds {
            let builtin = builtins::builtin_effects()
                .iter()
                .find(|entry| entry.effect.kind.as_str() == *kind)
                .unwrap_or_else(|| panic!("builtin effect kind not found: {kind}"));
            effects.insert(builtin.effect.name.clone(), builtin.effect.clone());
            effect_refs.push(NamedRef {
                name: builtin.effect.name.clone(),
                hash: builtin.hash_ref.clone(),
            });
        }

        let manifest = Manifest {
            air_version: CURRENT_AIR_VERSION.to_string(),
            schemas: Vec::new(),
            modules: Vec::new(),
            effects: effect_refs,
            effect_bindings: effect_bindings
                .iter()
                .map(|(kind, adapter_id)| EffectBinding {
                    kind: aos_air_types::EffectKind::new(*kind),
                    adapter_id: (*adapter_id).to_string(),
                })
                .collect(),
            caps: Vec::new(),
            policies: Vec::new(),
            secrets: Vec::new(),
            defaults: None,
            module_bindings: Default::default(),
            routing: None,
        };

        LoadedManifest {
            manifest,
            secrets: Vec::new(),
            modules: HashMap::new(),
            effects: effects.clone(),
            caps: HashMap::new(),
            policies: HashMap::new(),
            schemas: HashMap::new(),
            effect_catalog: EffectCatalog::from_defs(effects.values().cloned()),
        }
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
    async fn enqueue_domain_event_surfaces_validation_error() {
        let tmp = TempDir::new().unwrap();
        let manifest_path = tmp.path().join("manifest.cbor");
        write_minimal_manifest(&manifest_path);

        let store = Arc::new(MemStore::new());
        let host_config = HostConfig::default();
        let kernel_config = KernelConfig::default();
        let mut host = WorldHost::open(store, &manifest_path, host_config, kernel_config).unwrap();

        // Event schema is not declared in this manifest; enqueue should return an error.
        let err = host
            .enqueue_external(ExternalEvent::DomainEvent {
                schema: "demo/Event@1".into(),
                value: to_vec(&json!({"x": 1})).unwrap(),
                key: None,
            })
            .expect_err("missing event schema should fail");
        assert!(matches!(err, HostError::Kernel(_)));

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

        // No workflows, but we can still apply a receipt (should be ignored gracefully)
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

    #[test]
    fn startup_preflight_fails_when_bound_route_missing() {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(FsStore::open(tmp.path()).expect("fs store"));
        let loaded = loaded_manifest_with_effect_routes(
            &[EffectKind::HTTP_REQUEST],
            &[(EffectKind::HTTP_REQUEST, "http.missing")],
        );

        let err = match WorldHost::from_loaded_manifest(
            store,
            loaded,
            tmp.path(),
            HostConfig::default(),
            KernelConfig::default(),
        ) {
            Ok(_) => panic!("missing adapter route should fail startup"),
            Err(err) => err,
        };

        let HostError::Manifest(message) = err else {
            panic!("expected manifest error from preflight");
        };
        assert!(message.contains("http.request"));
        assert!(message.contains("http.missing"));
        assert!(message.contains("world_requires="));
        assert!(message.contains("host_provides="));
    }

    #[test]
    fn startup_preflight_accepts_bound_route_when_available() {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(FsStore::open(tmp.path()).expect("fs store"));
        let loaded = loaded_manifest_with_effect_routes(
            &[EffectKind::HTTP_REQUEST],
            &[(EffectKind::HTTP_REQUEST, "http.default")],
        );

        WorldHost::from_loaded_manifest(
            store,
            loaded,
            tmp.path(),
            HostConfig::default(),
            KernelConfig::default(),
        )
        .expect("default bound route should pass preflight");
    }

    #[test]
    fn startup_preflight_ignores_internal_effect_kinds() {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(FsStore::open(tmp.path()).expect("fs store"));
        let loaded = loaded_manifest_with_effect_routes(&[EffectKind::INTROSPECT_MANIFEST], &[]);

        WorldHost::from_loaded_manifest(
            store,
            loaded,
            tmp.path(),
            HostConfig::default(),
            KernelConfig::default(),
        )
        .expect("internal effects should not require external adapter routes");
    }

    #[test]
    fn startup_preflight_uses_kind_route_when_binding_absent() {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(FsStore::open(tmp.path()).expect("fs store"));
        let loaded = loaded_manifest_with_effect_routes(&[EffectKind::HTTP_REQUEST], &[]);

        WorldHost::from_loaded_manifest(
            store,
            loaded,
            tmp.path(),
            HostConfig::default(),
            KernelConfig::default(),
        )
        .expect("missing binding should fallback to legacy kind route");
    }

    #[test]
    fn startup_preflight_accepts_custom_host_profile_route() {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(FsStore::open(tmp.path()).expect("fs store"));
        let loaded = loaded_manifest_with_effect_routes(
            &[EffectKind::HTTP_REQUEST],
            &[(EffectKind::HTTP_REQUEST, "http.custom")],
        );
        let mut config = HostConfig::default();
        config.adapter_routes.insert(
            "http.custom".into(),
            AdapterProviderSpec {
                adapter_kind: EffectKind::HTTP_REQUEST.to_string(),
            },
        );

        WorldHost::from_loaded_manifest(store, loaded, tmp.path(), config, KernelConfig::default())
            .expect("custom host profile route should pass preflight");
    }

    #[test]
    fn startup_preflight_strict_mode_requires_explicit_bindings() {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(FsStore::open(tmp.path()).expect("fs store"));
        let loaded = loaded_manifest_with_effect_routes(&[EffectKind::HTTP_REQUEST], &[]);
        let mut config = HostConfig::default();
        config.strict_effect_bindings = true;

        let err = match WorldHost::from_loaded_manifest(
            store,
            loaded,
            tmp.path(),
            config,
            KernelConfig::default(),
        ) {
            Ok(_) => panic!("strict mode should fail without manifest bindings"),
            Err(err) => err,
        };

        let HostError::Manifest(message) = err else {
            panic!("expected manifest error from strict preflight");
        };
        assert!(message.contains("strict effect binding mode"));
        assert!(message.contains(EffectKind::HTTP_REQUEST));
    }

    #[test]
    fn startup_preflight_strict_mode_accepts_explicit_bindings() {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(FsStore::open(tmp.path()).expect("fs store"));
        let loaded = loaded_manifest_with_effect_routes(
            &[EffectKind::HTTP_REQUEST],
            &[(EffectKind::HTTP_REQUEST, "http.default")],
        );
        let mut config = HostConfig::default();
        config.strict_effect_bindings = true;

        let host = WorldHost::from_loaded_manifest(
            store,
            loaded,
            tmp.path(),
            config,
            KernelConfig::default(),
        )
        .expect("strict mode should pass with explicit route bindings");
        assert!(host.effect_route_diagnostics().strict_effect_bindings);
        assert!(
            !host
                .effect_route_diagnostics()
                .compatibility_fallback_enabled
        );
    }

    #[test]
    fn host_route_diagnostics_capture_compatibility_fallback_kinds() {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(FsStore::open(tmp.path()).expect("fs store"));
        let loaded = loaded_manifest_with_effect_routes(
            &[EffectKind::HTTP_REQUEST, EffectKind::BLOB_GET],
            &[(EffectKind::BLOB_GET, "blob.get.default")],
        );

        let host = WorldHost::from_loaded_manifest(
            store,
            loaded,
            tmp.path(),
            HostConfig::default(),
            KernelConfig::default(),
        )
        .expect("preflight should pass with compatibility fallback enabled");

        let diag = host.effect_route_diagnostics();
        assert_eq!(
            diag.world_requires.get(EffectKind::HTTP_REQUEST),
            Some(&EffectKind::HTTP_REQUEST.to_string())
        );
        assert_eq!(
            diag.world_requires.get(EffectKind::BLOB_GET),
            Some(&"blob.get.default".to_string())
        );
        assert_eq!(
            diag.compatibility_fallback_kinds,
            vec![EffectKind::HTTP_REQUEST.to_string()]
        );
    }
}
