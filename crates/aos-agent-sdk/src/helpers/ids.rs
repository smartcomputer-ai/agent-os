use crate::contracts::{RunId, SessionState, StepId, TurnId};

pub fn allocate_run_id(state: &mut SessionState) -> RunId {
    state.next_run_seq += 1;
    RunId {
        session_id: state.session_id.clone(),
        run_seq: state.next_run_seq,
    }
}

pub fn allocate_turn_id(state: &mut SessionState, run_id: &RunId) -> TurnId {
    state.next_turn_seq += 1;
    TurnId {
        run_id: run_id.clone(),
        turn_seq: state.next_turn_seq,
    }
}

pub fn allocate_step_id(state: &mut SessionState, turn_id: &TurnId) -> StepId {
    state.next_step_seq += 1;
    StepId {
        turn_id: turn_id.clone(),
        step_seq: state.next_step_seq,
    }
}
