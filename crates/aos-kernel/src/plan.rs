use std::collections::HashMap;

use aos_air_exec::{Env as ExprEnv, Value as ExprValue, eval_expr};
use aos_air_types::{DefPlan, PlanStepKind};
use indexmap::IndexMap;

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
        }
    }

    pub fn tick(&mut self, effects: &mut EffectManager) -> Result<bool, KernelError> {
        if self.completed {
            return Ok(true);
        }
        if self.step_idx >= self.plan.steps.len() {
            self.completed = true;
            return Ok(true);
        }
        let step = &self.plan.steps[self.step_idx];
        match &step.kind {
            PlanStepKind::Assign(assign) => {
                let value = eval_expr(&assign.expr, &self.env).map_err(|err| {
                    KernelError::Manifest(format!("plan assign eval error: {err}"))
                })?;
                self.env.vars.insert(assign.bind.var.clone(), value);
            }
            PlanStepKind::EmitEffect(emit) => {
                let value = eval_expr(&emit.params, &self.env).map_err(|err| {
                    KernelError::Manifest(format!("plan effect eval error: {err}"))
                })?;
                let params_cbor = serde_cbor::to_vec(&value)
                    .map_err(|err| KernelError::Manifest(err.to_string()))?;
                effects.enqueue_plan_effect(&emit.kind, &emit.cap, params_cbor)?;
                self.env.vars.insert(
                    emit.bind.effect_id_as.clone(),
                    ExprValue::Text("effect-id".into()),
                );
            }
            PlanStepKind::End(_) => {
                self.completed = true;
            }
            _ => {
                return Err(KernelError::Manifest(format!(
                    "unsupported plan step in v0 kernel: {:?}",
                    step.kind
                )));
            }
        }
        self.step_idx += 1;
        if self.step_idx >= self.plan.steps.len() {
            self.completed = true;
        }
        Ok(self.completed)
    }
}
