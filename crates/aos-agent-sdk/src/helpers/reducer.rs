use super::{
    allocate_run_id, allocate_step_id, allocate_turn_id, apply_cancel_fence,
    can_apply_host_command, enqueue_host_text, transition_lifecycle,
};
use crate::contracts::{
    HostCommandKind, RunConfig, SessionConfig, SessionEvent, SessionEventKind, SessionLifecycle,
    SessionState,
};

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
    MissingRunConfig,
    MissingActiveRun,
    MissingProvider,
    MissingModel,
    RunAlreadyActive,
}

impl SessionReduceError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidLifecycleTransition => "invalid lifecycle transition",
            Self::HostCommandRejected => "host command rejected",
            Self::MissingRunConfig => "run config missing",
            Self::MissingActiveRun => "active run missing",
            Self::MissingProvider => "run config provider missing",
            Self::MissingModel => "run config model missing",
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
        SessionEventKind::HostCommandReceived(command) => {
            on_host_command(state, command)?;
        }
        SessionEventKind::LifecycleChanged(next) => {
            transition_lifecycle(state, *next)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
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
    use crate::contracts::{RunId, SessionId};

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

    fn run_requested(overrides: Option<SessionConfig>) -> SessionEvent {
        SessionEvent {
            session_id: SessionId("11111111-1111-1111-1111-111111111111".into()),
            session_epoch: 0,
            step_epoch: 1,
            run_id: None,
            turn_id: None,
            step_id: None,
            event: SessionEventKind::RunRequested {
                input_ref:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                run_overrides: overrides,
            },
        }
    }

    fn run_started(step_epoch: u64) -> SessionEvent {
        SessionEvent {
            session_id: SessionId("11111111-1111-1111-1111-111111111111".into()),
            run_id: None,
            turn_id: None,
            step_id: None,
            session_epoch: 0,
            step_epoch,
            event: SessionEventKind::RunStarted,
        }
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
}
