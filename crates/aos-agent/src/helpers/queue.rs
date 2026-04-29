use crate::contracts::{QueuedRunStart, SessionLifecycle, SessionState};
use alloc::string::String;

pub fn pop_pending_steer_ref(state: &mut SessionState) -> Option<String> {
    if state.pending_steer_refs.is_empty() {
        return None;
    }
    Some(state.pending_steer_refs.remove(0))
}

pub fn pop_follow_up_if_ready(state: &mut SessionState) -> Option<QueuedRunStart> {
    if !matches!(
        state.lifecycle,
        SessionLifecycle::Idle
            | SessionLifecycle::WaitingInput
            | SessionLifecycle::Completed
            | SessionLifecycle::Failed
            | SessionLifecycle::Cancelled
            | SessionLifecycle::Interrupted
    ) {
        return None;
    }

    if state.queued_follow_up_runs.is_empty() {
        return None;
    }

    Some(state.queued_follow_up_runs.remove(0))
}
