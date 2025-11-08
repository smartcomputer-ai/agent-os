use std::collections::HashMap;

use aos_air_exec::{Env as ExprEnv, Value as ExprValue, eval_expr};
use aos_air_types::DefPlan;
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

pub struct PlanInstance {
    pub id: u64,
    pub name: String,
    pub plan: DefPlan,
    pub step_idx: usize,
    pub env: ExprEnv,
    pub completed: bool,
    effect_handles: HashMap<String, [u8; 32]>,
    receipt_wait: Option<ReceiptWait>,
    receipt_value: Option<ExprValue>,
    event_wait: Option<EventWait>,
    event_value: Option<ExprValue>,
}

struct ReceiptWait {
    intent_hash: [u8; 32],
}

struct EventWait {
    schema: String,
    bind_var: String,
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
        Self {
            id,
            name: plan.name.clone(),
            plan,
            step_idx: 0,
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
        }
    }

    pub fn tick(&mut self, effects: &mut EffectManager) -> Result<PlanTickOutcome, KernelError> {
        let mut outcome = PlanTickOutcome::default();
        if self.completed {
            outcome.completed = true;
            return Ok(outcome);
        }

        loop {
            if self.step_idx >= self.plan.steps.len() {
                self.completed = true;
                outcome.completed = true;
                return Ok(outcome);
            }

            let step = &self.plan.steps[self.step_idx];
            match &step.kind {
                aos_air_types::PlanStepKind::Assign(assign) => {
                    let value = eval_expr(&assign.expr, &self.env).map_err(|err| {
                        KernelError::Manifest(format!("plan assign eval error: {err}"))
                    })?;
                    self.env.vars.insert(assign.bind.var.clone(), value);
                    self.step_idx += 1;
                }
                aos_air_types::PlanStepKind::EmitEffect(emit) => {
                    let value = eval_expr(&emit.params, &self.env).map_err(|err| {
                        KernelError::Manifest(format!("plan effect eval error: {err}"))
                    })?;
                    let params_cbor = serde_cbor::to_vec(&value)
                        .map_err(|err| KernelError::Manifest(err.to_string()))?;
                    let intent_hash =
                        effects.enqueue_plan_effect(&emit.kind, &emit.cap, params_cbor)?;
                    let handle = emit.bind.effect_id_as.clone();
                    self.effect_handles.insert(handle.clone(), intent_hash);
                    self.env
                        .vars
                        .insert(handle.clone(), ExprValue::Text(handle));
                    self.step_idx += 1;
                }
                aos_air_types::PlanStepKind::AwaitReceipt(await_step) => {
                    if let Some(value) = self.receipt_value.take() {
                        self.env.vars.insert(await_step.bind.var.clone(), value);
                        self.receipt_wait = None;
                        self.step_idx += 1;
                        continue;
                    }

                    if self.receipt_wait.is_none() {
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
                        self.receipt_wait = Some(ReceiptWait { intent_hash });
                        outcome.waiting_receipt = Some(intent_hash);
                        return Ok(outcome);
                    } else if let Some(wait) = &self.receipt_wait {
                        outcome.waiting_receipt = Some(wait.intent_hash);
                        return Ok(outcome);
                    }
                }
                aos_air_types::PlanStepKind::AwaitEvent(await_event) => {
                    if let Some(value) = self.event_value.take() {
                        self.env.vars.insert(await_event.bind.var.clone(), value);
                        self.event_wait = None;
                        self.step_idx += 1;
                        continue;
                    }

                    if await_event.where_clause.is_some() {
                        return Err(KernelError::Manifest(
                            "await_event.where not yet supported".into(),
                        ));
                    }

                    if self.event_wait.is_none() {
                        let schema = await_event.event.as_str().to_string();
                        self.event_wait = Some(EventWait {
                            schema: schema.clone(),
                            bind_var: await_event.bind.var.clone(),
                        });
                        outcome.waiting_event = Some(schema);
                        return Ok(outcome);
                    } else if let Some(wait) = &self.event_wait {
                        outcome.waiting_event = Some(wait.schema.clone());
                        return Ok(outcome);
                    }
                }
                aos_air_types::PlanStepKind::RaiseEvent(raise) => {
                    let value = eval_expr(&raise.event, &self.env).map_err(|err| {
                        KernelError::Manifest(format!("plan raise_event eval error: {err}"))
                    })?;
                    let event = expr_value_to_domain_event(value)?;
                    outcome.raised_events.push(event);
                    self.step_idx += 1;
                }
                aos_air_types::PlanStepKind::End(_) => {
                    self.completed = true;
                    outcome.completed = true;
                    return Ok(outcome);
                }
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
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn waiting_event_schema(&self) -> Option<String> {
        self.event_wait.as_ref().map(|w| w.schema.clone())
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
