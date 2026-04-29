use crate::contracts::{
    ContextAction, ContextActionKind, ContextBudget, ContextInput, ContextInputKind,
    ContextInputScope, ContextPlan, ContextPriority, ContextReport, ContextSelection, RunCause,
    RunId, SessionContextState, SessionId,
};
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Clone)]
pub struct ContextRequest<'a> {
    pub session_id: &'a SessionId,
    pub run_id: &'a RunId,
    pub run_cause: Option<&'a RunCause>,
    pub budget: ContextBudget,
    pub session_context: &'a SessionContextState,
    pub prompt_refs: &'a [String],
    pub transcript_refs: &'a [String],
    pub turn_refs: &'a [String],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextError {
    EmptySelection,
}

pub trait ContextEngine {
    fn build_plan(&self, request: ContextRequest<'_>) -> Result<ContextPlan, ContextError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultContextEngine;

impl ContextEngine for DefaultContextEngine {
    fn build_plan(&self, request: ContextRequest<'_>) -> Result<ContextPlan, ContextError> {
        build_default_context_plan(request)
    }
}

pub fn build_default_context_plan(
    request: ContextRequest<'_>,
) -> Result<ContextPlan, ContextError> {
    let mut inputs = Vec::new();

    for (idx, value) in request.prompt_refs.iter().enumerate() {
        inputs.push(context_input(
            format!("prompt:{idx}"),
            ContextInputKind::PromptRef,
            ContextInputScope::Session,
            ContextPriority::Required,
            value.clone(),
            Some("prompt".into()),
        ));
    }

    for input in &request.session_context.pinned_inputs {
        inputs.push(input.clone());
    }

    for (idx, value) in request.session_context.summary_refs.iter().enumerate() {
        inputs.push(context_input(
            format!("summary:{idx}"),
            ContextInputKind::SummaryRef,
            ContextInputScope::Summary,
            ContextPriority::High,
            value.clone(),
            Some("summary".into()),
        ));
    }

    if let Some(cause) = request.run_cause {
        if let Some(payload_ref) = cause.payload_ref.as_ref() {
            inputs.push(context_input(
                "cause:payload".into(),
                ContextInputKind::DomainRef,
                ContextInputScope::Cause,
                ContextPriority::High,
                payload_ref.clone(),
                cause.payload_schema.clone(),
            ));
        }
        for (idx, subject) in cause.subject_refs.iter().enumerate() {
            if let Some(value) = subject.ref_.as_ref() {
                inputs.push(context_input(
                    format!("cause:subject:{idx}"),
                    ContextInputKind::DomainRef,
                    ContextInputScope::Cause,
                    ContextPriority::High,
                    value.clone(),
                    Some(subject.kind.clone()),
                ));
            }
        }
    }

    for (idx, value) in request.transcript_refs.iter().enumerate() {
        inputs.push(context_input(
            format!("transcript:{idx}"),
            ContextInputKind::MessageRef,
            ContextInputScope::Transcript,
            ContextPriority::Normal,
            value.clone(),
            Some("transcript".into()),
        ));
    }

    for (idx, value) in request.turn_refs.iter().enumerate() {
        inputs.push(context_input(
            format!("turn:{idx}"),
            ContextInputKind::MessageRef,
            ContextInputScope::Run,
            ContextPriority::Required,
            value.clone(),
            Some("turn".into()),
        ));
    }

    select_inputs(request, inputs)
}

fn context_input(
    input_id: String,
    kind: ContextInputKind,
    scope: ContextInputScope,
    priority: ContextPriority,
    content_ref: String,
    label: Option<String>,
) -> ContextInput {
    ContextInput {
        input_id,
        kind,
        scope,
        priority,
        content_ref,
        label,
        source_kind: None,
        source_id: None,
        correlation_id: None,
    }
}

fn select_inputs(
    request: ContextRequest<'_>,
    inputs: Vec<ContextInput>,
) -> Result<ContextPlan, ContextError> {
    let max_refs = request.budget.max_refs.unwrap_or(u64::MAX) as usize;
    let mut selected_refs = Vec::new();
    let mut selections = Vec::new();
    let mut seen = BTreeSet::new();
    let mut selected_count = 0_u64;
    let mut dropped_count = 0_u64;

    for input in inputs {
        if !seen.insert(input.content_ref.clone()) {
            dropped_count = dropped_count.saturating_add(1);
            selections.push(ContextSelection {
                input_id: input.input_id,
                selected: false,
                reason: "duplicate_ref".into(),
                content_ref: input.content_ref,
            });
            continue;
        }
        if selected_refs.len() >= max_refs && !matches!(input.priority, ContextPriority::Required) {
            dropped_count = dropped_count.saturating_add(1);
            selections.push(ContextSelection {
                input_id: input.input_id,
                selected: false,
                reason: "budget_exhausted".into(),
                content_ref: input.content_ref,
            });
            continue;
        }
        selected_count = selected_count.saturating_add(1);
        selected_refs.push(input.content_ref.clone());
        selections.push(ContextSelection {
            input_id: input.input_id,
            selected: true,
            reason: "selected".into(),
            content_ref: input.content_ref,
        });
    }

    if selected_refs.is_empty() {
        return Err(ContextError::EmptySelection);
    }

    let compaction_recommended = request
        .budget
        .max_refs
        .is_some_and(|max| dropped_count > 0 && max > 0);
    let actions = if compaction_recommended {
        vec![ContextAction {
            action_id: "context:compact:recommended".into(),
            kind: ContextActionKind::Compact,
            reason: "context budget dropped inputs".into(),
            required: false,
            input_ids: selections
                .iter()
                .filter(|selection| !selection.selected)
                .map(|selection| selection.input_id.clone())
                .collect(),
        }]
    } else {
        Vec::new()
    };

    Ok(ContextPlan {
        selected_refs,
        selections,
        actions,
        report: ContextReport {
            engine: "aos.agent/default".into(),
            selected_count,
            dropped_count,
            budget: request.budget,
            decisions: vec![format!(
                "selected context for session={} run={}",
                request.session_id.0, request.run_id.run_seq
            )],
            unresolved: Vec::new(),
            compaction_recommended,
            compaction_required: false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{CauseRef, RunCause, RunCauseOrigin};
    use alloc::vec;

    fn hash(seed: char) -> String {
        let mut value = String::from("sha256:");
        let nibble = b"0123456789abcdef"[seed as usize % 16] as char;
        for _ in 0..64 {
            value.push(nibble);
        }
        value
    }

    #[test]
    fn default_engine_preserves_prompt_ref_order_before_turn_refs() {
        let session = SessionId("s-1".into());
        let run_id = RunId {
            session_id: session.clone(),
            run_seq: 1,
        };
        let context_state = SessionContextState::default();
        let prompt = vec![hash('a'), hash('b')];
        let transcript = vec![hash('c')];
        let turn = vec![hash('d')];

        let plan = build_default_context_plan(ContextRequest {
            session_id: &session,
            run_id: &run_id,
            run_cause: None,
            budget: ContextBudget::default(),
            session_context: &context_state,
            prompt_refs: &prompt,
            transcript_refs: &transcript,
            turn_refs: &turn,
        })
        .expect("plan");

        assert_eq!(
            plan.selected_refs,
            vec![hash('a'), hash('b'), hash('c'), hash('d')]
        );
        assert_eq!(plan.report.selected_count, 4);
        assert_eq!(plan.report.dropped_count, 0);
    }

    #[test]
    fn default_engine_accepts_domain_cause_refs_without_product_variants() {
        let input = hash('a');
        let payload = hash('b');
        let subject = hash('c');
        let cause = RunCause {
            kind: "example/work_item_ready".into(),
            origin: RunCauseOrigin::DomainEvent {
                schema: "example/WorkItemReady@1".into(),
                event_ref: Some(payload.clone()),
                key: Some("work-1".into()),
            },
            input_refs: vec![input.clone()],
            payload_schema: Some("example/WorkItemReady@1".into()),
            payload_ref: Some(payload.clone()),
            subject_refs: vec![CauseRef {
                kind: "example/work_item".into(),
                id: "work-1".into(),
                ref_: Some(subject.clone()),
            }],
        };
        let session = SessionId("s-1".into());
        let run_id = RunId {
            session_id: session.clone(),
            run_seq: 1,
        };
        let context_state = SessionContextState::default();
        let turn = vec![input.clone()];

        let plan = build_default_context_plan(ContextRequest {
            session_id: &session,
            run_id: &run_id,
            run_cause: Some(&cause),
            budget: ContextBudget::default(),
            session_context: &context_state,
            prompt_refs: &[],
            transcript_refs: &[],
            turn_refs: &turn,
        })
        .expect("plan");

        assert_eq!(plan.selected_refs, vec![payload, subject, input]);
    }

    #[test]
    fn default_engine_reports_budget_drops_and_recommends_compaction() {
        let session = SessionId("s-1".into());
        let run_id = RunId {
            session_id: session.clone(),
            run_seq: 1,
        };
        let context_state = SessionContextState::default();
        let transcript = vec![hash('a'), hash('b'), hash('c')];

        let plan = build_default_context_plan(ContextRequest {
            session_id: &session,
            run_id: &run_id,
            run_cause: None,
            budget: ContextBudget {
                max_refs: Some(2),
                reserve_output_tokens: None,
            },
            session_context: &context_state,
            prompt_refs: &[],
            transcript_refs: &transcript,
            turn_refs: &[],
        })
        .expect("plan");

        assert_eq!(plan.selected_refs, vec![hash('a'), hash('b')]);
        assert_eq!(plan.report.dropped_count, 1);
        assert!(plan.report.compaction_recommended);
        assert_eq!(plan.actions.len(), 1);
    }
}
