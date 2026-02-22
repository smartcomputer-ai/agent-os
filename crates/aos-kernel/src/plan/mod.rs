use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use aos_air_exec::{Env as ExprEnv, Value as ExprValue, eval_expr};
use aos_air_types::plan_literals::SchemaIndex;
use aos_air_types::{DefPlan, Expr, PlanEdge, PlanStep, PlanStepKind, TypeExpr};
use aos_cbor::to_canonical_cbor;
use aos_effects::EffectIntent;
use aos_wasm_abi::DomainEvent;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::capability::CapGrantResolution;
use crate::effects::EffectManager;
use crate::error::KernelError;
use crate::event::IngressStamp;

mod codec;
mod readiness;
mod step_handlers;
mod waits;
use self::step_handlers::StepTickControl;
pub(crate) use codec::{eval_expr_or_value, expr_value_to_cbor_value, value_to_bool};

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
pub struct ReducerSchema {
    pub event_schema_name: String,
    pub event_schema: TypeExpr,
    pub key_schema: Option<TypeExpr>,
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
    WaitingPlan,
    Completed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanHandleValue {
    pub instance_id: u64,
    pub plan: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlanSpawnRequest {
    SpawnPlan {
        step_id: String,
        child_plan: String,
        input: ExprValue,
    },
    SpawnForEach {
        step_id: String,
        child_plan: String,
        inputs: Vec<ExprValue>,
        max_fanout: Option<u64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlanWaitRequest {
    AwaitPlan {
        step_id: String,
        handle: PlanHandleValue,
    },
    AwaitPlansAll {
        step_id: String,
        handles: Vec<PlanHandleValue>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPlanWait {
    pub handles: Vec<PlanHandleValue>,
    pub mode: PendingPlanWaitMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PendingPlanWaitMode {
    One,
    All,
}

pub type PlanCompletionValue = ExprValue;

pub struct PlanInstance {
    pub id: u64,
    pub name: String,
    pub plan: DefPlan,
    pub env: ExprEnv,
    pub completed: bool,
    context: Option<PlanContext>,
    effect_handles: HashMap<String, [u8; 32]>,
    receipt_waits: BTreeMap<[u8; 32], ReceiptWait>,
    receipt_values: HashMap<String, ExprValue>,
    spawn_values: HashMap<String, ExprValue>,
    plan_waits: HashMap<String, PendingPlanWait>,
    plan_wait_values: HashMap<String, ExprValue>,
    event_wait: Option<EventWait>,
    event_value: Option<ExprValue>,
    correlation_id: Option<Vec<u8>>,
    step_map: HashMap<String, PlanStep>,
    step_order: Vec<String>,
    predecessors: HashMap<String, Vec<Dependency>>,
    step_states: HashMap<String, StepState>,
    schema_index: Arc<SchemaIndex>,
    cap_handles: Arc<HashMap<String, CapGrantResolution>>,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanContext {
    pub logical_now_ns: u64,
    pub journal_height: u64,
    pub manifest_hash: String,
}

impl PlanContext {
    pub fn from_stamp(stamp: &IngressStamp) -> Self {
        Self {
            logical_now_ns: stamp.logical_now_ns,
            journal_height: stamp.journal_height,
            manifest_hash: stamp.manifest_hash.clone(),
        }
    }
}

#[derive(Default, Debug)]
pub struct PlanTickOutcome {
    pub raised_events: Vec<DomainEvent>,
    pub waiting_receipts: Vec<[u8; 32]>,
    pub waiting_event: Option<String>,
    pub waiting_plans: bool,
    pub spawn_requests: Vec<PlanSpawnRequest>,
    pub wait_requests: Vec<PlanWaitRequest>,
    pub completed: bool,
    pub intents_enqueued: Vec<EffectIntent>,
    pub result: Option<ExprValue>,
    pub result_schema: Option<String>,
    pub result_cbor: Option<Vec<u8>>,
    pub plan_error: Option<PlanError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanError {
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanInstanceSnapshot {
    pub id: u64,
    pub name: String,
    pub env: ExprEnv,
    pub completed: bool,
    pub context: Option<PlanContext>,
    pub effect_handles: Vec<(String, [u8; 32])>,
    pub receipt_waits: Vec<ReceiptWait>,
    pub receipt_values: Vec<(String, ExprValue)>,
    pub spawn_values: Vec<(String, ExprValue)>,
    pub plan_waits: Vec<(String, PendingPlanWait)>,
    pub plan_wait_values: Vec<(String, ExprValue)>,
    pub event_wait: Option<EventWait>,
    pub event_value: Option<ExprValue>,
    pub correlation_id: Option<Vec<u8>>,
    pub step_states: Vec<(String, StepState)>,
}

impl PlanInstance {
    pub fn new(
        id: u64,
        plan: DefPlan,
        input: ExprValue,
        schema_index: Arc<SchemaIndex>,
        correlation: Option<(Vec<u8>, ExprValue)>,
        cap_handles: Arc<HashMap<String, CapGrantResolution>>,
    ) -> Self {
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
        let mut env = ExprEnv {
            plan_input: input,
            vars: IndexMap::new(),
            steps: IndexMap::new(),
            current_event: None,
        };
        let mut correlation_id = None;
        if let Some((bytes, value)) = correlation {
            env.vars.insert("correlation_id".into(), value);
            correlation_id = Some(bytes);
        }
        Self {
            id,
            name: plan.name.clone(),
            plan,
            env,
            completed: false,
            context: None,
            effect_handles: HashMap::new(),
            receipt_waits: BTreeMap::new(),
            receipt_values: HashMap::new(),
            spawn_values: HashMap::new(),
            plan_waits: HashMap::new(),
            plan_wait_values: HashMap::new(),
            event_wait: None,
            event_value: None,
            correlation_id,
            step_map,
            step_order,
            predecessors,
            step_states,
            schema_index,
            cap_handles,
        }
    }

    pub fn tick(&mut self, effects: &mut EffectManager) -> Result<PlanTickOutcome, KernelError> {
        let mut outcome = PlanTickOutcome::default();
        if self.completed {
            outcome.completed = true;
            return Ok(outcome);
        }

        loop {
            let ready_steps = self.ready_steps()?;
            if ready_steps.is_empty() {
                if !self.receipt_waits.is_empty() {
                    outcome
                        .waiting_receipts
                        .extend(self.receipt_waits.keys().copied());
                    return Ok(outcome);
                }
                if let Some(wait) = &self.event_wait {
                    outcome.waiting_event = Some(wait.schema.clone());
                    return Ok(outcome);
                }
                if self
                    .step_states
                    .values()
                    .any(|state| matches!(state, StepState::WaitingPlan))
                {
                    outcome.waiting_plans = true;
                    return Ok(outcome);
                }
                if self.all_steps_completed() {
                    self.completed = true;
                    outcome.completed = true;
                    if let Some(err) = self.enforce_invariants()? {
                        self.completed = true;
                        outcome.plan_error = Some(err);
                        return Ok(outcome);
                    }
                }
                return Ok(outcome);
            }

            let mut progressed = false;
            let mut waiting_registered = false;

            let emit_ready: Vec<String> = ready_steps
                .iter()
                .filter(|id| matches!(self.step_map[*id].kind, PlanStepKind::EmitEffect(_)))
                .cloned()
                .collect();
            match self.run_emit_ready_steps(&emit_ready, effects, &mut outcome)? {
                StepTickControl::Return => return Ok(outcome),
                StepTickControl::RestartTick => continue,
                StepTickControl::Continue => {}
            }

            for step_id in ready_steps {
                let step = self
                    .step_map
                    .get(&step_id)
                    .expect("step must exist")
                    .clone();
                match self.process_ready_step(
                    step,
                    &step_id,
                    &mut outcome,
                    &mut waiting_registered,
                )? {
                    StepTickControl::Return => return Ok(outcome),
                    StepTickControl::RestartTick => {
                        progressed = true;
                        break;
                    }
                    StepTickControl::Continue => {}
                }
            }

            if progressed {
                continue;
            }

            if waiting_registered {
                return Ok(outcome);
            }

            if !self.receipt_waits.is_empty() {
                outcome
                    .waiting_receipts
                    .extend(self.receipt_waits.keys().copied());
                return Ok(outcome);
            }

            if self.all_steps_completed() {
                self.completed = true;
                outcome.completed = true;
                self.enforce_invariants()?;
            }
            return Ok(outcome);
        }
    }

    pub fn snapshot(&self) -> PlanInstanceSnapshot {
        PlanInstanceSnapshot {
            id: self.id,
            name: self.name.clone(),
            env: self.env.clone(),
            completed: self.completed,
            context: self.context.clone(),
            effect_handles: self
                .effect_handles
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            receipt_waits: self.receipt_waits.values().cloned().collect(),
            receipt_values: self
                .receipt_values
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            spawn_values: self
                .spawn_values
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            plan_waits: self
                .plan_waits
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            plan_wait_values: self
                .plan_wait_values
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            event_wait: self.event_wait.clone(),
            event_value: self.event_value.clone(),
            correlation_id: self.correlation_id.clone(),
            step_states: self
                .step_states
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
        }
    }

    pub fn from_snapshot(
        snapshot: PlanInstanceSnapshot,
        plan: DefPlan,
        schema_index: Arc<SchemaIndex>,
        cap_handles: Arc<HashMap<String, CapGrantResolution>>,
    ) -> Self {
        let mut instance = PlanInstance::new(
            snapshot.id,
            plan,
            snapshot.env.plan_input.clone(),
            schema_index,
            None,
            cap_handles,
        );
        instance.env = snapshot.env;
        instance.completed = snapshot.completed;
        instance.context = snapshot.context;
        instance.effect_handles = snapshot.effect_handles.into_iter().collect();
        instance.receipt_waits = snapshot
            .receipt_waits
            .into_iter()
            .map(|wait| (wait.intent_hash, wait))
            .collect();
        instance.receipt_values = snapshot.receipt_values.into_iter().collect();
        instance.spawn_values = snapshot.spawn_values.into_iter().collect();
        instance.plan_waits = snapshot.plan_waits.into_iter().collect();
        instance.plan_wait_values = snapshot.plan_wait_values.into_iter().collect();
        instance.event_wait = snapshot.event_wait;
        instance.event_value = snapshot.event_value;
        instance.correlation_id = snapshot.correlation_id;
        instance.step_states = snapshot.step_states.into_iter().collect();
        instance
    }

    pub fn set_context(&mut self, context: PlanContext) {
        self.context = Some(context);
    }

    pub fn context(&self) -> Option<&PlanContext> {
        self.context.as_ref()
    }

    pub fn deliver_spawn_value(&mut self, step_id: &str, value: ExprValue) -> bool {
        if !self.step_map.contains_key(step_id) {
            return false;
        }
        self.spawn_values.insert(step_id.to_string(), value);
        self.step_states
            .insert(step_id.to_string(), StepState::Pending);
        true
    }

    pub fn pending_wait_child_ids(&self, step_id: &str) -> Vec<u64> {
        self.plan_waits
            .get(step_id)
            .map(|wait| wait.handles.iter().map(|h| h.instance_id).collect())
            .unwrap_or_default()
    }

    pub fn resolve_plan_waits(&mut self, completed: &HashMap<u64, PlanCompletionValue>) -> bool {
        let mut resolved = false;
        let pending: Vec<(String, PendingPlanWait)> = self
            .plan_waits
            .iter()
            .map(|(step, wait)| (step.clone(), wait.clone()))
            .collect();
        for (step_id, wait) in pending {
            if let Some(value) = build_resolved_wait_value(&wait, completed) {
                self.plan_waits.remove(&step_id);
                self.plan_wait_values.insert(step_id.clone(), value);
                self.step_states.insert(step_id, StepState::Pending);
                resolved = true;
            }
        }
        resolved
    }

    fn record_step_value(&mut self, step_id: &str, value: ExprValue) {
        self.env.steps.insert(step_id.to_string(), value);
    }

    fn complete_step(&mut self, step_id: &str) -> Result<Option<PlanError>, KernelError> {
        self.mark_completed(step_id);
        self.enforce_invariants()
    }

    fn invariant_violation_error(&self) -> PlanError {
        PlanError {
            code: "invariant_violation".into(),
        }
    }

    fn enforce_invariants(&mut self) -> Result<Option<PlanError>, KernelError> {
        for (idx, invariant) in self.plan.invariants.iter().enumerate() {
            let value = eval_expr(invariant, &self.env).map_err(|err| {
                KernelError::Manifest(format!("plan invariant {idx} eval error: {err}"))
            })?;
            if !value_to_bool(value)? {
                return Ok(Some(self.invariant_violation_error()));
            }
        }
        Ok(None)
    }
}

fn build_resolved_wait_value(
    wait: &PendingPlanWait,
    completed: &HashMap<u64, PlanCompletionValue>,
) -> Option<ExprValue> {
    match wait.mode {
        PendingPlanWaitMode::One => {
            let handle = wait.handles.first()?;
            completed.get(&handle.instance_id).cloned()
        }
        PendingPlanWaitMode::All => {
            let mut values = Vec::with_capacity(wait.handles.len());
            for handle in &wait.handles {
                let value = completed.get(&handle.instance_id)?.clone();
                values.push(value);
            }
            Some(ExprValue::List(values))
        }
    }
}
