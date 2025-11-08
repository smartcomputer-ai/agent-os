use std::collections::HashMap;

use aos_air_exec::{Env as ExprEnv, Value as ExprValue, eval_expr};
use aos_air_types::{DefPlan, Expr, PlanEdge, PlanStep, PlanStepKind};
use aos_effects::EffectIntent;
use aos_wasm_abi::DomainEvent;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_cbor;

use crate::effects::EffectManager;
use crate::error::KernelError;

#[derive(Default)]
pub struct PlanRegistry {
    plans: HashMap<String, DefPlan>,
}

impl PlanRegistry {
    pub fn register(&mut self, plan: DefPlan) {
        self.plans.insert(plan.name.clone(), plan);
    }

    pub fn get(&self, name: &str) -> Option<&DefPlan> {
        self.plans.get(name)
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Dependency {
    pred: String,
    guard: Option<aos_air_types::Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepState {
    Pending,
    WaitingReceipt,
    WaitingEvent,
    Completed,
}

pub struct PlanInstance {
    pub id: u64,
    pub name: String,
    pub plan: DefPlan,
    pub env: ExprEnv,
    pub completed: bool,
    effect_handles: HashMap<String, [u8; 32]>,
    receipt_wait: Option<ReceiptWait>,
    receipt_value: Option<ExprValue>,
    event_wait: Option<EventWait>,
    event_value: Option<ExprValue>,
    step_map: HashMap<String, PlanStep>,
    step_order: Vec<String>,
    predecessors: HashMap<String, Vec<Dependency>>,
    step_states: HashMap<String, StepState>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReceiptWait {
    pub step_id: String,
    pub intent_hash: [u8; 32],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventWait {
    pub step_id: String,
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub where_clause: Option<Expr>,
}

#[derive(Default, Debug)]
pub struct PlanTickOutcome {
    pub raised_events: Vec<DomainEvent>,
    pub waiting_receipt: Option<[u8; 32]>,
    pub waiting_event: Option<String>,
    pub completed: bool,
    pub intents_enqueued: Vec<EffectIntent>,
    pub result: Option<ExprValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanInstanceSnapshot {
    pub id: u64,
    pub name: String,
    pub env: ExprEnv,
    pub completed: bool,
    pub effect_handles: Vec<(String, [u8; 32])>,
    pub receipt_wait: Option<ReceiptWait>,
    pub receipt_value: Option<ExprValue>,
    pub event_wait: Option<EventWait>,
    pub event_value: Option<ExprValue>,
    pub step_states: Vec<(String, StepState)>,
}

impl PlanInstance {
    pub fn new(id: u64, plan: DefPlan, input: ExprValue) -> Self {
        let mut step_map = HashMap::new();
        let mut step_order = Vec::new();
        for step in &plan.steps {
            step_order.push(step.id.clone());
            step_map.insert(step.id.clone(), step.clone());
        }
        let mut predecessors: HashMap<String, Vec<Dependency>> = HashMap::new();
        for PlanEdge { from, to, when } in &plan.edges {
            predecessors
                .entry(to.clone())
                .or_default()
                .push(Dependency {
                    pred: from.clone(),
                    guard: when.clone(),
                });
        }
        let mut step_states = HashMap::new();
        for id in &step_order {
            step_states.insert(id.clone(), StepState::Pending);
        }
        Self {
            id,
            name: plan.name.clone(),
            plan,
            env: ExprEnv {
                plan_input: input,
                vars: IndexMap::new(),
                steps: IndexMap::new(),
                current_event: None,
            },
            completed: false,
            effect_handles: HashMap::new(),
            receipt_wait: None,
            receipt_value: None,
            event_wait: None,
            event_value: None,
            step_map,
            step_order,
            predecessors,
            step_states,
        }
    }

    pub fn tick(&mut self, effects: &mut EffectManager) -> Result<PlanTickOutcome, KernelError> {
        let mut outcome = PlanTickOutcome::default();
        if self.completed {
            outcome.completed = true;
            return Ok(outcome);
        }

        loop {
            if let Some(step_id) = self.next_ready_step()? {
                let step = self.step_map.get(&step_id).expect("step must exist");
                match &step.kind {
                    PlanStepKind::Assign(assign) => {
                        let value = eval_expr(&assign.expr, &self.env).map_err(|err| {
                            KernelError::Manifest(format!("plan assign eval error: {err}"))
                        })?;
                        self.env.vars.insert(assign.bind.var.clone(), value.clone());
                        self.record_step_value(&step_id, value);
                        self.complete_step(&step_id)?;
                    }
                    PlanStepKind::EmitEffect(emit) => {
                        let params_value = eval_expr(&emit.params, &self.env).map_err(|err| {
                            KernelError::Manifest(format!("plan effect eval error: {err}"))
                        })?;
                        let params_cbor = serde_cbor::to_vec(&params_value)
                            .map_err(|err| KernelError::Manifest(err.to_string()))?;
                        let intent = effects.enqueue_plan_effect(
                            &self.name,
                            &emit.kind,
                            &emit.cap,
                            params_cbor,
                        )?;
                        outcome.intents_enqueued.push(intent.clone());
                        let handle = emit.bind.effect_id_as.clone();
                        self.effect_handles
                            .insert(handle.clone(), intent.intent_hash);
                        let handle_value = ExprValue::Text(handle.clone());
                        self.env.vars.insert(handle.clone(), handle_value.clone());
                        let mut record = IndexMap::new();
                        record.insert("handle".into(), handle_value);
                        record.insert(
                            "intent_hash".into(),
                            ExprValue::Bytes(intent.intent_hash.to_vec()),
                        );
                        record.insert("params".into(), params_value);
                        self.record_step_value(&step_id, ExprValue::Record(record));
                        self.complete_step(&step_id)?;
                    }
                    PlanStepKind::AwaitReceipt(await_step) => {
                        if let Some(value) = self.receipt_value.take() {
                            self.env
                                .vars
                                .insert(await_step.bind.var.clone(), value.clone());
                            self.record_step_value(&step_id, value);
                            self.receipt_wait = None;
                            self.complete_step(&step_id)?;
                            continue;
                        }

                        let handle_value =
                            eval_expr(&await_step.for_expr, &self.env).map_err(|err| {
                                KernelError::Manifest(format!("plan await eval error: {err}"))
                            })?;
                        let handle = match handle_value {
                            ExprValue::Text(s) => s,
                            _ => {
                                return Err(KernelError::Manifest(
                                    "await_receipt expects handle text".into(),
                                ));
                            }
                        };
                        let intent_hash = *self.effect_handles.get(&handle).ok_or_else(|| {
                            KernelError::Manifest(format!("unknown effect handle '{handle}'"))
                        })?;
                        self.receipt_wait = Some(ReceiptWait {
                            step_id: step_id.clone(),
                            intent_hash,
                        });
                        self.step_states
                            .insert(step_id.clone(), StepState::WaitingReceipt);
                        outcome.waiting_receipt = Some(intent_hash);
                        return Ok(outcome);
                    }
                    PlanStepKind::AwaitEvent(await_event) => {
                        if let Some(value) = self.event_value.take() {
                            self.env
                                .vars
                                .insert(await_event.bind.var.clone(), value.clone());
                            self.record_step_value(&step_id, value);
                            self.event_wait = None;
                            self.complete_step(&step_id)?;
                            continue;
                        }

                        self.event_wait = Some(EventWait {
                            step_id: step_id.clone(),
                            schema: await_event.event.as_str().to_string(),
                            where_clause: await_event.where_clause.clone(),
                        });
                        self.step_states
                            .insert(step_id.clone(), StepState::WaitingEvent);
                        outcome.waiting_event = Some(await_event.event.as_str().to_string());
                        return Ok(outcome);
                    }
                    PlanStepKind::RaiseEvent(raise) => {
                        let value = eval_expr(&raise.event, &self.env).map_err(|err| {
                            KernelError::Manifest(format!("plan raise_event eval error: {err}"))
                        })?;
                        let event_record = value.clone();
                        let event = expr_value_to_domain_event(value)?;
                        outcome.raised_events.push(event);
                        self.record_step_value(&step_id, event_record);
                        self.complete_step(&step_id)?;
                    }
                    PlanStepKind::End(end) => {
                        match (&self.plan.output, &end.result) {
                            (Some(_), None) => {
                                return Err(KernelError::Manifest(
                                    "plan declares output schema but end result missing".into(),
                                ));
                            }
                            (None, Some(_)) => {
                                return Err(KernelError::Manifest(
                                    "plan without output schema cannot return a result".into(),
                                ));
                            }
                            _ => {}
                        }

                        if let Some(result_expr) = &end.result {
                            let value = eval_expr(result_expr, &self.env).map_err(|err| {
                                KernelError::Manifest(format!("plan end result eval error: {err}"))
                            })?;
                            self.record_step_value(&step_id, value.clone());
                            outcome.result = Some(value);
                        } else {
                            self.record_step_value(&step_id, ExprValue::Unit);
                        }

                        self.completed = true;
                        outcome.completed = true;
                        self.enforce_invariants()?;
                        return Ok(outcome);
                    }
                }
            } else {
                if self.all_steps_completed() {
                    self.completed = true;
                    outcome.completed = true;
                    self.enforce_invariants()?;
                }
                return Ok(outcome);
            }
        }
    }

    pub fn snapshot(&self) -> PlanInstanceSnapshot {
        PlanInstanceSnapshot {
            id: self.id,
            name: self.name.clone(),
            env: self.env.clone(),
            completed: self.completed,
            effect_handles: self
                .effect_handles
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            receipt_wait: self.receipt_wait.clone(),
            receipt_value: self.receipt_value.clone(),
            event_wait: self.event_wait.clone(),
            event_value: self.event_value.clone(),
            step_states: self
                .step_states
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
        }
    }

    pub fn from_snapshot(snapshot: PlanInstanceSnapshot, plan: DefPlan) -> Self {
        let mut instance = PlanInstance::new(snapshot.id, plan, snapshot.env.plan_input.clone());
        instance.env = snapshot.env;
        instance.completed = snapshot.completed;
        instance.effect_handles = snapshot.effect_handles.into_iter().collect();
        instance.receipt_wait = snapshot.receipt_wait;
        instance.receipt_value = snapshot.receipt_value;
        instance.event_wait = snapshot.event_wait;
        instance.event_value = snapshot.event_value;
        instance.step_states = snapshot.step_states.into_iter().collect();
        instance
    }

    pub fn deliver_receipt(
        &mut self,
        intent_hash: [u8; 32],
        payload: &[u8],
    ) -> Result<bool, KernelError> {
        if let Some(wait) = &self.receipt_wait {
            if wait.intent_hash == intent_hash {
                let value = match serde_cbor::from_slice(payload) {
                    Ok(v) => v,
                    Err(_) => ExprValue::Bytes(payload.to_vec()),
                };
                self.receipt_value = Some(value);
                self.step_states
                    .insert(wait.step_id.clone(), StepState::Pending);
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn pending_receipt_hash(&self) -> Option<[u8; 32]> {
        self.receipt_wait.as_ref().map(|wait| wait.intent_hash)
    }

    pub fn override_pending_receipt_hash(&mut self, hash: [u8; 32]) {
        if let Some(wait) = self.receipt_wait.as_mut() {
            wait.intent_hash = hash;
        }
    }

    pub fn deliver_event(&mut self, event: &DomainEvent) -> Result<bool, KernelError> {
        if let Some(wait) = &self.event_wait {
            if wait.schema == event.schema {
                let value = serde_cbor::from_slice(&event.value)
                    .unwrap_or(ExprValue::Bytes(event.value.clone()));
                if let Some(predicate) = &wait.where_clause {
                    let prev = self.env.push_event(value.clone());
                    let passes = eval_expr(predicate, &self.env).map_err(|err| {
                        KernelError::Manifest(format!("await_event where eval error: {err}"))
                    })?;
                    self.env.restore_event(prev);
                    if !value_to_bool(passes)? {
                        return Ok(false);
                    }
                }
                self.event_value = Some(value);
                self.step_states
                    .insert(wait.step_id.clone(), StepState::Pending);
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn waiting_event_schema(&self) -> Option<&str> {
        self.event_wait.as_ref().map(|w| w.schema.as_str())
    }

    fn record_step_value(&mut self, step_id: &str, value: ExprValue) {
        self.env.steps.insert(step_id.to_string(), value);
    }

    fn complete_step(&mut self, step_id: &str) -> Result<(), KernelError> {
        self.mark_completed(step_id);
        self.enforce_invariants()
    }

    fn enforce_invariants(&mut self) -> Result<(), KernelError> {
        for (idx, invariant) in self.plan.invariants.iter().enumerate() {
            let value = eval_expr(invariant, &self.env).map_err(|err| {
                KernelError::Manifest(format!("plan invariant {idx} eval error: {err}"))
            })?;
            if !value_to_bool(value)? {
                return Err(KernelError::PlanInvariantFailed {
                    plan: self.name.clone(),
                    index: idx,
                });
            }
        }
        Ok(())
    }

    fn next_ready_step(&self) -> Result<Option<String>, KernelError> {
        for id in &self.step_order {
            if matches!(self.step_states[id], StepState::Pending)
                && self.predecessors_satisfied(id)?
            {
                return Ok(Some(id.clone()));
            }
        }
        Ok(None)
    }

    fn predecessors_satisfied(&self, step_id: &str) -> Result<bool, KernelError> {
        if let Some(deps) = self.predecessors.get(step_id) {
            for dep in deps {
                if !matches!(self.step_states.get(&dep.pred), Some(StepState::Completed)) {
                    return Ok(false);
                }
                if let Some(expr) = &dep.guard {
                    let value = eval_expr(expr, &self.env)
                        .map_err(|err| KernelError::Manifest(format!("guard eval error: {err}")))?;
                    if !value_to_bool(value)? {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }

    fn mark_completed(&mut self, step_id: &str) {
        self.step_states
            .insert(step_id.to_string(), StepState::Completed);
        if self
            .step_states
            .values()
            .all(|s| matches!(s, StepState::Completed))
        {
            self.completed = true;
        }
    }

    fn all_steps_completed(&self) -> bool {
        self.step_states
            .values()
            .all(|state| matches!(state, StepState::Completed))
    }
}

fn value_to_bool(value: ExprValue) -> Result<bool, KernelError> {
    match value {
        ExprValue::Bool(v) => Ok(v),
        other => Err(KernelError::Manifest(format!(
            "guard expression must return bool, got {:?}",
            other
        ))),
    }
}

fn expr_value_to_domain_event(value: ExprValue) -> Result<DomainEvent, KernelError> {
    if let ExprValue::Record(mut map) = value {
        let schema_value = map
            .shift_remove("$schema")
            .ok_or_else(|| KernelError::Manifest("raise_event missing $schema".into()))?;
        let schema = match schema_value {
            ExprValue::Text(s) => s,
            _ => {
                return Err(KernelError::Manifest(
                    "raise_event $schema must be text".into(),
                ));
            }
        };
        let bytes = serde_cbor::to_vec(&ExprValue::Record(map))
            .map_err(|err| KernelError::Manifest(err.to_string()))?;
        Ok(DomainEvent::new(schema, bytes))
    } else {
        Err(KernelError::Manifest(
            "raise_event expects record value".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityResolver;
    use crate::policy::AllowAllPolicy;
    use aos_air_types::{
        CapType, EffectKind, Expr, ExprConst, ExprOp, ExprOpCode, ExprRecord, ExprRef, PlanBind,
        PlanBindEffect, PlanEdge, PlanStep, PlanStepAssign, PlanStepAwaitEvent,
        PlanStepAwaitReceipt, PlanStepEmitEffect, PlanStepEnd, PlanStepKind, SchemaRef,
    };
    use aos_effects::CapabilityGrant;

    fn base_plan(steps: Vec<PlanStep>) -> DefPlan {
        DefPlan {
            name: "test/plan@1".into(),
            input: aos_air_types::SchemaRef::new("test/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps,
            edges: vec![],
            required_caps: vec!["cap".into()],
            allowed_effects: vec![EffectKind::HttpRequest],
            invariants: vec![],
        }
    }

    fn default_env() -> ExprValue {
        ExprValue::Record(IndexMap::new())
    }

    fn test_effect_manager() -> EffectManager {
        let grants = vec![
            (
                CapabilityGrant {
                    name: "cap".into(),
                    cap: "sys/http.out@1".into(),
                    params_cbor: Vec::new(),
                    expiry_ns: None,
                    budget: None,
                },
                CapType::HttpOut,
            ),
            (
                CapabilityGrant {
                    name: "cap_http".into(),
                    cap: "sys/http.out@1".into(),
                    params_cbor: Vec::new(),
                    expiry_ns: None,
                    budget: None,
                },
                CapType::HttpOut,
            ),
        ];
        let resolver = CapabilityResolver::from_runtime_grants(grants);
        EffectManager::new(resolver, Box::new(AllowAllPolicy))
    }

    /// Assign steps should synchronously write to the plan environment.
    #[test]
    fn assign_step_updates_env() {
        let steps = vec![PlanStep {
            id: "assign".into(),
            kind: PlanStepKind::Assign(aos_air_types::PlanStepAssign {
                expr: Expr::Const(ExprConst::Int { int: 42 }),
                bind: aos_air_types::PlanBind {
                    var: "answer".into(),
                },
            }),
        }];
        let mut plan = PlanInstance::new(1, base_plan(steps), default_env());
        let mut effects = test_effect_manager();
        let outcome = plan.tick(&mut effects).unwrap();
        assert!(outcome.completed);
        assert_eq!(plan.env.vars.get("answer").unwrap(), &ExprValue::Int(42));
    }

    /// `emit_effect` should enqueue an intent and record the effect handle for later awaits.
    #[test]
    fn emit_effect_enqueues_intent() {
        let steps = vec![PlanStep {
            id: "emit".into(),
            kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                kind: EffectKind::HttpRequest,
                params: Expr::Const(ExprConst::Text {
                    text: "data".into(),
                }),
                cap: "cap".into(),
                bind: PlanBindEffect {
                    effect_id_as: "req".into(),
                },
            }),
        }];
        let mut plan = PlanInstance::new(1, base_plan(steps), default_env());
        let mut effects = test_effect_manager();
        let outcome = plan.tick(&mut effects).unwrap();
        assert!(outcome.completed);
        assert_eq!(effects.drain().len(), 1);
        assert!(plan.effect_handles.contains_key("req"));
    }

    /// Plans must block on `await_receipt` until the referenced effect handle is fulfilled.
    #[test]
    fn await_receipt_waits_and_resumes() {
        let steps = vec![
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::HttpRequest,
                    params: Expr::Const(ExprConst::Text {
                        text: "data".into(),
                    }),
                    cap: "cap".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "await".into(),
                kind: PlanStepKind::AwaitReceipt(aos_air_types::PlanStepAwaitReceipt {
                    for_expr: Expr::Const(ExprConst::Text { text: "req".into() }),
                    bind: aos_air_types::PlanBind { var: "rcpt".into() },
                }),
            },
        ];
        let mut plan = PlanInstance::new(1, base_plan(steps), default_env());
        let mut effects = test_effect_manager();
        let first = plan.tick(&mut effects).unwrap();
        assert!(first.waiting_receipt.is_some());
        let hash = first.waiting_receipt.unwrap();
        assert!(plan.deliver_receipt(hash, b"\x01").unwrap());
        let second = plan.tick(&mut effects).unwrap();
        assert!(second.completed);
        assert!(plan.env.vars.contains_key("rcpt"));
    }

    /// `await_event` pauses the plan until a matching schema arrives and binds it into the env.
    #[test]
    fn await_event_waits_for_schema() {
        let steps = vec![PlanStep {
            id: "await".into(),
            kind: PlanStepKind::AwaitEvent(aos_air_types::PlanStepAwaitEvent {
                event: aos_air_types::SchemaRef::new("com.test/Evt@1").unwrap(),
                where_clause: None,
                bind: aos_air_types::PlanBind { var: "evt".into() },
            }),
        }];
        let mut plan = PlanInstance::new(1, base_plan(steps), default_env());
        let mut effects = test_effect_manager();
        let outcome = plan.tick(&mut effects).unwrap();
        assert_eq!(outcome.waiting_event, Some("com.test/Evt@1".into()));
        let event = DomainEvent::new(
            "com.test/Evt@1",
            serde_cbor::to_vec(&ExprValue::Int(5)).unwrap(),
        );
        assert!(plan.deliver_event(&event).unwrap());
        let outcome2 = plan.tick(&mut effects).unwrap();
        assert!(outcome2.completed);
        assert!(plan.env.vars.contains_key("evt"));
    }

    /// Guarded edges should prevent downstream steps from running when the guard is false.
    #[test]
    fn guard_blocks_step_until_true() {
        let mut plan = base_plan(vec![PlanStep {
            id: "end".into(),
            kind: PlanStepKind::End(PlanStepEnd { result: None }),
        }]);
        plan.edges.push(PlanEdge {
            from: "start".into(),
            to: "end".into(),
            when: Some(Expr::Const(ExprConst::Bool { bool: false })),
        });
        let mut instance = PlanInstance::new(1, plan, default_env());
        let mut effects = test_effect_manager();
        let outcome = instance.tick(&mut effects).unwrap();
        assert!(!outcome.completed);
    }

    /// Raising an event should surface a DomainEvent with the serialized payload.
    #[test]
    fn raise_event_produces_domain_event() {
        let steps = vec![PlanStep {
            id: "raise".into(),
            kind: PlanStepKind::RaiseEvent(aos_air_types::PlanStepRaiseEvent {
                reducer: "irrelevant".into(),
                key: None,
                event: Expr::Record(aos_air_types::ExprRecord {
                    record: IndexMap::from([
                        (
                            "$schema".into(),
                            Expr::Const(ExprConst::Text {
                                text: "com.test/Evt@1".into(),
                            }),
                        ),
                        ("value".into(), Expr::Const(ExprConst::Int { int: 9 })),
                    ]),
                }),
            }),
        }];
        let mut plan = PlanInstance::new(1, base_plan(steps), default_env());
        let mut effects = test_effect_manager();
        let outcome = plan.tick(&mut effects).unwrap();
        assert_eq!(outcome.raised_events.len(), 1);
        assert_eq!(outcome.raised_events[0].schema, "com.test/Evt@1");
    }

    #[test]
    fn step_values_are_available_via_step_refs() {
        let steps = vec![
            PlanStep {
                id: "first".into(),
                kind: PlanStepKind::Assign(PlanStepAssign {
                    expr: Expr::Record(ExprRecord {
                        record: IndexMap::from([(
                            "status".into(),
                            Expr::Const(ExprConst::Text { text: "ok".into() }),
                        )]),
                    }),
                    bind: PlanBind {
                        var: "first_state".into(),
                    },
                }),
            },
            PlanStep {
                id: "second".into(),
                kind: PlanStepKind::Assign(PlanStepAssign {
                    expr: Expr::Ref(ExprRef {
                        reference: "@step:first.status".into(),
                    }),
                    bind: PlanBind {
                        var: "copied".into(),
                    },
                }),
            },
        ];
        let mut plan = base_plan(steps);
        plan.edges.push(PlanEdge {
            from: "first".into(),
            to: "second".into(),
            when: None,
        });
        let mut instance = PlanInstance::new(1, plan, default_env());
        let mut effects = test_effect_manager();
        let outcome = instance.tick(&mut effects).unwrap();
        assert!(outcome.completed);
        assert_eq!(
            instance.env.vars.get("copied"),
            Some(&ExprValue::Text("ok".into()))
        );
    }

    #[test]
    fn await_event_where_clause_filters_events() {
        let steps = vec![
            PlanStep {
                id: "await".into(),
                kind: PlanStepKind::AwaitEvent(PlanStepAwaitEvent {
                    event: SchemaRef::new("com.test/Evt@1").unwrap(),
                    where_clause: Some(Expr::Op(ExprOp {
                        op: ExprOpCode::Eq,
                        args: vec![
                            Expr::Ref(ExprRef {
                                reference: "@event.correlation_id".into(),
                            }),
                            Expr::Const(ExprConst::Text {
                                text: "match".into(),
                            }),
                        ],
                    })),
                    bind: PlanBind { var: "evt".into() },
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd { result: None }),
            },
        ];
        let mut plan = base_plan(steps);
        plan.edges.push(PlanEdge {
            from: "await".into(),
            to: "end".into(),
            when: None,
        });
        let mut instance = PlanInstance::new(1, plan, default_env());
        let mut effects = test_effect_manager();
        let outcome = instance.tick(&mut effects).unwrap();
        assert_eq!(outcome.waiting_event, Some("com.test/Evt@1".into()));

        let mismatch_event = DomainEvent::new(
            "com.test/Evt@1",
            serde_cbor::to_vec(&ExprValue::Record(IndexMap::from([(
                "correlation_id".into(),
                ExprValue::Text("nope".into()),
            )])))
            .unwrap(),
        );
        assert!(!instance.deliver_event(&mismatch_event).unwrap());

        let match_event = DomainEvent::new(
            "com.test/Evt@1",
            serde_cbor::to_vec(&ExprValue::Record(IndexMap::from([(
                "correlation_id".into(),
                ExprValue::Text("match".into()),
            )])))
            .unwrap(),
        );
        assert!(instance.deliver_event(&match_event).unwrap());
        let outcome2 = instance.tick(&mut effects).unwrap();
        assert!(outcome2.completed);
        assert_eq!(
            instance.env.vars.get("evt"),
            Some(&ExprValue::Record(IndexMap::from([(
                "correlation_id".into(),
                ExprValue::Text("match".into()),
            )])))
        );
    }

    #[test]
    fn end_step_returns_result_when_schema_declared() {
        let steps = vec![PlanStep {
            id: "end".into(),
            kind: PlanStepKind::End(PlanStepEnd {
                result: Some(Expr::Const(ExprConst::Int { int: 7 })),
            }),
        }];
        let mut plan = base_plan(steps);
        plan.output = Some(SchemaRef::new("test/Out@1").unwrap());
        let mut instance = PlanInstance::new(1, plan, default_env());
        let mut effects = test_effect_manager();
        let outcome = instance.tick(&mut effects).unwrap();
        assert!(outcome.completed);
        assert_eq!(outcome.result, Some(ExprValue::Int(7)));
    }

    #[test]
    fn end_step_requires_result_when_schema_present() {
        let steps = vec![PlanStep {
            id: "end".into(),
            kind: PlanStepKind::End(PlanStepEnd { result: None }),
        }];
        let mut plan = base_plan(steps);
        plan.output = Some(SchemaRef::new("test/Out@1").unwrap());
        let mut instance = PlanInstance::new(1, plan, default_env());
        let mut effects = test_effect_manager();
        let err = instance.tick(&mut effects).unwrap_err();
        assert!(matches!(err, KernelError::Manifest(msg) if msg.contains("output schema")));
    }

    #[test]
    fn end_step_cannot_return_without_schema() {
        let steps = vec![PlanStep {
            id: "end".into(),
            kind: PlanStepKind::End(PlanStepEnd {
                result: Some(Expr::Const(ExprConst::Nat { nat: 1 })),
            }),
        }];
        let plan = base_plan(steps);
        let mut instance = PlanInstance::new(1, plan, default_env());
        let mut effects = test_effect_manager();
        let err = instance.tick(&mut effects).unwrap_err();
        assert!(matches!(err, KernelError::Manifest(msg) if msg.contains("without output schema")));
    }

    #[test]
    fn invariant_violation_errors_out_plan() {
        let steps = vec![
            PlanStep {
                id: "set_ok".into(),
                kind: PlanStepKind::Assign(PlanStepAssign {
                    expr: Expr::Const(ExprConst::Int { int: 5 }),
                    bind: PlanBind { var: "val".into() },
                }),
            },
            PlanStep {
                id: "set_bad".into(),
                kind: PlanStepKind::Assign(PlanStepAssign {
                    expr: Expr::Const(ExprConst::Int { int: 20 }),
                    bind: PlanBind { var: "val".into() },
                }),
            },
        ];
        let mut plan = base_plan(steps);
        plan.edges.push(PlanEdge {
            from: "set_ok".into(),
            to: "set_bad".into(),
            when: None,
        });
        plan.invariants.push(Expr::Op(ExprOp {
            op: ExprOpCode::Lt,
            args: vec![
                Expr::Ref(ExprRef {
                    reference: "@var:val".into(),
                }),
                Expr::Const(ExprConst::Int { int: 10 }),
            ],
        }));
        let mut instance = PlanInstance::new(1, plan, default_env());
        let mut effects = test_effect_manager();
        let err = instance.tick(&mut effects).unwrap_err();
        assert!(matches!(err, KernelError::PlanInvariantFailed { .. }));
    }

    #[test]
    fn snapshot_restores_waiting_receipt_state() {
        let steps = vec![
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::HttpRequest,
                    params: Expr::Const(ExprConst::Text {
                        text: "payload".into(),
                    }),
                    cap: "cap".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "await".into(),
                kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                    for_expr: Expr::Const(ExprConst::Text { text: "req".into() }),
                    bind: PlanBind { var: "resp".into() },
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd { result: None }),
            },
        ];
        let mut plan_def = base_plan(steps);
        plan_def.edges.extend([
            PlanEdge {
                from: "emit".into(),
                to: "await".into(),
                when: None,
            },
            PlanEdge {
                from: "await".into(),
                to: "end".into(),
                when: None,
            },
        ]);
        let mut instance = PlanInstance::new(1, plan_def.clone(), default_env());
        let mut effects = test_effect_manager();
        let first = instance.tick(&mut effects).unwrap();
        let mut hash = first.waiting_receipt.expect("waiting receipt");
        let snapshot = instance.snapshot();

        let mut restored = PlanInstance::from_snapshot(snapshot, plan_def);
        hash[0] ^= 0xAA;
        restored.override_pending_receipt_hash(hash);
        assert_eq!(restored.pending_receipt_hash(), Some(hash));
        assert!(restored.deliver_receipt(hash, b"\x01").unwrap());
        let mut effects = test_effect_manager();
        let outcome = restored.tick(&mut effects).unwrap();
        assert!(outcome.completed);
        assert!(restored.receipt_wait.is_none());
    }
}
