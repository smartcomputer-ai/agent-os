use std::collections::{HashMap, HashSet};

use petgraph::{algo::is_cyclic_directed, graphmap::DiGraphMap};
use thiserror::Error;

use crate::{CapGrantName, DefPlan, EffectKind, PlanStepKind, StepId};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("plan {plan} must contain at least one step")]
    EmptyPlan { plan: String },
    #[error("plan {plan} has duplicate step id {step_id}")]
    DuplicateStepId { plan: String, step_id: StepId },
    #[error("plan {plan} edge references unknown step id {step_id}")]
    EdgeReferencesUnknownStep { plan: String, step_id: StepId },
    #[error("plan {plan} contains cycles")]
    CyclicPlan { plan: String },
    #[error("plan {plan} step {step_id} emits effect {kind:?} which is not in allowed_effects")]
    EffectNotAllowed { plan: String, step_id: StepId, kind: EffectKind },
    #[error("plan {plan} step {step_id} uses cap {cap} which is not listed in required_caps")]
    CapNotDeclared { plan: String, step_id: StepId, cap: CapGrantName },
}

pub fn validate_plan(plan: &DefPlan) -> Result<(), ValidationError> {
    if plan.steps.is_empty() {
        return Err(ValidationError::EmptyPlan { plan: plan.name.clone() });
    }

    let mut step_ids = HashSet::new();
    for step in &plan.steps {
        if !step_ids.insert(&step.id) {
            return Err(ValidationError::DuplicateStepId { plan: plan.name.clone(), step_id: step.id.clone() });
        }
    }

    let mut graph = DiGraphMap::<&str, ()>::new();
    for step in &plan.steps {
        graph.add_node(step.id.as_str());
    }
    for edge in &plan.edges {
        if !step_ids.contains(&edge.from) {
            return Err(ValidationError::EdgeReferencesUnknownStep { plan: plan.name.clone(), step_id: edge.from.clone() });
        }
        if !step_ids.contains(&edge.to) {
            return Err(ValidationError::EdgeReferencesUnknownStep { plan: plan.name.clone(), step_id: edge.to.clone() });
        }
        graph.add_edge(edge.from.as_str(), edge.to.as_str(), ());
    }
    if is_cyclic_directed(&graph) {
        return Err(ValidationError::CyclicPlan { plan: plan.name.clone() });
    }

    let allowed_effects: HashSet<_> = plan.allowed_effects.iter().collect();
    let required_caps: HashSet<_> = plan.required_caps.iter().collect();

    for step in &plan.steps {
        if let PlanStepKind::EmitEffect(ref emit) = step.kind {
            if !allowed_effects.is_empty() && !allowed_effects.contains(&emit.kind) {
                return Err(ValidationError::EffectNotAllowed {
                    plan: plan.name.clone(),
                    step_id: step.id.clone(),
                    kind: emit.kind.clone(),
                });
            }
            if !required_caps.is_empty() && !required_caps.contains(&emit.cap) {
                return Err(ValidationError::CapNotDeclared {
                    plan: plan.name.clone(),
                    step_id: step.id.clone(),
                    cap: emit.cap.clone(),
                });
            }
        }
    }

    Ok(())
}

pub fn validate_plans(plans: &[DefPlan]) -> HashMap<String, Result<(), ValidationError>> {
    plans
        .iter()
        .map(|plan| (plan.name.clone(), validate_plan(plan)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DefPlan, EffectKind, Expr, ExprRecord, PlanBindEffect, PlanEdge, PlanStep, PlanStepEmitEffect, PlanStepEnd, PlanStepKind};
    use indexmap::IndexMap;

    fn sample_plan() -> DefPlan {
        let emit = PlanStep {
            id: "emit".into(),
            kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                kind: EffectKind::HttpRequest,
                params: Expr::Record(ExprRecord { record: IndexMap::new() }),
                cap: "http_cap".into(),
                bind: PlanBindEffect { effect_id_as: "req".into() },
            }),
        };
        let end = PlanStep {
            id: "end".into(),
            kind: PlanStepKind::End(PlanStepEnd { result: None }),
        };
        DefPlan {
            name: "com.acme/plan@1".into(),
            input: "com.acme/Input@1".into(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![emit, end],
            edges: vec![PlanEdge { from: "emit".into(), to: "end".into(), when: None }],
            required_caps: vec!["http_cap".into()],
            allowed_effects: vec![EffectKind::HttpRequest],
            invariants: vec![],
        }
    }

    #[test]
    fn valid_plan_passes() {
        let plan = sample_plan();
        assert!(validate_plan(&plan).is_ok());
    }

    #[test]
    fn duplicate_step_id_fails() {
        let mut plan = sample_plan();
        plan.steps[1].id = "emit".into();
        let err = validate_plan(&plan).unwrap_err();
        assert!(matches!(err, ValidationError::DuplicateStepId { .. }));
    }

    #[test]
    fn edge_missing_step_fails() {
        let mut plan = sample_plan();
        plan.edges[0].to = "unknown".into();
        let err = validate_plan(&plan).unwrap_err();
        assert!(matches!(err, ValidationError::EdgeReferencesUnknownStep { step_id, .. } if step_id == "unknown"));
    }

    #[test]
    fn cycle_detected() {
        let mut plan = sample_plan();
        plan.edges.push(PlanEdge { from: "end".into(), to: "emit".into(), when: None });
        let err = validate_plan(&plan).unwrap_err();
        assert!(matches!(err, ValidationError::CyclicPlan { .. }));
    }

    #[test]
    fn disallowed_effect_detected() {
        let mut plan = sample_plan();
        plan.allowed_effects = vec![EffectKind::TimerSet];
        let err = validate_plan(&plan).unwrap_err();
        assert!(matches!(err, ValidationError::EffectNotAllowed { kind, .. } if kind == EffectKind::HttpRequest));
    }

    #[test]
    fn missing_cap_detected() {
        let mut plan = sample_plan();
        plan.required_caps = vec!["other".into()];
        let err = validate_plan(&plan).unwrap_err();
        assert!(matches!(err, ValidationError::CapNotDeclared { cap, .. } if cap == "http_cap"));
    }
}
