use super::{
    allocate_run_id, allocate_step_id, allocate_turn_id, apply_cancel_fence,
    can_apply_host_command, enqueue_host_text, pop_follow_up_if_ready, pop_pending_steer,
    transition_lifecycle,
};
use crate::contracts::{
    ActiveToolBatch, HostCommandKind, RunConfig, SessionConfig, SessionEvent, SessionEventKind,
    SessionLifecycle, SessionState, ToolCallStatus,
};
use alloc::collections::{BTreeMap, BTreeSet};

/// Extension hooks for SDK-based session reducers.
pub trait SessionReducerHooks {
    type Error;

    fn before_event(
        &mut self,
        _state: &SessionState,
        _event: &SessionEvent,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn after_event(
        &mut self,
        _state: &SessionState,
        _event: &SessionEvent,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSessionHooks;

impl SessionReducerHooks for NoopSessionHooks {
    type Error = core::convert::Infallible;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionReduceError {
    InvalidLifecycleTransition,
    HostCommandRejected,
    StepBoundaryRejected,
    ToolBatchAlreadyActive,
    ToolBatchNotActive,
    ToolBatchIdMismatch,
    ToolCallUnknown,
    ToolBatchNotSettled,
    MissingRunConfig,
    MissingActiveRun,
    MissingActiveTurn,
    MissingProvider,
    MissingModel,
    UnknownProvider,
    UnknownModel,
    RunAlreadyActive,
}

impl SessionReduceError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidLifecycleTransition => "invalid lifecycle transition",
            Self::HostCommandRejected => "host command rejected",
            Self::StepBoundaryRejected => "step boundary rejected",
            Self::ToolBatchAlreadyActive => "tool batch already active",
            Self::ToolBatchNotActive => "tool batch not active",
            Self::ToolBatchIdMismatch => "tool batch id mismatch",
            Self::ToolCallUnknown => "tool call id not expected in active batch",
            Self::ToolBatchNotSettled => "tool batch not settled",
            Self::MissingRunConfig => "run config missing",
            Self::MissingActiveRun => "active run missing",
            Self::MissingActiveTurn => "active turn missing",
            Self::MissingProvider => "run config provider missing",
            Self::MissingModel => "run config model missing",
            Self::UnknownProvider => "run config provider unknown",
            Self::UnknownModel => "run config model unknown",
            Self::RunAlreadyActive => "run already active",
        }
    }
}

impl core::fmt::Display for SessionReduceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl core::error::Error for SessionReduceError {}

pub fn apply_session_event(
    state: &mut SessionState,
    event: &SessionEvent,
) -> Result<(), SessionReduceError> {
    if state.created_at == 0 {
        state.created_at = event.step_epoch;
    }
    state.updated_at = event.step_epoch;

    match &event.event {
        SessionEventKind::RunRequested { run_overrides, .. } => {
            on_run_requested(state, run_overrides.as_ref())?;
        }
        SessionEventKind::RunStarted => {
            on_run_started(state)?;
        }
        SessionEventKind::StepBoundary => {
            on_step_boundary(state)?;
        }
        SessionEventKind::HostCommandReceived(command) => {
            on_host_command(state, command)?;
        }
        SessionEventKind::ToolBatchStarted {
            tool_batch_id,
            expected_call_ids,
        } => {
            on_tool_batch_started(state, tool_batch_id, expected_call_ids)?;
        }
        SessionEventKind::ToolCallSettled {
            tool_batch_id,
            call_id,
            status,
            receipt_session_epoch,
            receipt_step_epoch,
        } => {
            on_tool_call_settled(
                state,
                tool_batch_id,
                call_id,
                status,
                *receipt_session_epoch,
                *receipt_step_epoch,
            )?;
        }
        SessionEventKind::ToolBatchSettled {
            tool_batch_id,
            results_ref,
        } => {
            on_tool_batch_settled(state, tool_batch_id, results_ref.clone())?;
        }
        SessionEventKind::LeaseIssued { lease } => {
            state.active_run_lease = Some(lease.clone());
            state.last_heartbeat_at = Some(lease.issued_at);
        }
        SessionEventKind::LeaseExpiryCheck { observed_time_ns } => {
            on_lease_expiry_check(state, *observed_time_ns)?;
        }
        SessionEventKind::LifecycleChanged(next) => {
            transition_lifecycle(state, *next)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
            let _ = pop_follow_up_if_ready(state);
        }
        SessionEventKind::RunCompleted => {
            transition_lifecycle(state, SessionLifecycle::Completed)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
            clear_active_run(state);
        }
        SessionEventKind::RunFailed { .. } => {
            transition_lifecycle(state, SessionLifecycle::Failed)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
            clear_active_run(state);
        }
        SessionEventKind::RunCancelled { .. } => {
            if state.lifecycle != SessionLifecycle::Cancelling {
                transition_lifecycle(state, SessionLifecycle::Cancelling)
                    .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
            }
            transition_lifecycle(state, SessionLifecycle::Cancelled)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
            clear_active_run(state);
        }
        SessionEventKind::HostCommandApplied { .. } | SessionEventKind::Noop => {}
    }

    Ok(())
}

/// Apply a session event with deterministic provider/model catalog preflight checks.
///
/// For `RunRequested`, unknown provider/model values are rejected before any
/// active-run state is mutated.
pub fn apply_session_event_with_catalog(
    state: &mut SessionState,
    event: &SessionEvent,
    allowed_providers: &[&str],
    allowed_models: &[&str],
) -> Result<(), SessionReduceError> {
    if let SessionEventKind::RunRequested { run_overrides, .. } = &event.event {
        validate_run_request_catalog(
            state,
            run_overrides.as_ref(),
            allowed_providers,
            allowed_models,
        )?;
    }
    apply_session_event(state, event)
}

/// Validate a run request against optional provider/model allowlists.
///
/// Empty allowlists disable validation for that dimension.
pub fn validate_run_request_catalog(
    state: &SessionState,
    run_overrides: Option<&SessionConfig>,
    allowed_providers: &[&str],
    allowed_models: &[&str],
) -> Result<(), SessionReduceError> {
    let requested = select_run_config(&state.session_config, run_overrides);
    validate_run_catalog(&requested, allowed_providers, allowed_models)
}

/// Validate a concrete run config against optional provider/model allowlists.
pub fn validate_run_catalog(
    config: &RunConfig,
    allowed_providers: &[&str],
    allowed_models: &[&str],
) -> Result<(), SessionReduceError> {
    validate_run_config(config)?;

    if !allowed_providers.is_empty()
        && !allowed_providers
            .iter()
            .any(|value| config.provider.trim() == value.trim())
    {
        return Err(SessionReduceError::UnknownProvider);
    }

    if !allowed_models.is_empty()
        && !allowed_models
            .iter()
            .any(|value| config.model.trim() == value.trim())
    {
        return Err(SessionReduceError::UnknownModel);
    }

    Ok(())
}

fn on_run_requested(
    state: &mut SessionState,
    run_overrides: Option<&SessionConfig>,
) -> Result<(), SessionReduceError> {
    if state.active_run_id.is_some() {
        return Err(SessionReduceError::RunAlreadyActive);
    }

    let requested = select_run_config(&state.session_config, run_overrides);
    validate_run_config(&requested)?;

    state.active_run_id = Some(allocate_run_id(state));
    state.active_run_config = Some(requested);
    state.active_turn_id = None;
    state.active_step_id = None;
    state.active_tool_batch = None;
    state.in_flight_effects = 0;
    state.active_run_lease = None;
    state.last_heartbeat_at = None;
    Ok(())
}

fn on_run_started(state: &mut SessionState) -> Result<(), SessionReduceError> {
    let run_id = state
        .active_run_id
        .clone()
        .ok_or(SessionReduceError::MissingActiveRun)?;
    let config = state
        .active_run_config
        .as_ref()
        .ok_or(SessionReduceError::MissingRunConfig)?;
    validate_run_config(config)?;

    transition_lifecycle(state, SessionLifecycle::Running)
        .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
    state.step_epoch = state.step_epoch.saturating_add(1);

    if state.active_turn_id.is_none() {
        let turn_id = allocate_turn_id(state, &run_id);
        let step_id = allocate_step_id(state, &turn_id);
        state.active_turn_id = Some(turn_id);
        state.active_step_id = Some(step_id);
    }

    Ok(())
}

fn on_host_command(
    state: &mut SessionState,
    command: &crate::contracts::HostCommand,
) -> Result<(), SessionReduceError> {
    if let Some(expected) = command.expected_session_epoch {
        if expected != state.session_epoch {
            return Ok(());
        }
    }

    if let Some(target_run_id) = &command.target_run_id {
        if state.active_run_id.as_ref() != Some(target_run_id) {
            return Ok(());
        }
    }

    if !can_apply_host_command(state, &command.command) {
        return Err(SessionReduceError::HostCommandRejected);
    }

    enqueue_host_text(state, &command.command);

    match &command.command {
        HostCommandKind::Pause => {
            transition_lifecycle(state, SessionLifecycle::Paused)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
        }
        HostCommandKind::Resume => {
            transition_lifecycle(state, SessionLifecycle::Running)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
        }
        HostCommandKind::Cancel { .. } => {
            transition_lifecycle(state, SessionLifecycle::Cancelling)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
            apply_cancel_fence(state);
            maybe_finalize_cancelled(state)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
        }
        HostCommandKind::LeaseHeartbeat {
            lease_id,
            heartbeat_at,
        } => {
            if let Some(lease) = &state.active_run_lease {
                if lease.lease_id == *lease_id {
                    state.last_heartbeat_at = Some(*heartbeat_at);
                }
            }
        }
        HostCommandKind::Steer { .. }
        | HostCommandKind::FollowUp { .. }
        | HostCommandKind::Noop => {}
    }

    Ok(())
}

fn select_run_config(session: &SessionConfig, override_cfg: Option<&SessionConfig>) -> RunConfig {
    let source = override_cfg.unwrap_or(session);
    RunConfig {
        provider: source.provider.clone(),
        model: source.model.clone(),
        reasoning_effort: source.reasoning_effort,
        max_tokens: source.max_tokens,
    }
}

fn on_step_boundary(state: &mut SessionState) -> Result<(), SessionReduceError> {
    match state.lifecycle {
        SessionLifecycle::Running => {}
        SessionLifecycle::Idle | SessionLifecycle::WaitingInput => {
            let _ = pop_follow_up_if_ready(state);
            return Ok(());
        }
        _ => return Err(SessionReduceError::StepBoundaryRejected),
    }

    if state
        .active_tool_batch
        .as_ref()
        .is_some_and(|batch| !batch.is_settled())
    {
        return Err(SessionReduceError::ToolBatchNotSettled);
    }

    if state
        .active_tool_batch
        .as_ref()
        .is_some_and(ActiveToolBatch::is_settled)
    {
        state.active_tool_batch = None;
    }

    let _ = pop_pending_steer(state);
    state.step_epoch = state.step_epoch.saturating_add(1);

    let run_id = state
        .active_run_id
        .clone()
        .ok_or(SessionReduceError::MissingActiveRun)?;
    let turn_id = match state.active_turn_id.clone() {
        Some(turn) => turn,
        None => {
            let new_turn = allocate_turn_id(state, &run_id);
            state.active_turn_id = Some(new_turn.clone());
            new_turn
        }
    };
    let next_step = allocate_step_id(state, &turn_id);
    state.active_step_id = Some(next_step);
    Ok(())
}

fn on_tool_batch_started(
    state: &mut SessionState,
    tool_batch_id: &crate::contracts::ToolBatchId,
    expected_call_ids: &[alloc::string::String],
) -> Result<(), SessionReduceError> {
    if state
        .active_tool_batch
        .as_ref()
        .is_some_and(|batch| !batch.is_settled())
    {
        return Err(SessionReduceError::ToolBatchAlreadyActive);
    }

    let expected_set: BTreeSet<alloc::string::String> = expected_call_ids.iter().cloned().collect();
    let mut call_status = BTreeMap::new();
    for call_id in &expected_set {
        call_status.insert(call_id.clone(), ToolCallStatus::Pending);
    }

    state.in_flight_effects = expected_set.len() as u64;
    state.active_tool_batch = Some(ActiveToolBatch {
        tool_batch_id: tool_batch_id.clone(),
        issued_at_step_epoch: state.step_epoch,
        expected_call_ids: expected_set,
        call_status,
        results_ref: None,
    });
    Ok(())
}

fn on_tool_call_settled(
    state: &mut SessionState,
    tool_batch_id: &crate::contracts::ToolBatchId,
    call_id: &str,
    status: &ToolCallStatus,
    receipt_session_epoch: u64,
    receipt_step_epoch: u64,
) -> Result<(), SessionReduceError> {
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(SessionReduceError::ToolBatchNotActive)?;
    if batch.tool_batch_id != *tool_batch_id {
        return Err(SessionReduceError::ToolBatchIdMismatch);
    }
    if !batch.expected_call_ids.contains(call_id) {
        return Err(SessionReduceError::ToolCallUnknown);
    }

    let stale = receipt_session_epoch != state.session_epoch
        || receipt_step_epoch != batch.issued_at_step_epoch;
    let effective = if stale {
        ToolCallStatus::IgnoredStale
    } else {
        status.clone()
    };

    let was_terminal = batch
        .call_status
        .get(call_id)
        .is_some_and(ToolCallStatus::is_terminal);
    let is_terminal = effective.is_terminal();
    batch.call_status.insert(call_id.into(), effective);

    if !was_terminal && is_terminal {
        state.in_flight_effects = state.in_flight_effects.saturating_sub(1);
    }

    maybe_finalize_cancelled(state).map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
    Ok(())
}

fn on_tool_batch_settled(
    state: &mut SessionState,
    tool_batch_id: &crate::contracts::ToolBatchId,
    results_ref: Option<alloc::string::String>,
) -> Result<(), SessionReduceError> {
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(SessionReduceError::ToolBatchNotActive)?;
    if batch.tool_batch_id != *tool_batch_id {
        return Err(SessionReduceError::ToolBatchIdMismatch);
    }
    if !batch.is_settled() {
        return Err(SessionReduceError::ToolBatchNotSettled);
    }
    batch.results_ref = results_ref;
    maybe_finalize_cancelled(state).map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
    Ok(())
}

fn on_lease_expiry_check(
    state: &mut SessionState,
    observed_time_ns: u64,
) -> Result<(), SessionReduceError> {
    let Some(lease) = state.active_run_lease.clone() else {
        return Ok(());
    };
    let heartbeat_base = state.last_heartbeat_at.unwrap_or(lease.issued_at);
    let timeout_ns = lease.heartbeat_timeout_secs.saturating_mul(1_000_000_000);
    let heartbeat_deadline = heartbeat_base.saturating_add(timeout_ns);
    let expired = lease.is_expired_at(observed_time_ns) || observed_time_ns >= heartbeat_deadline;
    if expired {
        transition_lifecycle(state, SessionLifecycle::Cancelling)
            .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
        apply_cancel_fence(state);
        maybe_finalize_cancelled(state)
            .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
    }
    Ok(())
}

fn maybe_finalize_cancelled(
    state: &mut SessionState,
) -> Result<(), crate::helpers::LifecycleError> {
    if state.lifecycle != SessionLifecycle::Cancelling {
        return Ok(());
    }
    let batch_settled = state
        .active_tool_batch
        .as_ref()
        .is_none_or(ActiveToolBatch::is_settled);
    if batch_settled && state.in_flight_effects == 0 {
        transition_lifecycle(state, SessionLifecycle::Cancelled)?;
        clear_active_run(state);
    }
    Ok(())
}

fn validate_run_config(config: &RunConfig) -> Result<(), SessionReduceError> {
    if config.provider.trim().is_empty() {
        return Err(SessionReduceError::MissingProvider);
    }
    if config.model.trim().is_empty() {
        return Err(SessionReduceError::MissingModel);
    }
    Ok(())
}

fn clear_active_run(state: &mut SessionState) {
    state.active_run_id = None;
    state.active_run_config = None;
    state.active_turn_id = None;
    state.active_step_id = None;
    state.active_tool_batch = None;
    state.in_flight_effects = 0;
    state.active_run_lease = None;
    state.last_heartbeat_at = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{
        HostCommand, RunId, SessionId, StepId, ToolBatchId, ToolCallStatus, TurnId,
    };
    use alloc::{string::String, vec, vec::Vec};

    fn valid_config() -> SessionConfig {
        SessionConfig {
            provider: "openai".into(),
            model: "gpt-5.2".into(),
            reasoning_effort: None,
            max_tokens: Some(512),
        }
    }

    fn base_state() -> SessionState {
        SessionState {
            session_id: SessionId("11111111-1111-1111-1111-111111111111".into()),
            lifecycle: SessionLifecycle::Idle,
            session_config: valid_config(),
            ..SessionState::default()
        }
    }

    fn event(step_epoch: u64, event: SessionEventKind) -> SessionEvent {
        SessionEvent {
            session_id: SessionId("11111111-1111-1111-1111-111111111111".into()),
            session_epoch: 0,
            step_epoch,
            run_id: None,
            turn_id: None,
            step_id: None,
            event,
        }
    }

    fn run_requested(overrides: Option<SessionConfig>) -> SessionEvent {
        event(
            1,
            SessionEventKind::RunRequested {
                input_ref:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                run_overrides: overrides,
            },
        )
    }

    fn run_started(step_epoch: u64) -> SessionEvent {
        event(step_epoch, SessionEventKind::RunStarted)
    }

    fn running_state() -> SessionState {
        let mut state = base_state();
        apply_session_event(&mut state, &run_requested(None)).expect("run requested");
        apply_session_event(&mut state, &run_started(2)).expect("run started");
        state
    }

    fn active_tool_batch_id(state: &SessionState, batch_seq: u64) -> ToolBatchId {
        let step_id = state.active_step_id.clone().unwrap_or(StepId {
            turn_id: TurnId {
                run_id: RunId {
                    session_id: state.session_id.clone(),
                    run_seq: 1,
                },
                turn_seq: 1,
            },
            step_seq: 1,
        });
        ToolBatchId { step_id, batch_seq }
    }

    #[test]
    fn run_start_snapshots_overrides_immutably() {
        let mut state = base_state();
        let overrides = SessionConfig {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-5".into(),
            reasoning_effort: state.session_config.reasoning_effort,
            max_tokens: Some(1024),
        };

        apply_session_event(&mut state, &run_requested(Some(overrides))).expect("run requested");
        state.session_config.provider = "openai".into();
        state.session_config.model = "gpt-5.2".into();
        apply_session_event(&mut state, &run_started(2)).expect("run started");

        assert_eq!(state.lifecycle, SessionLifecycle::Running);
        assert_eq!(
            state
                .active_run_config
                .as_ref()
                .map(|cfg| cfg.provider.as_str()),
            Some("anthropic")
        );
        assert_eq!(
            state
                .active_run_config
                .as_ref()
                .map(|cfg| cfg.model.as_str()),
            Some("claude-sonnet-4-5")
        );
        assert!(state.active_turn_id.is_some());
        assert!(state.active_step_id.is_some());
    }

    #[test]
    fn run_request_rejects_missing_provider_without_activation() {
        let mut state = base_state();
        state.session_config.provider = " ".into();
        let err = apply_session_event(&mut state, &run_requested(None)).expect_err("provider");
        assert_eq!(err, SessionReduceError::MissingProvider);
        assert!(state.active_run_id.is_none());
        assert!(state.active_run_config.is_none());
    }

    #[test]
    fn run_started_without_requested_run_fails() {
        let mut state = base_state();
        let err = apply_session_event(&mut state, &run_started(1)).expect_err("missing run");
        assert_eq!(err, SessionReduceError::MissingActiveRun);
    }

    #[test]
    fn run_request_rejects_when_active_run_exists() {
        let mut state = base_state();
        state.lifecycle = SessionLifecycle::Running;
        state.active_run_id = Some(RunId {
            session_id: state.session_id.clone(),
            run_seq: 1,
        });
        state.active_run_config = Some(RunConfig {
            provider: "openai".into(),
            model: "gpt-5.2".into(),
            reasoning_effort: None,
            max_tokens: Some(512),
        });

        let err = apply_session_event(&mut state, &run_requested(None)).expect_err("active run");
        assert_eq!(err, SessionReduceError::RunAlreadyActive);
    }

    #[test]
    fn step_boundary_consumes_pending_steer_and_advances_step() {
        let mut state = running_state();
        let previous_step_seq = state
            .active_step_id
            .as_ref()
            .map(|id| id.step_seq)
            .unwrap_or(0);

        apply_session_event(
            &mut state,
            &event(
                3,
                SessionEventKind::HostCommandReceived(HostCommand {
                    command_id: "cmd-steer".into(),
                    target_run_id: None,
                    expected_session_epoch: None,
                    issued_at: 3,
                    command: HostCommandKind::Steer {
                        text: "use structured output".into(),
                    },
                }),
            ),
        )
        .expect("steer command");
        assert_eq!(state.pending_steer.len(), 1);

        apply_session_event(&mut state, &event(4, SessionEventKind::StepBoundary))
            .expect("step boundary");
        assert!(state.pending_steer.is_empty());
        assert_eq!(state.step_epoch, 2);
        assert_eq!(
            state.active_step_id.as_ref().map(|id| id.step_seq),
            Some(previous_step_seq + 1)
        );
    }

    #[test]
    fn follow_up_is_consumed_only_after_waiting_input_or_idle() {
        let mut state = running_state();
        apply_session_event(
            &mut state,
            &event(
                3,
                SessionEventKind::HostCommandReceived(HostCommand {
                    command_id: "cmd-followup".into(),
                    target_run_id: None,
                    expected_session_epoch: None,
                    issued_at: 3,
                    command: HostCommandKind::FollowUp {
                        text: "also summarize cost".into(),
                    },
                }),
            ),
        )
        .expect("follow up");
        assert_eq!(state.pending_follow_up.len(), 1);

        apply_session_event(&mut state, &event(4, SessionEventKind::StepBoundary))
            .expect("boundary while running");
        assert_eq!(state.pending_follow_up.len(), 1);

        apply_session_event(
            &mut state,
            &event(
                5,
                SessionEventKind::LifecycleChanged(SessionLifecycle::WaitingInput),
            ),
        )
        .expect("to waiting input");
        assert!(state.pending_follow_up.is_empty());
    }

    #[test]
    fn tool_batch_blocks_step_until_settled_and_orders_call_status_keys() {
        let mut state = running_state();
        let batch_id = active_tool_batch_id(&state, 1);
        apply_session_event(
            &mut state,
            &event(
                3,
                SessionEventKind::ToolBatchStarted {
                    tool_batch_id: batch_id.clone(),
                    expected_call_ids: vec!["call_b".into(), "call_a".into()],
                },
            ),
        )
        .expect("batch started");
        assert_eq!(state.in_flight_effects, 2);

        let blocked =
            apply_session_event(&mut state, &event(4, SessionEventKind::StepBoundary)).unwrap_err();
        assert_eq!(blocked, SessionReduceError::ToolBatchNotSettled);

        apply_session_event(
            &mut state,
            &event(
                5,
                SessionEventKind::ToolCallSettled {
                    tool_batch_id: batch_id.clone(),
                    call_id: "call_b".into(),
                    status: ToolCallStatus::Succeeded,
                    receipt_session_epoch: 0,
                    receipt_step_epoch: 1,
                },
            ),
        )
        .expect("settle b");
        apply_session_event(
            &mut state,
            &event(
                6,
                SessionEventKind::ToolCallSettled {
                    tool_batch_id: batch_id.clone(),
                    call_id: "call_a".into(),
                    status: ToolCallStatus::Failed {
                        code: "tool_err".into(),
                        detail: "boom".into(),
                    },
                    receipt_session_epoch: 0,
                    receipt_step_epoch: 1,
                },
            ),
        )
        .expect("settle a");
        let keys: Vec<String> = state
            .active_tool_batch
            .as_ref()
            .expect("active batch")
            .call_status
            .keys()
            .cloned()
            .collect();
        assert_eq!(keys, vec![String::from("call_a"), String::from("call_b")]);

        apply_session_event(
            &mut state,
            &event(
                7,
                SessionEventKind::ToolBatchSettled {
                    tool_batch_id: batch_id,
                    results_ref: Some(
                        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                            .into(),
                    ),
                },
            ),
        )
        .expect("batch settled");
        apply_session_event(&mut state, &event(8, SessionEventKind::StepBoundary))
            .expect("boundary after settled");
        assert!(state.active_tool_batch.is_none());
        assert_eq!(state.in_flight_effects, 0);
    }

    #[test]
    fn stale_receipts_are_marked_ignored_and_cancel_finalizes() {
        let mut state = running_state();
        let batch_id = active_tool_batch_id(&state, 1);
        apply_session_event(
            &mut state,
            &event(
                3,
                SessionEventKind::ToolBatchStarted {
                    tool_batch_id: batch_id.clone(),
                    expected_call_ids: vec!["call_a".into(), "call_b".into()],
                },
            ),
        )
        .expect("batch started");

        apply_session_event(
            &mut state,
            &event(
                4,
                SessionEventKind::HostCommandReceived(HostCommand {
                    command_id: "cmd-cancel".into(),
                    target_run_id: None,
                    expected_session_epoch: None,
                    issued_at: 4,
                    command: HostCommandKind::Cancel { reason: None },
                }),
            ),
        )
        .expect("cancel");
        assert_eq!(state.lifecycle, SessionLifecycle::Cancelling);
        assert_eq!(state.session_epoch, 1);

        apply_session_event(
            &mut state,
            &event(
                5,
                SessionEventKind::ToolCallSettled {
                    tool_batch_id: batch_id.clone(),
                    call_id: "call_a".into(),
                    status: ToolCallStatus::Succeeded,
                    receipt_session_epoch: 0,
                    receipt_step_epoch: 1,
                },
            ),
        )
        .expect("stale receipt");
        let status_a = state
            .active_tool_batch
            .as_ref()
            .and_then(|batch| batch.call_status.get("call_a"))
            .cloned();
        assert_eq!(status_a, Some(ToolCallStatus::IgnoredStale));
        assert_eq!(state.lifecycle, SessionLifecycle::Cancelling);

        apply_session_event(
            &mut state,
            &event(
                6,
                SessionEventKind::ToolCallSettled {
                    tool_batch_id: batch_id,
                    call_id: "call_b".into(),
                    status: ToolCallStatus::Succeeded,
                    receipt_session_epoch: 0,
                    receipt_step_epoch: 1,
                },
            ),
        )
        .expect("stale receipt b");
        assert_eq!(state.lifecycle, SessionLifecycle::Cancelled);
        assert!(state.active_run_id.is_none());
        assert!(state.active_tool_batch.is_none());
    }

    #[test]
    fn lease_expiry_check_cancels_deterministically() {
        let mut state = running_state();
        apply_session_event(
            &mut state,
            &event(
                3,
                SessionEventKind::LeaseIssued {
                    lease: crate::contracts::RunLease {
                        lease_id: "lease-1".into(),
                        issued_at: 1_000,
                        expires_at: 10_000,
                        heartbeat_timeout_secs: 1,
                    },
                },
            ),
        )
        .expect("lease issued");
        assert_eq!(state.last_heartbeat_at, Some(1_000));

        apply_session_event(
            &mut state,
            &event(
                4,
                SessionEventKind::LeaseExpiryCheck {
                    observed_time_ns: 2_000_000_000,
                },
            ),
        )
        .expect("lease check");
        assert_eq!(state.lifecycle, SessionLifecycle::Cancelled);
        assert!(state.active_run_id.is_none());
    }

    #[test]
    fn catalog_validation_rejects_unknown_provider_without_state_mutation() {
        let state = base_state();
        let override_cfg = SessionConfig {
            provider: "unknown-provider".into(),
            model: "gpt-5.2".into(),
            reasoning_effort: None,
            max_tokens: Some(64),
        };

        let err = validate_run_request_catalog(
            &state,
            Some(&override_cfg),
            &["openai", "anthropic"],
            &["gpt-5.2", "claude-sonnet-4-5"],
        )
        .expect_err("unknown provider");
        assert_eq!(err, SessionReduceError::UnknownProvider);
        assert!(state.active_run_id.is_none());
        assert_eq!(state.next_run_seq, 0);
    }

    #[test]
    fn catalog_validation_rejects_unknown_model_without_state_mutation() {
        let state = base_state();
        let override_cfg = SessionConfig {
            provider: "openai".into(),
            model: "not-a-model".into(),
            reasoning_effort: None,
            max_tokens: Some(64),
        };

        let err = validate_run_request_catalog(
            &state,
            Some(&override_cfg),
            &["openai", "anthropic"],
            &["gpt-5.2", "claude-sonnet-4-5"],
        )
        .expect_err("unknown model");
        assert_eq!(err, SessionReduceError::UnknownModel);
        assert!(state.active_run_id.is_none());
        assert_eq!(state.next_run_seq, 0);
    }

    #[test]
    fn catalog_apply_rejects_unknown_provider_without_partial_activation() {
        let mut state = base_state();
        let before = state.clone();
        let request = event(
            1,
            SessionEventKind::RunRequested {
                input_ref:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                run_overrides: Some(SessionConfig {
                    provider: "unknown-provider".into(),
                    model: "gpt-5.2".into(),
                    reasoning_effort: None,
                    max_tokens: Some(64),
                }),
            },
        );

        let err = apply_session_event_with_catalog(
            &mut state,
            &request,
            &["openai", "anthropic"],
            &["gpt-5.2", "claude-sonnet-4-5"],
        )
        .expect_err("catalog reject");
        assert_eq!(err, SessionReduceError::UnknownProvider);
        assert_eq!(state, before);
    }
}
