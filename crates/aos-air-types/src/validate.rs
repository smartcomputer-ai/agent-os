use std::collections::{HashMap, HashSet};

use petgraph::{algo::is_cyclic_directed, graphmap::DiGraphMap};
use thiserror::Error;

use crate::{CapGrantName, DefPlan, EffectKind, Expr, PlanStepKind, StepId};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("plan {plan} must contain at least one step")]
    EmptyPlan { plan: String },
    #[error("plan {plan} has duplicate step id {step_id}")]
    DuplicateStepId { plan: String, step_id: StepId },
    #[error("plan {plan} has duplicate edge {from} -> {to}")]
    DuplicateEdge {
        plan: String,
        from: StepId,
        to: StepId,
    },
    #[error("plan {plan} edge references unknown step id {step_id}")]
    EdgeReferencesUnknownStep { plan: String, step_id: StepId },
    #[error("plan {plan} contains cycles")]
    CyclicPlan { plan: String },
    #[error(
        "plan {plan} declared required_caps {declared:?} but derived {derived:?} from emit_effect steps"
    )]
    DeclaredCapsMismatch {
        plan: String,
        declared: Vec<CapGrantName>,
        derived: Vec<CapGrantName>,
    },
    #[error(
        "plan {plan} declared allowed_effects {declared:?} but derived {derived:?} from emit_effect steps"
    )]
    DeclaredEffectsMismatch {
        plan: String,
        declared: Vec<EffectKind>,
        derived: Vec<EffectKind>,
    },
    #[error("plan {plan} declares duplicate effect handle '{handle}'")]
    DuplicateEffectHandle { plan: String, handle: String },
    #[error("plan {plan} step {step_id} await_receipt 'for' must be an @var:handle reference")]
    AwaitReceiptInvalidReference { plan: String, step_id: StepId },
    #[error("plan {plan} step {step_id} awaits receipt for unknown handle {handle}")]
    AwaitReceiptUnknownHandle {
        plan: String,
        step_id: StepId,
        handle: String,
    },
    #[error(
        "plan {plan} await_event step {step_id} where clause references unknown symbol {reference}"
    )]
    AwaitEventUnknownReference {
        plan: String,
        step_id: StepId,
        reference: String,
    },
    #[error("plan {plan} invariant {index} references unknown symbol {reference}")]
    InvariantUnknownReference {
        plan: String,
        index: usize,
        reference: String,
    },
    #[error("plan {plan} invariant {index} may not reference @event")]
    InvariantEventReference { plan: String, index: usize },
}

fn sort_and_dedup_caps(mut caps: Vec<CapGrantName>) -> Vec<CapGrantName> {
    caps.sort();
    caps.dedup();
    caps
}

fn sort_and_dedup_effects(mut effects: Vec<EffectKind>) -> Vec<EffectKind> {
    effects.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    effects.dedup();
    effects
}

pub fn derive_plan_caps_and_effects(plan: &DefPlan) -> (Vec<CapGrantName>, Vec<EffectKind>) {
    let mut caps: HashSet<CapGrantName> = HashSet::new();
    let mut effects: HashSet<EffectKind> = HashSet::new();

    for step in &plan.steps {
        if let PlanStepKind::EmitEffect(emit) = &step.kind {
            caps.insert(emit.cap.clone());
            effects.insert(emit.kind.clone());
        }
    }

    let mut caps: Vec<_> = caps.into_iter().collect();
    let mut effects: Vec<_> = effects.into_iter().collect();
    caps.sort();
    effects.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    (caps, effects)
}

/// Fill derived caps/effects when the author omits them and canonicalize order when provided.
pub fn normalize_plan_caps_and_effects(plan: &mut DefPlan) {
    let (derived_caps, derived_effects) = derive_plan_caps_and_effects(plan);

    if plan.required_caps.is_empty() {
        plan.required_caps = derived_caps.clone();
    } else {
        plan.required_caps = sort_and_dedup_caps(plan.required_caps.clone());
    }

    if plan.allowed_effects.is_empty() {
        plan.allowed_effects = derived_effects.clone();
    } else {
        plan.allowed_effects = sort_and_dedup_effects(plan.allowed_effects.clone());
    }
}

pub fn validate_plan(plan: &DefPlan) -> Result<(), ValidationError> {
    if plan.steps.is_empty() {
        return Err(ValidationError::EmptyPlan {
            plan: plan.name.clone(),
        });
    }

    let mut step_ids = HashSet::new();
    for step in &plan.steps {
        if !step_ids.insert(step.id.as_str()) {
            return Err(ValidationError::DuplicateStepId {
                plan: plan.name.clone(),
                step_id: step.id.clone(),
            });
        }
    }

    let mut graph = DiGraphMap::<&str, ()>::new();
    for step in &plan.steps {
        graph.add_node(step.id.as_str());
    }
    let mut edges = HashSet::new();
    for edge in &plan.edges {
        if !step_ids.contains(edge.from.as_str()) {
            return Err(ValidationError::EdgeReferencesUnknownStep {
                plan: plan.name.clone(),
                step_id: edge.from.clone(),
            });
        }
        if !step_ids.contains(edge.to.as_str()) {
            return Err(ValidationError::EdgeReferencesUnknownStep {
                plan: plan.name.clone(),
                step_id: edge.to.clone(),
            });
        }
        let key = (edge.from.clone(), edge.to.clone());
        if !edges.insert(key.clone()) {
            return Err(ValidationError::DuplicateEdge {
                plan: plan.name.clone(),
                from: key.0,
                to: key.1,
            });
        }
        graph.add_edge(edge.from.as_str(), edge.to.as_str(), ());
    }
    if is_cyclic_directed(&graph) {
        return Err(ValidationError::CyclicPlan {
            plan: plan.name.clone(),
        });
    }

    let (derived_caps, derived_effects) = derive_plan_caps_and_effects(plan);

    let declared_caps = sort_and_dedup_caps(plan.required_caps.clone());
    if !plan.required_caps.is_empty() && declared_caps != derived_caps {
        return Err(ValidationError::DeclaredCapsMismatch {
            plan: plan.name.clone(),
            declared: declared_caps,
            derived: derived_caps,
        });
    }

    let declared_effects = sort_and_dedup_effects(plan.allowed_effects.clone());
    if !plan.allowed_effects.is_empty() && declared_effects != derived_effects {
        return Err(ValidationError::DeclaredEffectsMismatch {
            plan: plan.name.clone(),
            declared: declared_effects,
            derived: derived_effects,
        });
    }

    // "correlation_id" is injected by the kernel when a plan is started via a trigger
    // that specifies `correlate_by`. Allow expressions to reference it even though the
    // value is only present at runtime when correlation is configured.
    let mut available_vars: HashSet<String> = plan.locals.keys().cloned().collect();
    available_vars.insert("correlation_id".into());
    let mut effect_handles: HashSet<String> = HashSet::new();
    for step in &plan.steps {
        match &step.kind {
            PlanStepKind::EmitEffect(emit) => {
                if !effect_handles.insert(emit.bind.effect_id_as.clone()) {
                    return Err(ValidationError::DuplicateEffectHandle {
                        plan: plan.name.clone(),
                        handle: emit.bind.effect_id_as.clone(),
                    });
                }
                available_vars.insert(emit.bind.effect_id_as.clone());
            }
            PlanStepKind::Assign(assign) => {
                available_vars.insert(assign.bind.var.clone());
            }
            PlanStepKind::AwaitReceipt(await_step) => {
                validate_await_receipt(plan, &step.id, await_step, &effect_handles)?;
                available_vars.insert(await_step.bind.var.clone());
            }
            PlanStepKind::AwaitEvent(await_step) => {
                if let Some(predicate) = &await_step.where_clause {
                    validate_where_clause(plan, &step.id, predicate, &available_vars, &step_ids)?;
                }
                available_vars.insert(await_step.bind.var.clone());
            }
            _ => {}
        }
    }

    let declared_vars = available_vars;
    validate_invariants(plan, &declared_vars, &step_ids)?;

    Ok(())
}

pub fn validate_plans(plans: &[DefPlan]) -> HashMap<String, Result<(), ValidationError>> {
    plans
        .iter()
        .map(|plan| (plan.name.clone(), validate_plan(plan)))
        .collect()
}

fn validate_await_receipt(
    plan: &DefPlan,
    step_id: &StepId,
    await_step: &crate::PlanStepAwaitReceipt,
    effect_handles: &HashSet<String>,
) -> Result<(), ValidationError> {
    let handle = extract_handle_reference(&await_step.for_expr).ok_or_else(|| {
        ValidationError::AwaitReceiptInvalidReference {
            plan: plan.name.clone(),
            step_id: step_id.clone(),
        }
    })?;

    if !effect_handles.contains(&handle) {
        return Err(ValidationError::AwaitReceiptUnknownHandle {
            plan: plan.name.clone(),
            step_id: step_id.clone(),
            handle,
        });
    }

    Ok(())
}

fn validate_where_clause(
    plan: &DefPlan,
    step_id: &StepId,
    predicate: &Expr,
    available_vars: &HashSet<String>,
    step_ids: &HashSet<&str>,
) -> Result<(), ValidationError> {
    let mut refs = Vec::new();
    collect_expr_refs(predicate, &mut refs);
    for reference in refs {
        match classify_reference(&reference) {
            ReferenceKind::PlanInput => {}
            ReferenceKind::Var(name) => {
                if !available_vars.contains(&name) {
                    return Err(ValidationError::AwaitEventUnknownReference {
                        plan: plan.name.clone(),
                        step_id: step_id.clone(),
                        reference,
                    });
                }
            }
            ReferenceKind::Step(name) => {
                if !step_ids.contains(name.as_str()) {
                    return Err(ValidationError::AwaitEventUnknownReference {
                        plan: plan.name.clone(),
                        step_id: step_id.clone(),
                        reference,
                    });
                }
            }
            ReferenceKind::Event => {}
            ReferenceKind::Unknown => {
                return Err(ValidationError::AwaitEventUnknownReference {
                    plan: plan.name.clone(),
                    step_id: step_id.clone(),
                    reference,
                });
            }
        }
    }
    Ok(())
}

fn validate_invariants(
    plan: &DefPlan,
    declared_vars: &HashSet<String>,
    step_ids: &HashSet<&str>,
) -> Result<(), ValidationError> {
    for (index, invariant) in plan.invariants.iter().enumerate() {
        let mut refs = Vec::new();
        collect_expr_refs(invariant, &mut refs);
        for reference in refs {
            match classify_reference(&reference) {
                ReferenceKind::PlanInput => {}
                ReferenceKind::Var(name) => {
                    if !declared_vars.contains(&name) {
                        return Err(ValidationError::InvariantUnknownReference {
                            plan: plan.name.clone(),
                            index,
                            reference,
                        });
                    }
                }
                ReferenceKind::Step(name) => {
                    if !step_ids.contains(name.as_str()) {
                        return Err(ValidationError::InvariantUnknownReference {
                            plan: plan.name.clone(),
                            index,
                            reference,
                        });
                    }
                }
                ReferenceKind::Event => {
                    return Err(ValidationError::InvariantEventReference {
                        plan: plan.name.clone(),
                        index,
                    });
                }
                ReferenceKind::Unknown => {
                    return Err(ValidationError::InvariantUnknownReference {
                        plan: plan.name.clone(),
                        index,
                        reference,
                    });
                }
            }
        }
    }
    Ok(())
}

fn extract_handle_reference(expr: &Expr) -> Option<String> {
    if let Expr::Ref(r) = expr {
        if let Some(rest) = r.reference.strip_prefix("@var:") {
            let mut parts = rest.split('.');
            let name = parts.next().unwrap_or(rest);
            if parts.next().is_none() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn collect_expr_refs(expr: &Expr, refs: &mut Vec<String>) {
    match expr {
        Expr::Ref(reference) => refs.push(reference.reference.clone()),
        Expr::Const(_) => {}
        Expr::Op(op) => {
            for arg in &op.args {
                collect_expr_refs(arg, refs);
            }
        }
        Expr::Record(record) => {
            for value in record.record.values() {
                collect_expr_refs(value, refs);
            }
        }
        Expr::List(list) => {
            for value in &list.list {
                collect_expr_refs(value, refs);
            }
        }
        Expr::Set(set) => {
            for value in &set.set {
                collect_expr_refs(value, refs);
            }
        }
        Expr::Map(map) => {
            for entry in &map.map {
                collect_expr_refs(&entry.key, refs);
                collect_expr_refs(&entry.value, refs);
            }
        }
        Expr::Variant(variant) => {
            if let Some(value) = &variant.variant.value {
                collect_expr_refs(value, refs);
            }
        }
    }
}

enum ReferenceKind {
    PlanInput,
    Var(String),
    Step(String),
    Event,
    Unknown,
}

fn classify_reference(reference: &str) -> ReferenceKind {
    if reference.starts_with("@plan.input") {
        ReferenceKind::PlanInput
    } else if let Some(rest) = reference.strip_prefix("@var:") {
        let name = rest.split('.').next().unwrap_or(rest).to_string();
        ReferenceKind::Var(name)
    } else if let Some(rest) = reference.strip_prefix("@step:") {
        let name = rest.split('.').next().unwrap_or(rest).to_string();
        ReferenceKind::Step(name)
    } else if reference.starts_with("@event") {
        ReferenceKind::Event
    } else {
        ReferenceKind::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DefPlan, EffectKind, Expr, ExprConst, ExprRecord, ExprRef, PlanBind, PlanBindEffect,
        PlanEdge, PlanStep, PlanStepAwaitEvent, PlanStepAwaitReceipt, PlanStepEmitEffect,
        PlanStepEnd, PlanStepKind, SchemaRef,
    };
    use indexmap::IndexMap;

    fn sample_plan() -> DefPlan {
        let emit = PlanStep {
            id: "emit".into(),
            kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                kind: EffectKind::http_request(),
                params: Expr::Record(ExprRecord {
                    record: IndexMap::new(),
                })
                .into(),
                cap: "http_cap".into(),
                bind: PlanBindEffect {
                    effect_id_as: "req".into(),
                },
            }),
        };
        let end = PlanStep {
            id: "end".into(),
            kind: PlanStepKind::End(PlanStepEnd { result: None }),
        };
        DefPlan {
            name: "com.acme/plan@1".into(),
            input: SchemaRef::new("com.acme/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![emit, end],
            edges: vec![PlanEdge {
                from: "emit".into(),
                to: "end".into(),
                when: None,
            }],
            required_caps: vec!["http_cap".into()],
            allowed_effects: vec![EffectKind::http_request()],
            invariants: vec![],
        }
    }

    fn plan_with_emit_and_await() -> DefPlan {
        let mut plan = sample_plan();
        let await_step = PlanStep {
            id: "await".into(),
            kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                for_expr: Expr::Ref(ExprRef {
                    reference: "@var:req".into(),
                }),
                bind: PlanBind { var: "rcpt".into() },
            }),
        };
        plan.steps.insert(1, await_step);
        plan.edges = vec![
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
        ];
        plan
    }

    fn plan_with_await_event() -> DefPlan {
        DefPlan {
            name: "com.acme/event-plan@1".into(),
            input: SchemaRef::new("com.acme/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![
                PlanStep {
                    id: "await".into(),
                    kind: PlanStepKind::AwaitEvent(PlanStepAwaitEvent {
                        event: SchemaRef::new("com.acme/Trigger@1").unwrap(),
                        where_clause: None,
                        bind: PlanBind { var: "evt".into() },
                    }),
                },
                PlanStep {
                    id: "end".into(),
                    kind: PlanStepKind::End(PlanStepEnd { result: None }),
                },
            ],
            edges: vec![PlanEdge {
                from: "await".into(),
                to: "end".into(),
                when: None,
            }],
            required_caps: vec![],
            allowed_effects: vec![],
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
        assert!(
            matches!(err, ValidationError::EdgeReferencesUnknownStep { step_id, .. } if step_id == "unknown")
        );
    }

    #[test]
    fn cycle_detected() {
        let mut plan = sample_plan();
        plan.edges.push(PlanEdge {
            from: "end".into(),
            to: "emit".into(),
            when: None,
        });
        let err = validate_plan(&plan).unwrap_err();
        assert!(matches!(err, ValidationError::CyclicPlan { .. }));
    }

    #[test]
    fn disallowed_effect_detected() {
        let mut plan = sample_plan();
        plan.allowed_effects = vec![EffectKind::timer_set()];
        let err = validate_plan(&plan).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::DeclaredEffectsMismatch { declared, derived, .. }
            if declared == vec![EffectKind::timer_set()] && derived == vec![EffectKind::http_request()]
        ));
    }

    #[test]
    fn missing_cap_detected() {
        let mut plan = sample_plan();
        plan.required_caps = vec!["other".into()];
        let err = validate_plan(&plan).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::DeclaredCapsMismatch { declared, derived, .. }
            if declared == vec!["other".to_string()] && derived == vec!["http_cap".to_string()]
        ));
    }

    #[test]
    fn duplicate_effect_handle_detected() {
        let mut plan = sample_plan();
        let duplicate_emit = PlanStep {
            id: "emit2".into(),
            kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                kind: EffectKind::http_request(),
                params: Expr::Record(ExprRecord {
                    record: IndexMap::new(),
                })
                .into(),
                cap: "http_cap".into(),
                bind: PlanBindEffect {
                    effect_id_as: "req".into(),
                },
            }),
        };
        plan.steps.insert(0, duplicate_emit);
        let err = validate_plan(&plan).unwrap_err();
        assert!(matches!(err, ValidationError::DuplicateEffectHandle { .. }));
    }

    #[test]
    fn omitted_caps_and_effects_are_derived() {
        let mut plan = sample_plan();
        plan.required_caps.clear();
        plan.allowed_effects.clear();
        assert!(validate_plan(&plan).is_ok());
    }

    #[test]
    fn normalize_plan_caps_and_effects_populates_and_sorts() {
        let mut plan = sample_plan();
        // Introduce disorder and duplicates.
        plan.required_caps = vec!["b".into(), "a".into(), "a".into()];
        plan.allowed_effects = vec![
            EffectKind::timer_set(),
            EffectKind::http_request(),
            EffectKind::http_request(),
        ];

        normalize_plan_caps_and_effects(&mut plan);

        assert_eq!(plan.required_caps, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            plan.allowed_effects,
            vec![EffectKind::http_request(), EffectKind::timer_set()]
        );
    }

    #[test]
    fn await_receipt_requires_handle_reference() {
        let mut plan = plan_with_emit_and_await();
        if let PlanStepKind::AwaitReceipt(step) = &mut plan.steps[1].kind {
            step.for_expr = Expr::Const(ExprConst::Bool { bool: true });
        }
        let err = validate_plan(&plan).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::AwaitReceiptInvalidReference { .. }
        ));
    }

    #[test]
    fn await_receipt_rejects_unknown_handle() {
        let mut plan = plan_with_emit_and_await();
        if let PlanStepKind::AwaitReceipt(step) = &mut plan.steps[1].kind {
            step.for_expr = Expr::Ref(ExprRef {
                reference: "@var:missing".into(),
            });
        }
        let err = validate_plan(&plan).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::AwaitReceiptUnknownHandle { .. }
        ));
    }

    #[test]
    fn await_event_where_rejects_unknown_reference() {
        let mut plan = plan_with_await_event();
        if let PlanStepKind::AwaitEvent(step) = &mut plan.steps[0].kind {
            step.where_clause = Some(Expr::Ref(ExprRef {
                reference: "@var:missing".into(),
            }));
        }
        let err = validate_plan(&plan).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::AwaitEventUnknownReference { .. }
        ));
    }

    #[test]
    fn invariant_unknown_reference_fails() {
        let mut plan = sample_plan();
        plan.invariants.push(Expr::Ref(ExprRef {
            reference: "@var:unknown".into(),
        }));
        let err = validate_plan(&plan).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::InvariantUnknownReference { .. }
        ));
    }

    #[test]
    fn invariant_event_reference_fails() {
        let mut plan = sample_plan();
        plan.invariants.push(Expr::Ref(ExprRef {
            reference: "@event.value".into(),
        }));
        let err = validate_plan(&plan).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::InvariantEventReference { .. }
        ));
    }
}
