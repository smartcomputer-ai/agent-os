use aos_air_exec::eval_expr;

use crate::error::KernelError;

use super::{PlanInstance, StepState};
use super::codec::value_to_bool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepReadiness {
    Ready,
    Blocked,
    Skip,
}

impl PlanInstance {
    pub(super) fn ready_steps(&mut self) -> Result<Vec<String>, KernelError> {
        loop {
            let mut ready = Vec::new();
            let mut skip = Vec::new();
            for id in &self.step_order {
                if matches!(self.step_states[id], StepState::Pending) {
                    match self.step_readiness(id)? {
                        StepReadiness::Ready => ready.push(id.clone()),
                        StepReadiness::Skip => skip.push(id.clone()),
                        StepReadiness::Blocked => {}
                    }
                }
            }

            if !ready.is_empty() {
                return Ok(ready);
            }

            if skip.is_empty() {
                return Ok(Vec::new());
            }

            for id in skip {
                self.mark_skipped(&id);
            }

            // Continue to allow skip propagation through descendants before the caller
            // decides whether the plan is waiting or completed.
        }
    }

    fn step_readiness(&self, step_id: &str) -> Result<StepReadiness, KernelError> {
        let Some(deps) = self.predecessors.get(step_id) else {
            return Ok(StepReadiness::Ready);
        };

        let mut active_dep = false;
        let mut pending_dep = false;
        for dep in deps {
            match self.step_states.get(&dep.pred) {
                Some(StepState::Completed) => {
                    let guard_true = if let Some(expr) = &dep.guard {
                        let value = eval_expr(expr, &self.env)
                            .map_err(|err| KernelError::Manifest(format!("guard eval error: {err}")))?;
                        value_to_bool(value)?
                    } else {
                        true
                    };
                    if guard_true {
                        active_dep = true;
                    }
                }
                Some(StepState::Skipped) => {}
                _ => {
                    pending_dep = true;
                }
            }
        }

        if pending_dep {
            Ok(StepReadiness::Blocked)
        } else if active_dep {
            Ok(StepReadiness::Ready)
        } else {
            Ok(StepReadiness::Skip)
        }
    }

    pub(super) fn mark_completed(&mut self, step_id: &str) {
        self.step_states
            .insert(step_id.to_string(), StepState::Completed);
        self.refresh_completed_flag();
    }

    pub(super) fn mark_skipped(&mut self, step_id: &str) {
        self.step_states
            .insert(step_id.to_string(), StepState::Skipped);
        self.refresh_completed_flag();
    }

    fn refresh_completed_flag(&mut self) {
        if self.all_steps_completed() {
            self.completed = true;
        }
    }

    pub(super) fn all_steps_completed(&self) -> bool {
        self.step_states
            .values()
            .all(|state| matches!(state, StepState::Completed | StepState::Skipped))
    }
}
