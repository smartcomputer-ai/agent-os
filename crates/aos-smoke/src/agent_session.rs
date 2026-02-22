use std::path::Path;

use anyhow::{Result, anyhow, ensure};
use aos_agent_sdk::{
    HostCommand, HostCommandKind, SessionConfig, SessionEvent, SessionEventKind, SessionId,
    SessionLifecycle, SessionState, ToolBatchId, ToolCallStatus,
};

use crate::example_host::{ExampleHost, HarnessConfig};

const REDUCER_NAME: &str = "demo/AgentSessionReducer@1";
const EVENT_SCHEMA: &str = "aos.agent/SessionEvent@1";
const MODULE_CRATE: &str = "crates/aos-smoke/fixtures/20-agent-session/reducer";
const SESSION_ID: &str = "11111111-1111-1111-1111-111111111111";

pub fn run(example_root: &Path) -> Result<()> {
    assert_catalog_rejections(example_root)?;

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
    let run1_state: SessionState = host.read_state()?;
    ensure!(
        run1_state
            .active_run_config
            .as_ref()
            .and_then(|cfg| cfg.prompt_refs.as_ref())
            .is_some_and(|refs| {
                refs == &vec![
                    "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                        .to_string(),
                ]
            }),
        "expected direct prompt refs materialized into active run config"
    );
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

    // Run #4: lease heartbeat renewal prevents premature cancellation.
    host.send_event(&run_requested_event(21))?;
    host.send_event(&session_event(0, 22, SessionEventKind::RunStarted))?;
    host.send_event(&session_event(
        0,
        23,
        SessionEventKind::LeaseIssued {
            lease: aos_agent_sdk::RunLease {
                lease_id: "lease-2".into(),
                issued_at: 1_000_000_000,
                expires_at: 10_000_000_000,
                heartbeat_timeout_secs: 1,
            },
        },
    ))?;
    host.send_event(&session_event(
        0,
        24,
        SessionEventKind::HostCommandReceived(HostCommand {
            command_id: "cmd-heartbeat".into(),
            target_run_id: None,
            expected_session_epoch: None,
            issued_at: 24,
            command: HostCommandKind::LeaseHeartbeat {
                lease_id: "lease-2".into(),
                heartbeat_at: 1_500_000_000,
            },
        }),
    ))?;
    host.send_event(&session_event(
        0,
        25,
        SessionEventKind::LeaseExpiryCheck {
            observed_time_ns: 2_000_000_000,
        },
    ))?;
    let renewed_state: SessionState = host.read_state()?;
    ensure!(
        renewed_state.lifecycle == SessionLifecycle::Running,
        "expected heartbeat renewal to keep lifecycle Running, got {:?}",
        renewed_state.lifecycle
    );
    host.send_event(&session_event(
        0,
        26,
        SessionEventKind::LeaseExpiryCheck {
            observed_time_ns: 2_600_000_000,
        },
    ))?;

    // Run #5: high-contention multi-batch fan-in with out-of-order and duplicate receipts.
    host.send_event(&run_requested_event(27))?;
    host.send_event(&session_event(0, 28, SessionEventKind::RunStarted))?;
    let run5_state: SessionState = host.read_state()?;
    let batch5a = active_batch_id(&run5_state, 1)?;
    host.send_event(&session_event(
        0,
        29,
        SessionEventKind::ToolBatchStarted {
            tool_batch_id: batch5a.clone(),
            expected_call_ids: vec!["hc_d".into(), "hc_a".into(), "hc_c".into(), "hc_b".into()],
        },
    ))?;
    let batch5a_started: SessionState = host.read_state()?;
    let issued_step_a = batch5a_started
        .active_tool_batch
        .as_ref()
        .ok_or_else(|| anyhow!("missing active batch after start"))?
        .issued_at_step_epoch;
    let receipt_epoch_a = batch5a_started.session_epoch;

    for (step_epoch, call_id, status) in [
        (30, "hc_c", ToolCallStatus::Succeeded),
        (31, "hc_a", ToolCallStatus::Succeeded),
        (
            32,
            "hc_d",
            ToolCallStatus::Failed {
                code: "tool_timeout".into(),
                detail: "timeout".into(),
            },
        ),
        (33, "hc_c", ToolCallStatus::Succeeded), // duplicate; must not change in-flight.
        (34, "hc_b", ToolCallStatus::Succeeded),
    ] {
        host.send_event(&session_event(
            0,
            step_epoch,
            SessionEventKind::ToolCallSettled {
                tool_batch_id: batch5a.clone(),
                call_id: call_id.into(),
                status,
                receipt_session_epoch: receipt_epoch_a,
                receipt_step_epoch: issued_step_a,
            },
        ))?;
    }
    let batch5a_state: SessionState = host.read_state()?;
    ensure!(
        batch5a_state.in_flight_effects == 0,
        "expected in_flight_effects=0 after terminal fan-in, got {}",
        batch5a_state.in_flight_effects
    );
    let ordered_keys: Vec<&str> = batch5a_state
        .active_tool_batch
        .as_ref()
        .ok_or_else(|| anyhow!("active batch missing before settle"))?
        .call_status
        .keys()
        .map(String::as_str)
        .collect();
    ensure!(
        ordered_keys == vec!["hc_a", "hc_b", "hc_c", "hc_d"],
        "expected deterministic call-id ordering in batch map, got {:?}",
        ordered_keys
    );
    host.send_event(&session_event(
        0,
        35,
        SessionEventKind::ToolBatchSettled {
            tool_batch_id: batch5a,
            results_ref: Some(
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".into(),
            ),
        },
    ))?;
    host.send_event(&session_event(0, 36, SessionEventKind::StepBoundary))?;

    let run5_mid_state: SessionState = host.read_state()?;
    let batch5b = active_batch_id(&run5_mid_state, 2)?;
    host.send_event(&session_event(
        0,
        37,
        SessionEventKind::ToolBatchStarted {
            tool_batch_id: batch5b.clone(),
            expected_call_ids: vec!["hd_z".into(), "hd_x".into(), "hd_y".into()],
        },
    ))?;
    let batch5b_started: SessionState = host.read_state()?;
    let issued_step_b = batch5b_started
        .active_tool_batch
        .as_ref()
        .ok_or_else(|| anyhow!("missing second active batch"))?
        .issued_at_step_epoch;
    let receipt_epoch_b = batch5b_started.session_epoch;

    host.send_event(&session_event(
        0,
        38,
        SessionEventKind::ToolCallSettled {
            tool_batch_id: batch5b.clone(),
            call_id: "hd_y".into(),
            status: ToolCallStatus::Succeeded,
            receipt_session_epoch: receipt_epoch_b,
            receipt_step_epoch: issued_step_b.saturating_sub(1), // stale-on-purpose
        },
    ))?;
    host.send_event(&session_event(
        0,
        39,
        SessionEventKind::ToolCallSettled {
            tool_batch_id: batch5b.clone(),
            call_id: "hd_x".into(),
            status: ToolCallStatus::Failed {
                code: "tool_error".into(),
                detail: "boom".into(),
            },
            receipt_session_epoch: receipt_epoch_b,
            receipt_step_epoch: issued_step_b,
        },
    ))?;
    host.send_event(&session_event(
        0,
        40,
        SessionEventKind::ToolCallSettled {
            tool_batch_id: batch5b.clone(),
            call_id: "hd_z".into(),
            status: ToolCallStatus::Succeeded,
            receipt_session_epoch: receipt_epoch_b,
            receipt_step_epoch: issued_step_b,
        },
    ))?;
    let batch5b_state: SessionState = host.read_state()?;
    let stale_status = batch5b_state
        .active_tool_batch
        .as_ref()
        .and_then(|batch| batch.call_status.get("hd_y"))
        .cloned();
    ensure!(
        stale_status == Some(ToolCallStatus::IgnoredStale),
        "expected stale receipt status=IgnoredStale, got {:?}",
        stale_status
    );
    host.send_event(&session_event(
        0,
        41,
        SessionEventKind::ToolBatchSettled {
            tool_batch_id: batch5b,
            results_ref: Some(
                "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".into(),
            ),
        },
    ))?;
    host.send_event(&session_event(0, 42, SessionEventKind::StepBoundary))?;
    host.send_event(&session_event(0, 43, SessionEventKind::RunCompleted))?;

    // Run #6: max_steps_per_run circuit breaker (MVP P2.3) deterministically fails runaway loop.
    host.send_event(&run_requested_event(44))?;
    host.send_event(&session_event(0, 45, SessionEventKind::RunStarted))?;
    host.send_event(&session_event(0, 46, SessionEventKind::StepBoundary))?;
    host.send_event(&session_event(0, 47, SessionEventKind::StepBoundary))?;
    host.send_event(&session_event(0, 48, SessionEventKind::StepBoundary))?;
    host.send_event(&session_event(0, 49, SessionEventKind::StepBoundary))?;
    host.send_event(&session_event(0, 50, SessionEventKind::StepBoundary))?;
    host.send_event(&session_event(0, 51, SessionEventKind::StepBoundary))?;
    let run6_state: SessionState = host.read_state()?;
    ensure!(
        run6_state.lifecycle == SessionLifecycle::Failed,
        "expected step-cap circuit breaker to set Failed, got {:?}",
        run6_state.lifecycle
    );
    ensure!(
        run6_state.active_run_id.is_none() && run6_state.active_run_config.is_none(),
        "expected run6 active run cleared after cap trigger"
    );
    ensure!(
        run6_state.active_run_step_count == 0,
        "expected run6 step counter reset after cap trigger"
    );

    // Run #7: no-tool completion; active_run_config remains immutable during the run.
    host.send_event(&run_requested_event_with_config(54, "openai", "gpt-5.2"))?;
    host.send_event(&session_event(0, 55, SessionEventKind::RunStarted))?;
    let run7_started: SessionState = host.read_state()?;
    let run7_config = run7_started
        .active_run_config
        .clone()
        .ok_or_else(|| anyhow!("run7 active_run_config missing"))?;
    ensure!(
        run7_config.provider == "openai" && run7_config.model == "gpt-5.2",
        "unexpected run7 config: provider={} model={}",
        run7_config.provider,
        run7_config.model
    );
    host.send_event(&session_event(0, 56, SessionEventKind::StepBoundary))?;
    let run7_mid: SessionState = host.read_state()?;
    ensure!(
        run7_mid.active_run_config == Some(run7_config.clone()),
        "run7 active_run_config drifted during run"
    );
    host.send_event(&session_event(0, 57, SessionEventKind::RunCompleted))?;

    // Run #8: provider/model update applies to next run only.
    host.send_event(&run_requested_event_with_config(
        58,
        "anthropic",
        "claude-sonnet-4-5",
    ))?;
    host.send_event(&session_event(0, 59, SessionEventKind::RunStarted))?;
    let run8_started: SessionState = host.read_state()?;
    let run8_config = run8_started
        .active_run_config
        .clone()
        .ok_or_else(|| anyhow!("run8 active_run_config missing"))?;
    ensure!(
        run8_config.provider == "anthropic" && run8_config.model == "claude-sonnet-4-5",
        "unexpected run8 config: provider={} model={}",
        run8_config.provider,
        run8_config.model
    );
    host.send_event(&session_event(0, 60, SessionEventKind::RunCompleted))?;

    let state: SessionState = host.read_state()?;
    ensure!(
        state.lifecycle == SessionLifecycle::Completed,
        "expected final Completed lifecycle, got {:?}",
        state.lifecycle
    );
    ensure!(
        state.active_run_id.is_none() && state.active_run_config.is_none(),
        "expected active run to be cleared"
    );
    ensure!(
        state.next_run_seq == 8,
        "expected deterministic run_seq=8, got {}",
        state.next_run_seq
    );
    ensure!(
        state.session_epoch == 3,
        "expected session_epoch=3, got {}",
        state.session_epoch
    );
    ensure!(
        state.updated_at == 60,
        "expected updated_at=60, got {}",
        state.updated_at
    );

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

fn assert_catalog_rejections(example_root: &Path) -> Result<()> {
    // Isolate reject-path checks in throwaway hosts so replay assertions for the
    // main conformance timeline remain strict and deterministic.
    let mut provider_host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_CRATE,
    })?;
    provider_host.send_event(&run_requested_event_with_config(1, "openai", "gpt-5.2"))?;
    provider_host.send_event(&session_event(0, 2, SessionEventKind::RunStarted))?;
    provider_host.send_event(&session_event(0, 3, SessionEventKind::RunCompleted))?;
    let provider_before: SessionState = provider_host.read_state()?;
    let _ = provider_host
        .send_event(&run_requested_event_with_config(
            4,
            "unknown-provider",
            "gpt-5.2",
        ))
        .expect_err("unknown provider should reject run request");
    let provider_after: SessionState = provider_host.read_state()?;
    ensure!(
        provider_after == provider_before,
        "unknown provider request must not mutate session state"
    );

    let mut model_host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_CRATE,
    })?;
    model_host.send_event(&run_requested_event_with_config(1, "openai", "gpt-5.2"))?;
    model_host.send_event(&session_event(0, 2, SessionEventKind::RunStarted))?;
    model_host.send_event(&session_event(0, 3, SessionEventKind::RunCompleted))?;
    let model_before: SessionState = model_host.read_state()?;
    let _ = model_host
        .send_event(&run_requested_event_with_config(4, "openai", "not-a-model"))
        .expect_err("unknown model should reject run request");
    let model_after: SessionState = model_host.read_state()?;
    ensure!(
        model_after == model_before,
        "unknown model request must not mutate session state"
    );
    println!("   catalog rejection checks: unknown provider/model rejected without state mutation");

    Ok(())
}

fn run_requested_event(step_epoch: u64) -> SessionEvent {
    run_requested_event_with_config(step_epoch, "openai", "gpt-5.2")
}

fn run_requested_event_with_config(step_epoch: u64, provider: &str, model: &str) -> SessionEvent {
    session_event(
        0,
        step_epoch,
        SessionEventKind::RunRequested {
            input_ref: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .into(),
            run_overrides: Some(SessionConfig {
                provider: provider.into(),
                model: model.into(),
                reasoning_effort: None,
                max_tokens: Some(512),
                workspace_binding: None,
                default_prompt_pack: None,
                default_prompt_refs: Some(vec![
                    "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                        .into(),
                ]),
                default_tool_catalog: None,
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
