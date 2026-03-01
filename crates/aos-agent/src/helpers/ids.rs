use crate::contracts::{RunId, SessionState, ToolBatchId};

pub fn allocate_run_id(state: &mut SessionState) -> RunId {
    state.next_run_seq = state.next_run_seq.saturating_add(1);
    RunId {
        session_id: state.session_id.clone(),
        run_seq: state.next_run_seq,
    }
}

pub fn allocate_tool_batch_id(state: &mut SessionState, run_id: &RunId) -> ToolBatchId {
    state.next_tool_batch_seq = state.next_tool_batch_seq.saturating_add(1);
    ToolBatchId {
        run_id: run_id.clone(),
        batch_seq: state.next_tool_batch_seq,
    }
}
