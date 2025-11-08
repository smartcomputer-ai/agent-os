use std::collections::HashMap;
use std::sync::Arc;

use aos_air_exec::Value as ExprValue;
use aos_air_types::{Manifest, Name};
use aos_store::Store;
use aos_wasm_abi::{ABI_VERSION, CallContext, DomainEvent, ReducerInput, ReducerOutput};
use serde_cbor;

use crate::effects::EffectManager;
use crate::error::KernelError;
use crate::event::{KernelEvent, ReducerEvent};
use crate::manifest::{LoadedManifest, ManifestLoader};
use crate::plan::{PlanInstance, PlanRegistry};
use crate::reducer::ReducerRegistry;
use crate::scheduler::{Scheduler, Task};

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
    pub(crate) fn from_loaded_manifest(
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
            scheduler: Scheduler::default(),
            effect_manager: EffectManager::new(),
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
        if !output.effects.is_empty() {
            self.effect_manager
                .enqueue_reducer_effects(&output.effects)?;
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
            }
        }
        Ok(())
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use aos_air_exec::Value as ExprValue;
    use aos_air_types::{
        DefModule, DefPlan, EffectKind, Expr, ExprConst, ExprRecord, ExprRef, HashRef, Manifest,
        ModuleAbi, ModuleKind, NamedRef, PlanBind, PlanBindEffect, PlanEdge, PlanStep,
        PlanStepAssign, PlanStepAwaitEvent, PlanStepAwaitReceipt, PlanStepEmitEffect,
        PlanStepEnd, PlanStepKind, PlanStepRaiseEvent, Routing, RoutingEvent, SchemaRef, Trigger,
    };
    use aos_store::MemStore;
    use aos_wasm_abi::{ReducerEffect, ReducerOutput};
    use indexmap::IndexMap;
    use serde_cbor;
    use wat::parse_str;

    const START_SCHEMA: &str = "com.acme/Start@1";

    fn zero_hash() -> HashRef {
        HashRef::new("sha256:0000000000000000000000000000000000000000000000000000000000000000")
            .unwrap()
    }

    fn schema(name: &str) -> SchemaRef {
        SchemaRef::new(name).unwrap()
    }

    fn text_expr(value: &str) -> Expr {
        Expr::Const(ExprConst::Text {
            text: value.to_string(),
        })
    }

    fn var_expr(name: &str) -> Expr {
        Expr::Ref(ExprRef {
            reference: format!("@var:{name}"),
        })
    }

    fn plan_input_expr(field: &str) -> Expr {
        Expr::Ref(ExprRef {
            reference: format!("@plan.input.{field}"),
        })
    }

    fn plan_input_record(fields: Vec<(&str, ExprValue)>) -> ExprValue {
        ExprValue::Record(IndexMap::from_iter(fields.into_iter().map(|(k, v)| (
            k.to_string(),
            v,
        ))))
    }

    fn build_loaded_manifest(
        plans: Vec<DefPlan>,
        triggers: Vec<Trigger>,
        modules: Vec<DefModule>,
        routing_events: Vec<RoutingEvent>,
    ) -> LoadedManifest {
        let plan_refs: Vec<NamedRef> = plans
            .iter()
            .map(|plan| NamedRef {
                name: plan.name.clone(),
                hash: zero_hash(),
            })
            .collect();
        let module_refs: Vec<NamedRef> = modules
            .iter()
            .map(|module| NamedRef {
                name: module.name.clone(),
                hash: module.wasm_hash.clone(),
            })
            .collect();
        let routing = if routing_events.is_empty() {
            None
        } else {
            Some(Routing {
                events: routing_events,
                inboxes: vec![],
            })
        };
        let manifest = Manifest {
            schemas: vec![],
            modules: module_refs,
            plans: plan_refs,
            caps: vec![],
            policies: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing,
            triggers,
        };
        let modules_map = modules
            .into_iter()
            .map(|module| (module.name.clone(), module))
            .collect();
        let plans_map = plans
            .into_iter()
            .map(|plan| (plan.name.clone(), plan))
            .collect();
        LoadedManifest {
            manifest,
            modules: modules_map,
            plans: plans_map,
        }
    }

    fn start_trigger(plan: &str) -> Trigger {
        Trigger {
            event: schema(START_SCHEMA),
            plan: plan.to_string(),
            correlate_by: None,
        }
    }

    #[test]
    fn kernel_runs_reducer_and_queues_effects() {
        let store = Arc::new(MemStore::new());
        let output = ReducerOutput {
            state: Some(vec![0xAA]),
            domain_events: vec![],
            effects: vec![ReducerEffect::new("timer.set", vec![0x01])],
            ann: None,
        };
        let output_bytes = output.encode().unwrap();
        let wasm_bytes = parse_str(&stub_reducer(&output_bytes));
        let wasm_bytes = wasm_bytes.expect("wat compile");
        let wasm_hash = store.put_blob(&wasm_bytes).unwrap();
        let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).unwrap();

        let module_def = DefModule {
            name: "com.acme/Reducer@1".into(),
            module_kind: ModuleKind::Reducer,
            wasm_hash: wasm_hash_ref,
            key_schema: None,
            abi: ModuleAbi { reducer: None },
        };

        let plan_def = DefPlan {
            name: "com.acme/Plan@1".into(),
            input: SchemaRef::new("com.acme/PlanIn@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![
                PlanStep {
                    id: "emit".into(),
                    kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                        kind: EffectKind::HttpRequest,
                        params: Expr::Const(ExprConst::Text {
                            text: "body".into(),
                        }),
                        cap: "cap_http".into(),
                        bind: PlanBindEffect {
                            effect_id_as: "req".into(),
                        },
                    }),
                },
                PlanStep {
                    id: "end".into(),
                    kind: PlanStepKind::End(PlanStepEnd { result: None }),
                },
            ],
            edges: vec![],
            required_caps: vec!["cap_http".into()],
            allowed_effects: vec![EffectKind::HttpRequest],
            invariants: vec![],
        };

        let manifest = Manifest {
            schemas: vec![],
            modules: vec![NamedRef {
                name: module_def.name.clone(),
                hash: module_def.wasm_hash.clone(),
            }],
            plans: vec![NamedRef {
                name: plan_def.name.clone(),
                hash: HashRef::new(
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                )
                .unwrap(),
            }],
            caps: vec![],
            policies: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing: Some(aos_air_types::Routing {
                events: vec![aos_air_types::RoutingEvent {
                    event: SchemaRef::new("com.acme/Start@1").unwrap(),
                    reducer: module_def.name.clone(),
                    key_field: None,
                }],
                inboxes: vec![],
            }),
            triggers: vec![aos_air_types::Trigger {
                event: SchemaRef::new("com.acme/Start@1").unwrap(),
                plan: plan_def.name.clone(),
                correlate_by: None,
            }],
        };

        let loaded = LoadedManifest {
            manifest,
            modules: HashMap::from([(module_def.name.clone(), module_def)]),
            plans: HashMap::from([(plan_def.name.clone(), plan_def)]),
        };

        let mut kernel = Kernel::from_loaded_manifest(store, loaded).unwrap();
        let plan_input = ExprValue::Record(IndexMap::from([(
            "id".into(),
            ExprValue::Text("123".into()),
        )]));
        let event_value = serde_cbor::to_vec(&plan_input).unwrap();
        kernel.submit_domain_event("com.acme/Start@1", event_value);
        kernel.tick().unwrap();
        kernel.tick().unwrap();

        let effects = kernel.drain_effects();
        assert_eq!(effects.len(), 2);
        assert_eq!(
            kernel.reducer_state("com.acme/Reducer@1"),
            Some(&vec![0xAA])
        );
    }

    fn stub_reducer(output_bytes: &[u8]) -> String {
        let data_literal = output_bytes
            .iter()
            .map(|b| format!("\\{:02x}", b))
            .collect::<String>();
        let len = output_bytes.len();
        format!(
            r#"(module
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const {len}))
  (data (i32.const 0) "{data}")
  (func (export "alloc") (param i32) (result i32)
    (local $old i32)
    global.get $heap
    local.tee $old
    local.get 0
    i32.add
    global.set $heap
    local.get $old)
  (func (export "step") (param i32 i32) (result i32 i32)
    (i32.const 0)
    (i32.const {len}))
)"#,
            len = len,
            data = data_literal
        )
    }

    #[test]
    fn plan_receipt_routing_resumes_waiting_instance() {
        let store = Arc::new(MemStore::new());
        let plan = DefPlan {
            name: "com.acme/Plan@1".into(),
            input: schema("com.acme/PlanIn@1"),
            output: None,
            locals: IndexMap::new(),
            steps: vec![
                PlanStep {
                    id: "emit".into(),
                    kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                        kind: EffectKind::HttpRequest,
                        params: text_expr("params"),
                        cap: "cap_http".into(),
                        bind: PlanBindEffect {
                            effect_id_as: "req".into(),
                        },
                    }),
                },
                PlanStep {
                    id: "await".into(),
                    kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                        for_expr: var_expr("req"),
                        bind: PlanBind { var: "resp".into() },
                    }),
                },
                PlanStep {
                    id: "assign".into(),
                    kind: PlanStepKind::Assign(PlanStepAssign {
                        expr: var_expr("resp"),
                        bind: PlanBind {
                            var: "copied".into(),
                        },
                    }),
                },
                PlanStep {
                    id: "wait_evt".into(),
                    kind: PlanStepKind::AwaitEvent(PlanStepAwaitEvent {
                        event: schema("com.acme/Next@1"),
                        where_clause: None,
                        bind: PlanBind { var: "evt".into() },
                    }),
                },
            ],
            edges: vec![],
            required_caps: vec!["cap_http".into()],
            allowed_effects: vec![EffectKind::HttpRequest],
            invariants: vec![],
        };

        let loaded = build_loaded_manifest(vec![plan.clone()], vec![start_trigger(&plan.name)], vec![], vec![]);
        let mut kernel = Kernel::from_loaded_manifest(store, loaded).unwrap();
        let plan_input = plan_input_record(vec![("id", ExprValue::Text("123".into()))]);
        let event_value = serde_cbor::to_vec(&plan_input).unwrap();
        kernel.submit_domain_event(START_SCHEMA, event_value);
        kernel.tick().unwrap();
        kernel.tick().unwrap();

        let mut effects = kernel.drain_effects();
        assert_eq!(effects.len(), 1);
        let effect = effects.remove(0);
        assert_eq!(kernel.pending_receipts.len(), 1);
        let (&receipt_hash, &plan_id) = kernel.pending_receipts.iter().next().unwrap();
        assert_eq!(receipt_hash, effect.intent_hash);

        let receipt_payload = serde_cbor::to_vec(&ExprValue::Text("done".into())).unwrap();
        let receipt = aos_effects::EffectReceipt {
            intent_hash: effect.intent_hash,
            adapter_id: "adapter.http".into(),
            status: aos_effects::ReceiptStatus::Ok,
            payload_cbor: receipt_payload,
            cost_cents: None,
            signature: vec![],
        };
        kernel.handle_receipt(receipt).unwrap();
        assert!(kernel.pending_receipts.is_empty());

        kernel.tick().unwrap();
        let plan_instance = kernel.plan_instances.get(&plan_id).unwrap();
        assert_eq!(
            plan_instance.env.vars.get("resp"),
            Some(&ExprValue::Text("done".into()))
        );
        assert_eq!(
            plan_instance.env.vars.get("copied"),
            Some(&ExprValue::Text("done".into()))
        );
        assert_eq!(
            kernel
                .waiting_events
                .get("com.acme/Next@1")
                .map(|ids| ids.contains(&plan_id)),
            Some(true)
        );

        let resume_event = DomainEvent::new(
            "com.acme/Next@1",
            serde_cbor::to_vec(&ExprValue::Int(1)).unwrap(),
        );
        kernel
            .deliver_event_to_waiting_plans(&resume_event)
            .unwrap();
        kernel.tick().unwrap();
        assert!(kernel.plan_instances.is_empty());
    }

    #[test]
    fn plan_event_wakeup_only_resumes_matching_schema() {
        let store = Arc::new(MemStore::new());
        let plan_ready = DefPlan {
            name: "com.acme/WaitReady@1".into(),
            input: schema("com.acme/PlanIn@1"),
            output: None,
            locals: IndexMap::new(),
            steps: vec![
                PlanStep {
                    id: "wait".into(),
                    kind: PlanStepKind::AwaitEvent(PlanStepAwaitEvent {
                        event: schema("com.acme/Ready@1"),
                        where_clause: None,
                        bind: PlanBind { var: "evt".into() },
                    }),
                },
                PlanStep {
                    id: "end".into(),
                    kind: PlanStepKind::End(PlanStepEnd { result: None }),
                },
            ],
            edges: vec![],
            required_caps: vec![],
            allowed_effects: vec![],
            invariants: vec![],
        };
        let plan_other = DefPlan {
            name: "com.acme/WaitOther@1".into(),
            input: schema("com.acme/PlanIn@1"),
            output: None,
            locals: IndexMap::new(),
            steps: vec![
                PlanStep {
                    id: "wait".into(),
                    kind: PlanStepKind::AwaitEvent(PlanStepAwaitEvent {
                        event: schema("com.acme/Other@1"),
                        where_clause: None,
                        bind: PlanBind { var: "evt".into() },
                    }),
                },
                PlanStep {
                    id: "end".into(),
                    kind: PlanStepKind::End(PlanStepEnd { result: None }),
                },
            ],
            edges: vec![],
            required_caps: vec![],
            allowed_effects: vec![],
            invariants: vec![],
        };

        let loaded = build_loaded_manifest(
            vec![plan_ready.clone(), plan_other.clone()],
            vec![start_trigger(&plan_ready.name), start_trigger(&plan_other.name)],
            vec![],
            vec![],
        );
        let mut kernel = Kernel::from_loaded_manifest(store, loaded).unwrap();
        let event_value = serde_cbor::to_vec(&plan_input_record(vec![])).unwrap();
        kernel.submit_domain_event(START_SCHEMA, event_value);
        kernel.tick().unwrap();
        kernel.tick().unwrap();
        kernel.tick().unwrap();

        let ready_waiters = kernel.waiting_events.get("com.acme/Ready@1").unwrap();
        let other_waiters = kernel.waiting_events.get("com.acme/Other@1").unwrap();
        assert_eq!(ready_waiters.len(), 1);
        assert_eq!(other_waiters.len(), 1);
        let ready_plan_id = ready_waiters[0];
        let other_plan_id = other_waiters[0];

        let ready_event = DomainEvent::new(
            "com.acme/Ready@1",
            serde_cbor::to_vec(&ExprValue::Nat(7)).unwrap(),
        );
        kernel
            .deliver_event_to_waiting_plans(&ready_event)
            .unwrap();
        kernel.tick().unwrap();

        assert!(!kernel.plan_instances.contains_key(&ready_plan_id));
        assert!(kernel.plan_instances.contains_key(&other_plan_id));
        assert!(kernel
            .waiting_events
            .get("com.acme/Other@1")
            .unwrap()
            .contains(&other_plan_id));
        assert!(!kernel
            .waiting_events
            .contains_key("com.acme/Ready@1"));
    }

    #[test]
    fn guarded_branching_controls_effects_and_completion() {
        let plan = DefPlan {
            name: "com.acme/Guarded@1".into(),
            input: schema("com.acme/PlanIn@1"),
            output: None,
            locals: IndexMap::new(),
            steps: vec![
                PlanStep {
                    id: "assign".into(),
                    kind: PlanStepKind::Assign(PlanStepAssign {
                        expr: plan_input_expr("flag"),
                        bind: PlanBind { var: "flag".into() },
                    }),
                },
                PlanStep {
                    id: "emit".into(),
                    kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                        kind: EffectKind::HttpRequest,
                        params: text_expr("do it"),
                        cap: "cap_http".into(),
                        bind: PlanBindEffect {
                            effect_id_as: "req".into(),
                        },
                    }),
                },
                PlanStep {
                    id: "end".into(),
                    kind: PlanStepKind::End(PlanStepEnd { result: None }),
                },
            ],
            edges: vec![
                PlanEdge {
                    from: "assign".into(),
                    to: "emit".into(),
                    when: Some(var_expr("flag")),
                },
                PlanEdge {
                    from: "emit".into(),
                    to: "end".into(),
                    when: None,
                },
            ],
            required_caps: vec!["cap_http".into()],
            allowed_effects: vec![EffectKind::HttpRequest],
            invariants: vec![],
        };
        let plan_name = plan.name.clone();

        // Guard true path produces effect and completes.
        let loaded_true = build_loaded_manifest(vec![plan.clone()], vec![start_trigger(&plan.name)], vec![], vec![]);
        let mut kernel_true =
            Kernel::from_loaded_manifest(Arc::new(MemStore::new()), loaded_true).unwrap();
        let true_input = plan_input_record(vec![("flag", ExprValue::Bool(true))]);
        kernel_true.submit_domain_event(START_SCHEMA, serde_cbor::to_vec(&true_input).unwrap());
        kernel_true.tick().unwrap();
        kernel_true.tick().unwrap();
        let effects = kernel_true.drain_effects();
        assert_eq!(effects.len(), 1);
        assert!(kernel_true.plan_instances.is_empty());

        // Guard false path blocks effect and keeps plan pending.
        let loaded_false = build_loaded_manifest(vec![plan], vec![start_trigger(&plan_name)], vec![], vec![]);
        let mut kernel_false =
            Kernel::from_loaded_manifest(Arc::new(MemStore::new()), loaded_false).unwrap();
        let false_input = plan_input_record(vec![("flag", ExprValue::Bool(false))]);
        kernel_false.submit_domain_event(START_SCHEMA, serde_cbor::to_vec(&false_input).unwrap());
        kernel_false.tick().unwrap();
        kernel_false.tick().unwrap();
        assert!(kernel_false.drain_effects().is_empty());
        assert!(!kernel_false.plan_instances.is_empty());
    }

    #[test]
    fn raised_events_are_routed_to_reducers() {
        let store = Arc::new(MemStore::new());
        let reducer_output = ReducerOutput {
            state: Some(vec![0xEE]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        };
        let output_bytes = reducer_output.encode().unwrap();
        let wasm_bytes = parse_str(&stub_reducer(&output_bytes)).expect("wat compile");
        let wasm_hash = store.put_blob(&wasm_bytes).unwrap();
        let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).unwrap();

        let reducer_module = DefModule {
            name: "com.acme/Reducer@1".into(),
            module_kind: ModuleKind::Reducer,
            wasm_hash: wasm_hash_ref,
            key_schema: None,
            abi: ModuleAbi { reducer: None },
        };

        let plan = DefPlan {
            name: "com.acme/Raise@1".into(),
            input: schema("com.acme/PlanIn@1"),
            output: None,
            locals: IndexMap::new(),
            steps: vec![
                PlanStep {
                    id: "raise".into(),
                    kind: PlanStepKind::RaiseEvent(PlanStepRaiseEvent {
                        reducer: reducer_module.name.clone(),
                        event: Expr::Record(ExprRecord {
                            record: IndexMap::from([
                                (
                                    "$schema".into(),
                                    text_expr("com.acme/Raised@1"),
                                ),
                                ("value".into(), Expr::Const(ExprConst::Int { int: 9 })),
                            ]),
                        }),
                        key: None,
                    }),
                },
                PlanStep {
                    id: "end".into(),
                    kind: PlanStepKind::End(PlanStepEnd { result: None }),
                },
            ],
            edges: vec![],
            required_caps: vec![],
            allowed_effects: vec![],
            invariants: vec![],
        };

        let routing = vec![RoutingEvent {
            event: schema("com.acme/Raised@1"),
            reducer: reducer_module.name.clone(),
            key_field: None,
        }];
        let loaded = build_loaded_manifest(
            vec![plan.clone()],
            vec![start_trigger(&plan.name)],
            vec![reducer_module],
            routing,
        );
        let mut kernel = Kernel::from_loaded_manifest(store, loaded).unwrap();

        let plan_input = plan_input_record(vec![]);
        kernel.submit_domain_event(START_SCHEMA, serde_cbor::to_vec(&plan_input).unwrap());
        kernel.tick().unwrap();
        kernel.tick().unwrap();
        kernel.tick().unwrap();

        assert_eq!(
            kernel.reducer_state("com.acme/Reducer@1"),
            Some(&vec![0xEE])
        );
    }
}
