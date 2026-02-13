use std::path::Path;

use anyhow::{Result, anyhow, ensure};
use aos_agent_sdk::{
    HostCommand, HostCommandKind, SessionConfig, SessionEvent, SessionEventKind, SessionId,
    SessionLifecycle, SessionState,
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

    let run_overrides = SessionConfig {
        provider: "openai".into(),
        model: "gpt-5.2".into(),
        reasoning_effort: None,
        max_tokens: Some(512),
    };

    host.send_event(&session_event(
        0,
        1,
        SessionEventKind::RunRequested {
            input_ref: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .into(),
            run_overrides: Some(run_overrides),
        },
    ))?;
    host.send_event(&session_event(0, 2, SessionEventKind::RunStarted))?;
    host.send_event(&session_event(
        0,
        3,
        SessionEventKind::HostCommandReceived(HostCommand {
            command_id: "cmd-pause".into(),
            target_run_id: None,
            expected_session_epoch: None,
            issued_at: 3,
            command: HostCommandKind::Pause,
        }),
    ))?;
    host.send_event(&session_event(
        0,
        4,
        SessionEventKind::HostCommandReceived(HostCommand {
            command_id: "cmd-resume".into(),
            target_run_id: None,
            expected_session_epoch: None,
            issued_at: 4,
            command: HostCommandKind::Resume,
        }),
    ))?;
    host.send_event(&session_event(0, 5, SessionEventKind::RunCompleted))?;

    let state: SessionState = host.read_state()?;
    ensure!(
        state.lifecycle == SessionLifecycle::Completed,
        "expected Completed lifecycle, got {:?}",
        state.lifecycle
    );
    ensure!(
        state.active_run_id.is_none() && state.active_run_config.is_none(),
        "expected active run to be cleared"
    );
    ensure!(
        state.next_run_seq == 1,
        "expected deterministic run_seq=1, got {}",
        state.next_run_seq
    );

    if state.updated_at != 5 {
        return Err(anyhow!("expected updated_at=5, got {}", state.updated_at));
    }

    println!(
        "   lifecycle={:?} next_run_seq={} updated_at={}",
        state.lifecycle, state.next_run_seq, state.updated_at
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
