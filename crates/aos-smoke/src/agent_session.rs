use std::path::Path;

use anyhow::{Result, anyhow, ensure};
use aos_agent_sdk::{
    HostCommand, HostCommandKind, SessionConfig, SessionEvent, SessionEventKind, SessionId,
    SessionLifecycle, SessionState, ToolBatchId, ToolCallStatus,
};

use crate::example_host::{ExampleHost, HarnessConfig};

const REDUCER_NAME: &str = "demo/AgentSessionReducer@1";
const EVENT_SCHEMA: &str = "aos.agent/SessionEvent@1";
const MODULE_CRATE: &str = "crates/aos-smoke/fixtures/10-agent-session/reducer";
const SESSION_ID: &str = "11111111-1111-1111-1111-111111111111";

pub fn run(example_root: &Path) -> Result<()> {
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_CRATE,
    })?;

    println!("→ Agent Session demo");

    // Run #1: boundary queues + deterministic tool-batch settle.
    host.send_event(&run_requested_event(1))?;
    host.send_event(&session_event(0, 2, SessionEventKind::RunStarted))?;
    host.send_event(&session_event(
        0,
        3,
        SessionEventKind::HostCommandReceived(HostCommand {
            command_id: "cmd-steer".into(),
            target_run_id: None,
            expected_session_epoch: None,
            issued_at: 3,
            command: HostCommandKind::Steer {
                text: "respond compactly".into(),
            },
        }),
    ))?;
    host.send_event(&session_event(0, 4, SessionEventKind::StepBoundary))?;

    let state_after_boundary: SessionState = host.read_state()?;
    ensure!(
        state_after_boundary.pending_steer.is_empty(),
        "expected steer queue consumed at boundary"
    );
    let batch1 = active_batch_id(&state_after_boundary, 1)?;
    host.send_event(&session_event(
        0,
        5,
        SessionEventKind::ToolBatchStarted {
            tool_batch_id: batch1.clone(),
            expected_call_ids: vec!["call_b".into(), "call_a".into()],
        },
    ))?;
    host.send_event(&session_event(
        0,
        6,
        SessionEventKind::ToolCallSettled {
            tool_batch_id: batch1.clone(),
            call_id: "call_b".into(),
            status: ToolCallStatus::Succeeded,
            receipt_session_epoch: 0,
            receipt_step_epoch: 2,
        },
    ))?;
    host.send_event(&session_event(
        0,
        7,
        SessionEventKind::ToolCallSettled {
            tool_batch_id: batch1.clone(),
            call_id: "call_a".into(),
            status: ToolCallStatus::Succeeded,
            receipt_session_epoch: 0,
            receipt_step_epoch: 2,
        },
    ))?;
    host.send_event(&session_event(
        0,
        8,
        SessionEventKind::ToolBatchSettled {
            tool_batch_id: batch1,
            results_ref: Some(
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            ),
        },
    ))?;
    host.send_event(&session_event(0, 9, SessionEventKind::StepBoundary))?;
    host.send_event(&session_event(0, 10, SessionEventKind::RunCompleted))?;

    // Run #2: cancel + stale late receipts -> IgnoredStale and deterministic Cancelled.
    host.send_event(&run_requested_event(11))?;
    host.send_event(&session_event(0, 12, SessionEventKind::RunStarted))?;
    let run2_state: SessionState = host.read_state()?;
    let batch2 = active_batch_id(&run2_state, 1)?;
    host.send_event(&session_event(
        0,
        13,
        SessionEventKind::ToolBatchStarted {
            tool_batch_id: batch2.clone(),
            expected_call_ids: vec!["late_a".into(), "late_b".into()],
        },
    ))?;
    host.send_event(&session_event(
        0,
        14,
        SessionEventKind::HostCommandReceived(HostCommand {
            command_id: "cmd-cancel".into(),
            target_run_id: None,
            expected_session_epoch: None,
            issued_at: 14,
            command: HostCommandKind::Cancel {
                reason: Some("operator stop".into()),
            },
        }),
    ))?;
    host.send_event(&session_event(
        0,
        15,
        SessionEventKind::ToolCallSettled {
            tool_batch_id: batch2.clone(),
            call_id: "late_a".into(),
            status: ToolCallStatus::Succeeded,
            receipt_session_epoch: 0,
            receipt_step_epoch: 4,
        },
    ))?;
    host.send_event(&session_event(
        0,
        16,
        SessionEventKind::ToolCallSettled {
            tool_batch_id: batch2,
            call_id: "late_b".into(),
            status: ToolCallStatus::Succeeded,
            receipt_session_epoch: 0,
            receipt_step_epoch: 4,
        },
    ))?;

    // Run #3: deterministic lease-expiry cancellation path.
    host.send_event(&run_requested_event(17))?;
    host.send_event(&session_event(0, 18, SessionEventKind::RunStarted))?;
    host.send_event(&session_event(
        0,
        19,
        SessionEventKind::LeaseIssued {
            lease: aos_agent_sdk::RunLease {
                lease_id: "lease-1".into(),
                issued_at: 10_000,
                expires_at: 60_000,
                heartbeat_timeout_secs: 1,
            },
        },
    ))?;
    host.send_event(&session_event(
        0,
        20,
        SessionEventKind::LeaseExpiryCheck {
            observed_time_ns: 2_000_000_000,
        },
    ))?;

    let state: SessionState = host.read_state()?;
    ensure!(
        state.lifecycle == SessionLifecycle::Cancelled,
        "expected Cancelled lifecycle, got {:?}",
        state.lifecycle
    );
    ensure!(
        state.active_run_id.is_none() && state.active_run_config.is_none(),
        "expected active run to be cleared"
    );
    ensure!(
        state.next_run_seq == 3,
        "expected deterministic run_seq=3, got {}",
        state.next_run_seq
    );
    if state.updated_at != 20 {
        return Err(anyhow!("expected updated_at=20, got {}", state.updated_at));
    }

    println!(
        "   lifecycle={:?} next_run_seq={} session_epoch={} updated_at={}",
        state.lifecycle, state.next_run_seq, state.session_epoch, state.updated_at
    );

    host.finish()?.verify_replay()?;
    Ok(())
}

fn session_event(session_epoch: u64, step_epoch: u64, kind: SessionEventKind) -> SessionEvent {
    SessionEvent {
        session_id: SessionId(SESSION_ID.into()),
        run_id: None,
        turn_id: None,
        step_id: None,
        session_epoch,
        step_epoch,
        event: kind,
    }
}

fn run_requested_event(step_epoch: u64) -> SessionEvent {
    session_event(
        0,
        step_epoch,
        SessionEventKind::RunRequested {
            input_ref: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .into(),
            run_overrides: Some(SessionConfig {
                provider: "openai".into(),
                model: "gpt-5.2".into(),
                reasoning_effort: None,
                max_tokens: Some(512),
            }),
        },
    )
}

fn active_batch_id(state: &SessionState, batch_seq: u64) -> Result<ToolBatchId> {
    let step_id = state
        .active_step_id
        .clone()
        .ok_or_else(|| anyhow!("missing active step id"))?;
    Ok(ToolBatchId { step_id, batch_seq })
}
