use crate::contracts::{HostCommandKind, SessionLifecycle, SessionState};
use alloc::string::String;

pub fn enqueue_host_text(state: &mut SessionState, command: &HostCommandKind) {
    match command {
        HostCommandKind::Steer { text } => state.pending_steer.push(text.clone()),
        HostCommandKind::FollowUp { text } => state.pending_follow_up.push(text.clone()),
        _ => {}
    }
}

pub fn pop_pending_steer(state: &mut SessionState) -> Option<String> {
    if state.pending_steer.is_empty() {
        return None;
    }
    Some(state.pending_steer.remove(0))
}

pub fn pop_follow_up_if_ready(state: &mut SessionState) -> Option<String> {
    if !matches!(
        state.lifecycle,
        SessionLifecycle::Idle | SessionLifecycle::WaitingInput
    ) {
        return None;
    }

    if state.pending_follow_up.is_empty() {
        return None;
    }

    Some(state.pending_follow_up.remove(0))
}
