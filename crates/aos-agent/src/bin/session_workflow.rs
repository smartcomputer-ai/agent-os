//! Session workflow scaffold (`aos.agent/SessionWorkflow@1`).

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use aos_agent::{
    RunId, SessionLifecycle, SessionLifecycleChanged, SessionState, SessionWorkflowEvent,
    helpers::{SessionEffectCommand, SessionReduceError, apply_session_workflow_event},
};
use aos_wasm_sdk::{ReduceError, Value, Workflow, WorkflowCtx, aos_workflow};

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_workflow!(SessionWorkflow);

#[derive(Default)]
struct SessionWorkflow;

impl Workflow for SessionWorkflow {
    type State = SessionState;
    type Event = SessionWorkflowEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        let prev_lifecycle = ctx.state.lifecycle;
        let prev_run_id = ctx.state.active_run_id.clone();
        let out = apply_session_workflow_event(&mut ctx.state, &event).map_err(map_reduce_error)?;
        for effect in out.effects {
            match effect {
                SessionEffectCommand::LlmGenerate {
                    params, cap_slot, ..
                } => {
                    if let Some(cap_slot) = cap_slot.as_deref() {
                        let mut effects = ctx.effects();
                        effects.sys().llm_generate(&params, cap_slot);
                    } else {
                        ctx.effects().emit_raw("llm.generate", &params, None);
                    }
                }
                SessionEffectCommand::ToolEffect {
                    kind,
                    params_json,
                    cap_slot,
                    ..
                } => {
                    let params: serde_json::Value =
                        serde_json::from_str(&params_json).unwrap_or(serde_json::Value::Null);
                    ctx.effects()
                        .emit_raw(kind.as_str(), &params, cap_slot.as_deref());
                }
                SessionEffectCommand::BlobPut {
                    params, cap_slot, ..
                } => {
                    if let Some(cap_slot) = cap_slot.as_deref() {
                        let mut effects = ctx.effects();
                        effects.sys().blob_put(&params, cap_slot);
                    } else {
                        ctx.effects().emit_raw("blob.put", &params, None);
                    }
                }
                SessionEffectCommand::BlobGet {
                    params, cap_slot, ..
                } => {
                    if let Some(cap_slot) = cap_slot.as_deref() {
                        let mut effects = ctx.effects();
                        effects.sys().blob_get(&params, cap_slot);
                    } else {
                        ctx.effects().emit_raw("blob.get", &params, None);
                    }
                }
            }
        }
        emit_lifecycle_event_if_changed(ctx, prev_lifecycle, prev_run_id);
        Ok(())
    }
}

fn emit_lifecycle_event_if_changed(
    ctx: &mut WorkflowCtx<SessionState, Value>,
    prev_lifecycle: SessionLifecycle,
    prev_run_id: Option<RunId>,
) {
    let Some(payload) = lifecycle_event_payload(ctx, prev_lifecycle, prev_run_id) else {
        return;
    };
    ctx.intent("aos.agent/SessionLifecycleChanged@1")
        .payload(&payload)
        .send();
}

fn lifecycle_event_payload(
    ctx: &WorkflowCtx<SessionState, Value>,
    prev_lifecycle: SessionLifecycle,
    prev_run_id: Option<RunId>,
) -> Option<SessionLifecycleChanged> {
    let observed_at_ns = ctx
        .logical_now_ns()
        .or_else(|| ctx.now_ns())
        .unwrap_or(ctx.state.updated_at);

    lifecycle_event_from_state(&ctx.state, prev_lifecycle, prev_run_id, observed_at_ns)
}

fn lifecycle_event_from_state(
    state: &SessionState,
    prev_lifecycle: SessionLifecycle,
    prev_run_id: Option<RunId>,
    observed_at_ns: u64,
) -> Option<SessionLifecycleChanged> {
    if prev_lifecycle == state.lifecycle {
        return None;
    }
    if state.session_id.0.is_empty() {
        return None;
    }

    Some(SessionLifecycleChanged {
        session_id: state.session_id.clone(),
        observed_at_ns,
        from: prev_lifecycle,
        to: state.lifecycle,
        run_id: state.active_run_id.clone().or(prev_run_id),
        in_flight_effects: state.in_flight_effects,
    })
}

fn map_reduce_error(err: SessionReduceError) -> ReduceError {
    match err {
        SessionReduceError::InvalidLifecycleTransition => {
            ReduceError::new("invalid lifecycle transition")
        }
        SessionReduceError::HostCommandRejected => ReduceError::new("host command rejected"),
        SessionReduceError::ToolBatchAlreadyActive => ReduceError::new("tool batch already active"),
        SessionReduceError::MissingProvider => ReduceError::new("run config provider missing"),
        SessionReduceError::MissingModel => ReduceError::new("run config model missing"),
        SessionReduceError::UnknownProvider => ReduceError::new("run config provider unknown"),
        SessionReduceError::UnknownModel => ReduceError::new("run config model unknown"),
        SessionReduceError::RunAlreadyActive => ReduceError::new("run already active"),
        SessionReduceError::RunNotActive => ReduceError::new("run not active"),
        SessionReduceError::EmptyMessageRefs => {
            ReduceError::new("llm message_refs must not be empty")
        }
        SessionReduceError::TooManyPendingEffects => ReduceError::new("too many pending effects"),
        SessionReduceError::InvalidHashRef => ReduceError::new("invalid hash ref"),
        SessionReduceError::ToolProfileUnknown => ReduceError::new("tool profile unknown"),
        SessionReduceError::UnknownToolOverride => ReduceError::new("unknown tool override"),
        SessionReduceError::InvalidToolRegistry => ReduceError::new("invalid tool registry"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_agent::SessionId;

    #[test]
    fn lifecycle_event_emits_on_transition() {
        let payload = lifecycle_event_from_state(
            &SessionState {
                session_id: SessionId("11111111-1111-1111-1111-111111111111".into()),
                lifecycle: SessionLifecycle::Running,
                active_run_id: Some(RunId {
                    session_id: SessionId("11111111-1111-1111-1111-111111111111".into()),
                    run_seq: 1,
                }),
                in_flight_effects: 3,
                ..SessionState::default()
            },
            SessionLifecycle::Idle,
            None,
            42,
        )
        .expect("lifecycle event payload");
        assert_eq!(payload.observed_at_ns, 42);
        assert_eq!(payload.from, SessionLifecycle::Idle);
        assert_eq!(payload.to, SessionLifecycle::Running);
        assert_eq!(payload.in_flight_effects, 3);
        assert!(payload.run_id.is_some());
    }

    #[test]
    fn lifecycle_event_uses_previous_run_id_when_terminal_clears_active() {
        let session_id = SessionId("11111111-1111-1111-1111-111111111111".into());
        let run_id = RunId {
            session_id: session_id.clone(),
            run_seq: 7,
        };
        let payload = lifecycle_event_from_state(
            &SessionState {
                session_id: session_id.clone(),
                lifecycle: SessionLifecycle::Failed,
                active_run_id: None,
                ..SessionState::default()
            },
            SessionLifecycle::Running,
            Some(run_id.clone()),
            88,
        )
        .expect("lifecycle event payload");
        assert_eq!(payload.to, SessionLifecycle::Failed);
        assert_eq!(payload.run_id, Some(run_id));
    }

    #[test]
    fn lifecycle_event_skips_when_unchanged() {
        let payload = lifecycle_event_from_state(
            &SessionState {
                session_id: SessionId("11111111-1111-1111-1111-111111111111".into()),
                lifecycle: SessionLifecycle::Running,
                ..SessionState::default()
            },
            SessionLifecycle::Running,
            None,
            0,
        );
        assert!(payload.is_none());
    }
}
