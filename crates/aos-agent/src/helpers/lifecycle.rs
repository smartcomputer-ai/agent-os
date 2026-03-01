use crate::contracts::{HostCommandKind, SessionLifecycle, SessionState};

#[derive(Debug, PartialEq, Eq)]
pub enum LifecycleError {
    InvalidTransition {
        from: SessionLifecycle,
        to: SessionLifecycle,
    },
}

impl core::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidTransition { from, to } => {
                write!(f, "invalid transition from {from:?} to {to:?}")
            }
        }
    }
}

impl core::error::Error for LifecycleError {}

pub fn transition_lifecycle(
    state: &mut SessionState,
    next: SessionLifecycle,
) -> Result<(), LifecycleError> {
    if state.lifecycle == next {
        return Ok(());
    }

    let allowed = matches!(
        (state.lifecycle, next),
        (SessionLifecycle::Idle, SessionLifecycle::Running)
            | (SessionLifecycle::Running, SessionLifecycle::WaitingInput)
            | (SessionLifecycle::WaitingInput, SessionLifecycle::Running)
            | (SessionLifecycle::WaitingInput, SessionLifecycle::Paused)
            | (SessionLifecycle::Running, SessionLifecycle::Paused)
            | (SessionLifecycle::Paused, SessionLifecycle::Running)
            | (SessionLifecycle::Running, SessionLifecycle::Cancelling)
            | (SessionLifecycle::WaitingInput, SessionLifecycle::Cancelling)
            | (SessionLifecycle::Paused, SessionLifecycle::Cancelling)
            | (SessionLifecycle::Cancelling, SessionLifecycle::Cancelled)
            | (SessionLifecycle::Running, SessionLifecycle::Completed)
            | (SessionLifecycle::WaitingInput, SessionLifecycle::Completed)
            | (SessionLifecycle::Running, SessionLifecycle::Failed)
            | (SessionLifecycle::WaitingInput, SessionLifecycle::Failed)
            | (SessionLifecycle::Completed, SessionLifecycle::Running)
            | (SessionLifecycle::Failed, SessionLifecycle::Running)
            | (SessionLifecycle::Cancelled, SessionLifecycle::Running)
    );

    if !allowed {
        return Err(LifecycleError::InvalidTransition {
            from: state.lifecycle,
            to: next,
        });
    }

    state.lifecycle = next;
    Ok(())
}

pub fn can_apply_host_command(state: &SessionState, command: &HostCommandKind) -> bool {
    match command {
        HostCommandKind::Pause => {
            matches!(
                state.lifecycle,
                SessionLifecycle::Running | SessionLifecycle::WaitingInput
            )
        }
        HostCommandKind::Resume => matches!(state.lifecycle, SessionLifecycle::Paused),
        HostCommandKind::Cancel { .. } => {
            matches!(
                state.lifecycle,
                SessionLifecycle::Running
                    | SessionLifecycle::WaitingInput
                    | SessionLifecycle::Paused
            )
        }
        _ => !state.lifecycle.is_terminal(),
    }
}
