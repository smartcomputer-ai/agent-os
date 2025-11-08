use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use aos_air_exec::Value as ExprValue;
use aos_air_types::{Manifest, Name};
use aos_cbor::Hash as DigestHash;
use aos_store::Store;
use aos_wasm_abi::{ABI_VERSION, CallContext, DomainEvent, ReducerInput, ReducerOutput};
use serde_cbor;

use crate::capability::CapabilityResolver;
use crate::effects::EffectManager;
use crate::error::KernelError;
use crate::event::{KernelEvent, ReducerEvent};
use crate::manifest::{LoadedManifest, ManifestLoader};
use crate::plan::{PlanInstance, PlanRegistry};
use crate::policy::AllowAllPolicy;
use crate::receipts::{ReducerEffectContext, build_reducer_receipt_event};
use crate::reducer::ReducerRegistry;
use crate::scheduler::{Scheduler, Task};

const RECENT_RECEIPT_CACHE: usize = 512;

pub struct Kernel<S: Store> {
    manifest: Manifest,
    module_defs: HashMap<Name, aos_air_types::DefModule>,
    reducers: ReducerRegistry<S>,
    router: HashMap<String, Vec<Name>>,
    plan_registry: PlanRegistry,
    plan_instances: HashMap<u64, PlanInstance>,
    plan_triggers: HashMap<String, Vec<String>>,
    waiting_events: HashMap<String, Vec<u64>>,
    pending_receipts: HashMap<[u8; 32], u64>,
    pending_reducer_receipts: HashMap<[u8; 32], ReducerEffectContext>,
    recent_receipts: VecDeque<[u8; 32]>,
    recent_receipt_index: HashSet<[u8; 32]>,
    scheduler: Scheduler,
    effect_manager: EffectManager,
    reducer_state: HashMap<Name, Vec<u8>>,
}

pub struct KernelBuilder<S: Store> {
    store: Arc<S>,
}

impl<S: Store + 'static> KernelBuilder<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    pub fn from_manifest_path(
        self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<Kernel<S>, KernelError> {
        let loaded = ManifestLoader::load_from_path(&*self.store, path)?;
        Kernel::from_loaded_manifest(self.store, loaded)
    }
}

impl<S: Store + 'static> Kernel<S> {
    pub fn from_loaded_manifest(
        store: Arc<S>,
        loaded: LoadedManifest,
    ) -> Result<Self, KernelError> {
        let mut router = HashMap::new();
        if let Some(routing) = loaded.manifest.routing.as_ref() {
            for route in &routing.events {
                router
                    .entry(route.event.as_str().to_string())
                    .or_insert_with(Vec::new)
                    .push(route.reducer.clone());
            }
        }
        let mut plan_registry = PlanRegistry::default();
        for plan in loaded.plans.values() {
            plan_registry.register(plan.clone());
        }
        let mut plan_triggers = HashMap::new();
        for trigger in &loaded.manifest.triggers {
            plan_triggers
                .entry(trigger.event.as_str().to_string())
                .or_insert_with(Vec::new)
                .push(trigger.plan.clone());
        }
        let capability_resolver =
            CapabilityResolver::from_manifest(&loaded.manifest, &loaded.caps)?;
        ensure_plan_capabilities(&loaded.plans, &capability_resolver)?;
        ensure_module_capabilities(&loaded.manifest, &capability_resolver)?;

        Ok(Self {
            manifest: loaded.manifest,
            module_defs: loaded.modules,
            reducers: ReducerRegistry::new(store)?,
            router,
            plan_registry,
            plan_instances: HashMap::new(),
            plan_triggers,
            waiting_events: HashMap::new(),
            pending_receipts: HashMap::new(),
            pending_reducer_receipts: HashMap::new(),
            recent_receipts: VecDeque::new(),
            recent_receipt_index: HashSet::new(),
            scheduler: Scheduler::default(),
            effect_manager: EffectManager::new(capability_resolver, AllowAllPolicy),
            reducer_state: HashMap::new(),
        })
    }

    pub fn enqueue_event(&mut self, event: KernelEvent) {
        match event {
            KernelEvent::Reducer(ev) => self.scheduler.push_reducer(ev),
        }
    }

    pub fn submit_domain_event(&mut self, schema: impl Into<String>, value: Vec<u8>) {
        let event = DomainEvent::new(schema.into(), value);
        self.scheduler.push_reducer(ReducerEvent { event });
    }

    pub fn tick(&mut self) -> Result<(), KernelError> {
        if let Some(task) = self.scheduler.pop() {
            match task {
                Task::Reducer(event) => self.handle_reducer_event(event)?,
                Task::Plan(id) => self.handle_plan_task(id)?,
            }
        }
        Ok(())
    }

    fn handle_reducer_event(&mut self, event: ReducerEvent) -> Result<(), KernelError> {
        let reducers = self
            .router
            .get(&event.event.schema)
            .cloned()
            .unwrap_or_default();
        for reducer_name in reducers {
            let module_def = self
                .module_defs
                .get(&reducer_name)
                .ok_or_else(|| KernelError::ReducerNotFound(reducer_name.clone()))?;
            self.reducers.ensure_loaded(&reducer_name, module_def)?;

            let input = ReducerInput {
                version: ABI_VERSION,
                state: self.reducer_state.get(&reducer_name).cloned(),
                event: event.event.clone(),
                ctx: CallContext::new(false, None),
            };
            let output = self.reducers.invoke(&reducer_name, &input)?;
            self.handle_reducer_output(reducer_name.clone(), output)?;
        }
        self.start_plans_for_event(&event.event)?;
        Ok(())
    }

    fn handle_reducer_output(
        &mut self,
        reducer_name: String,
        output: ReducerOutput,
    ) -> Result<(), KernelError> {
        match output.state {
            Some(state) => {
                self.reducer_state.insert(reducer_name.clone(), state);
            }
            None => {
                self.reducer_state.remove(&reducer_name);
            }
        }
        for event in output.domain_events {
            self.deliver_event_to_waiting_plans(&event)?;
            self.scheduler.push_reducer(ReducerEvent { event });
        }
        for effect in &output.effects {
            let slot = effect.cap_slot.clone().unwrap_or_else(|| "default".into());
            let cap_name = self
                .manifest
                .module_bindings
                .get(&reducer_name)
                .and_then(|binding| binding.slots.get(&slot))
                .ok_or_else(|| KernelError::CapabilityBindingMissing {
                    reducer: reducer_name.clone(),
                    slot: slot.clone(),
                })?
                .clone();
            let hash =
                self.effect_manager
                    .enqueue_reducer_effect(&reducer_name, &cap_name, effect)?;
            self.pending_reducer_receipts.insert(
                hash,
                ReducerEffectContext::new(
                    reducer_name.clone(),
                    effect.kind.clone(),
                    effect.params_cbor.clone(),
                ),
            );
        }
        Ok(())
    }

    pub fn drain_effects(&mut self) -> Vec<aos_effects::EffectIntent> {
        self.effect_manager.drain()
    }

    pub fn reducer_state(&self, reducer: &str) -> Option<&Vec<u8>> {
        self.reducer_state.get(reducer)
    }

    fn start_plans_for_event(&mut self, event: &DomainEvent) -> Result<(), KernelError> {
        if let Some(plan_names) = self.plan_triggers.get(&event.schema) {
            for plan_name in plan_names {
                if let Some(plan_def) = self.plan_registry.get(plan_name) {
                    let input: ExprValue = serde_cbor::from_slice(&event.value).map_err(|err| {
                        KernelError::Manifest(format!(
                            "failed to decode plan input for {}: {err}",
                            plan_name
                        ))
                    })?;
                    let instance_id = self.scheduler.alloc_plan_id();
                    let instance = PlanInstance::new(instance_id, plan_def.clone(), input);
                    self.plan_instances.insert(instance_id, instance);
                    self.scheduler.push_plan(instance_id);
                }
            }
        }
        Ok(())
    }

    fn handle_plan_task(&mut self, id: u64) -> Result<(), KernelError> {
        let waiting_schema = self
            .plan_instances
            .get(&id)
            .and_then(|inst| inst.waiting_event_schema())
            .map(|s| s.to_string());
        if let Some(schema) = waiting_schema {
            self.remove_plan_from_waiting_events_for_schema(id, &schema);
        }
        if let Some(instance) = self.plan_instances.get_mut(&id) {
            let outcome = instance.tick(&mut self.effect_manager)?;
            for event in &outcome.raised_events {
                self.deliver_event_to_waiting_plans(event)?;
                self.scheduler.push_reducer(ReducerEvent {
                    event: event.clone(),
                });
            }
            if let Some(hash) = outcome.waiting_receipt {
                self.pending_receipts.insert(hash, id);
            }
            if let Some(schema) = outcome.waiting_event.clone() {
                self.waiting_events.entry(schema).or_default().push(id);
            }
            if outcome.completed {
                self.plan_instances.remove(&id);
            } else if outcome.waiting_receipt.is_none() && outcome.waiting_event.is_none() {
                self.scheduler.push_plan(id);
            }
        }
        Ok(())
    }

    pub fn handle_receipt(
        &mut self,
        receipt: aos_effects::EffectReceipt,
    ) -> Result<(), KernelError> {
        if let Some(plan_id) = self.pending_receipts.remove(&receipt.intent_hash) {
            if let Some(instance) = self.plan_instances.get_mut(&plan_id) {
                if instance.deliver_receipt(receipt.intent_hash, &receipt.payload_cbor)? {
                    self.scheduler.push_plan(plan_id);
                }
                self.remember_receipt(receipt.intent_hash);
                return Ok(());
            } else {
                log::warn!(
                    "receipt {} arrived for completed plan {}",
                    format_intent_hash(&receipt.intent_hash),
                    plan_id
                );
                self.remember_receipt(receipt.intent_hash);
                return Ok(());
            }
        }

        if let Some(context) = self.pending_reducer_receipts.remove(&receipt.intent_hash) {
            let event = build_reducer_receipt_event(&context, &receipt)?;
            self.scheduler.push_reducer(ReducerEvent { event });
            self.remember_receipt(receipt.intent_hash);
            return Ok(());
        }

        if self.recent_receipt_index.contains(&receipt.intent_hash) {
            log::warn!(
                "late receipt {} ignored (already applied)",
                format_intent_hash(&receipt.intent_hash)
            );
            return Ok(());
        }

        Err(KernelError::UnknownReceipt(format_intent_hash(
            &receipt.intent_hash,
        )))
    }

    fn deliver_event_to_waiting_plans(&mut self, event: &DomainEvent) -> Result<(), KernelError> {
        if let Some(mut plan_ids) = self.waiting_events.remove(&event.schema) {
            let mut still_waiting = Vec::new();
            for id in plan_ids.drain(..) {
                if let Some(instance) = self.plan_instances.get_mut(&id) {
                    if instance.deliver_event(event)? {
                        self.scheduler.push_plan(id);
                    } else {
                        still_waiting.push(id);
                    }
                }
            }
            if !still_waiting.is_empty() {
                self.waiting_events
                    .insert(event.schema.clone(), still_waiting);
            }
        }
        Ok(())
    }

    fn remove_plan_from_waiting_events_for_schema(&mut self, plan_id: u64, schema: &str) {
        if let Some(ids) = self.waiting_events.get_mut(schema) {
            if let Some(pos) = ids.iter().position(|id| *id == plan_id) {
                ids.swap_remove(pos);
            }
            if ids.is_empty() {
                self.waiting_events.remove(schema);
            }
        }
    }

    fn remember_receipt(&mut self, hash: [u8; 32]) {
        if self.recent_receipt_index.contains(&hash) {
            return;
        }
        if self.recent_receipts.len() >= RECENT_RECEIPT_CACHE {
            if let Some(old) = self.recent_receipts.pop_front() {
                self.recent_receipt_index.remove(&old);
            }
        }
        self.recent_receipts.push_back(hash);
        self.recent_receipt_index.insert(hash);
    }
}

fn format_intent_hash(hash: &[u8; 32]) -> String {
    DigestHash::from_bytes(hash)
        .map(|h| h.to_hex())
        .unwrap_or_else(|_| format!("{:?}", hash))
}

fn ensure_plan_capabilities(
    plans: &HashMap<Name, aos_air_types::DefPlan>,
    resolver: &CapabilityResolver,
) -> Result<(), KernelError> {
    for plan in plans.values() {
        for cap in &plan.required_caps {
            if !resolver.has_grant(cap) {
                return Err(KernelError::PlanCapabilityMissing {
                    plan: plan.name.clone(),
                    cap: cap.clone(),
                });
            }
        }
    }
    Ok(())
}

fn ensure_module_capabilities(
    manifest: &Manifest,
    resolver: &CapabilityResolver,
) -> Result<(), KernelError> {
    for (module, binding) in &manifest.module_bindings {
        for (_slot, cap) in &binding.slots {
            if !resolver.has_grant(cap) {
                return Err(KernelError::ModuleCapabilityMissing {
                    module: module.clone(),
                    cap: cap.clone(),
                });
            }
        }
    }
    Ok(())
}
