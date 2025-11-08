use std::collections::HashMap;

use aos_air_exec::{Env as ExprEnv, Value as ExprValue, eval_expr};
use aos_air_types::{DefPlan, PlanEdge, PlanStep, PlanStepKind};
use aos_wasm_abi::DomainEvent;
use indexmap::IndexMap;
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

#[derive(Clone)]
struct Dependency {
    pred: String,
    guard: Option<aos_air_types::Expr>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StepState {
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

struct ReceiptWait {
    step_id: String,
    intent_hash: [u8; 32],
}

struct EventWait {
    step_id: String,
    schema: String,
}

#[derive(Default)]
pub struct PlanTickOutcome {
    pub raised_events: Vec<DomainEvent>,
    pub waiting_receipt: Option<[u8; 32]>,
    pub waiting_event: Option<String>,
    pub completed: bool,
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
                        self.env.vars.insert(assign.bind.var.clone(), value);
                        self.mark_completed(&step_id);
                    }
                    PlanStepKind::EmitEffect(emit) => {
                        let value = eval_expr(&emit.params, &self.env).map_err(|err| {
                            KernelError::Manifest(format!("plan effect eval error: {err}"))
                        })?;
                        let params_cbor = serde_cbor::to_vec(&value)
                            .map_err(|err| KernelError::Manifest(err.to_string()))?;
                        let intent_hash = effects.enqueue_plan_effect(
                            &self.name,
                            &emit.kind,
                            &emit.cap,
                            params_cbor,
                        )?;
                        let handle = emit.bind.effect_id_as.clone();
                        self.effect_handles.insert(handle.clone(), intent_hash);
                        self.env
                            .vars
                            .insert(handle.clone(), ExprValue::Text(handle));
                        self.mark_completed(&step_id);
                    }
                    PlanStepKind::AwaitReceipt(await_step) => {
                        if let Some(value) = self.receipt_value.take() {
                            self.env.vars.insert(await_step.bind.var.clone(), value);
                            self.receipt_wait = None;
                            self.mark_completed(&step_id);
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
                            self.env.vars.insert(await_event.bind.var.clone(), value);
                            self.event_wait = None;
                            self.mark_completed(&step_id);
                            continue;
                        }

                        if await_event.where_clause.is_some() {
                            return Err(KernelError::Manifest(
                                "await_event.where not yet supported".into(),
                            ));
                        }

                        self.event_wait = Some(EventWait {
                            step_id: step_id.clone(),
                            schema: await_event.event.as_str().to_string(),
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
                        let event = expr_value_to_domain_event(value)?;
                        outcome.raised_events.push(event);
                        self.mark_completed(&step_id);
                    }
                    PlanStepKind::End(end) => {
                        if end.result.is_some() {
                            return Err(KernelError::Manifest(
                                "end.result enforcement not implemented".into(),
                            ));
                        }
                        self.completed = true;
                        outcome.completed = true;
                        return Ok(outcome);
                    }
                }
            } else {
                if self.all_steps_completed() {
                    self.completed = true;
                    outcome.completed = true;
                }
                return Ok(outcome);
            }
        }
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

    pub fn deliver_event(&mut self, event: &DomainEvent) -> Result<bool, KernelError> {
        if let Some(wait) = &self.event_wait {
            if wait.schema == event.schema {
                let value = serde_cbor::from_slice(&event.value)
                    .unwrap_or(ExprValue::Bytes(event.value.clone()));
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
    use crate::capability::{AllowAllPolicy, CapabilityResolver};
    use aos_air_types::{
        CapType, EffectKind, Expr, ExprConst, PlanBindEffect, PlanStep, PlanStepEmitEffect,
        PlanStepEnd, PlanStepKind,
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
        EffectManager::new(resolver, AllowAllPolicy)
    }

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
}
