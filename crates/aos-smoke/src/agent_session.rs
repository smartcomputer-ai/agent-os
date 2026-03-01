use std::path::Path;

use anyhow::{Result, ensure};
use aos_agent::{
    HostCommand, HostCommandKind, SessionConfig, SessionId, SessionIngress, SessionIngressKind,
    SessionLifecycle, SessionState,
};
use aos_host::config::HostConfig;

use crate::example_host::{ExampleHost, HarnessConfig};

const WORKFLOW_NAME: &str = "aos.agent/SessionWorkflow@1";
const EVENT_SCHEMA: &str = "aos.agent/SessionIngress@1";
const SDK_AIR_ROOT: &str = "crates/aos-agent/air";
const SDK_WASM_PACKAGE: &str = "aos-agent";
const SDK_WASM_BIN: &str = "session_workflow";
const SESSION_ID: &str = "11111111-1111-1111-1111-111111111111";

pub fn run(example_root: &Path) -> Result<()> {
    assert_run_request_validation(example_root)?;

    let sdk_air_root = crate::workspace_root().join(SDK_AIR_ROOT);
    let import_roots = vec![sdk_air_root];
    let mut host = ExampleHost::prepare_with_imports_host_config_and_module_bin(
        HarnessConfig {
            example_root,
            assets_root: None,
            workflow_name: WORKFLOW_NAME,
            event_schema: EVENT_SCHEMA,
            module_crate: "",
        },
        &import_roots,
        Some(HostConfig {
            llm: None,
            ..HostConfig::default()
        }),
        SDK_WASM_PACKAGE,
        SDK_WASM_BIN,
    )?;

    println!("→ Agent Session demo");

    // Run #1: request -> cancel.
    host.send_event(&run_requested_event_with_config(1, "openai", "gpt-5.2"))?;
    let run1 = host.read_state::<SessionState>()?;
    ensure!(
        matches!(
            run1.lifecycle,
            SessionLifecycle::Running | SessionLifecycle::WaitingInput
        ),
        "expected run1 lifecycle Running|WaitingInput, got {:?}",
        run1.lifecycle
    );

    host.send_event(&session_event(
        2,
        SessionIngressKind::HostCommandReceived(HostCommand {
            command_id: "cmd-cancel".into(),
            issued_at: 2,
            command: HostCommandKind::Cancel {
                reason: Some("operator stop".into()),
            },
        }),
    ))?;

    let cancelled = host.read_state::<SessionState>()?;
    ensure!(
        cancelled.lifecycle == SessionLifecycle::Cancelled,
        "expected Cancelled lifecycle, got {:?}",
        cancelled.lifecycle
    );
    ensure!(
        cancelled.active_run_id.is_none() && cancelled.active_run_config.is_none(),
        "expected active run cleared after cancellation"
    );

    // Run #2: request + explicit completion.
    host.send_event(&run_requested_event_with_config(
        3,
        "anthropic",
        "claude-sonnet-4-5",
    ))?;
    let run2 = host.read_state::<SessionState>()?;
    ensure!(
        matches!(
            run2.lifecycle,
            SessionLifecycle::Running | SessionLifecycle::WaitingInput
        ),
        "expected run2 lifecycle Running|WaitingInput, got {:?}",
        run2.lifecycle
    );
    ensure!(
        run2.active_run_config
            .as_ref()
            .is_some_and(|cfg| cfg.provider == "anthropic" && cfg.model == "claude-sonnet-4-5"),
        "unexpected run2 active_run_config"
    );
    host.send_event(&session_event(4, SessionIngressKind::RunCompleted))?;

    let state = host.read_state::<SessionState>()?;
    ensure!(
        state.lifecycle == SessionLifecycle::Completed,
        "expected final Completed lifecycle, got {:?}",
        state.lifecycle
    );
    ensure!(
        state.active_run_id.is_none() && state.active_run_config.is_none(),
        "expected active run cleared"
    );
    ensure!(
        state.next_run_seq == 2,
        "expected deterministic run_seq=2, got {}",
        state.next_run_seq
    );
    ensure!(
        state.updated_at == 4,
        "expected updated_at=4, got {}",
        state.updated_at
    );

    println!(
        "   lifecycle={:?} next_run_seq={} updated_at={}",
        state.lifecycle, state.next_run_seq, state.updated_at
    );

    let key = host.single_keyed_cell_key()?;
    host.finish_with_keyed_samples(Some(WORKFLOW_NAME), &[key])?
        .verify_replay()?;
    Ok(())
}

fn assert_run_request_validation(example_root: &Path) -> Result<()> {
    let sdk_air_root = crate::workspace_root().join(SDK_AIR_ROOT);
    let import_roots = vec![sdk_air_root];
    let mut host = ExampleHost::prepare_with_imports_host_config_and_module_bin(
        HarnessConfig {
            example_root,
            assets_root: None,
            workflow_name: WORKFLOW_NAME,
            event_schema: EVENT_SCHEMA,
            module_crate: "",
        },
        &import_roots,
        Some(HostConfig {
            llm: None,
            ..HostConfig::default()
        }),
        SDK_WASM_PACKAGE,
        SDK_WASM_BIN,
    )?;

    host.send_event(&run_requested_event_with_config(1, "openai", "gpt-5.2"))?;
    let baseline = host.read_state::<SessionState>()?;

    let err = host
        .send_event(&run_requested_event_with_config(2, "", "gpt-5.2"))
        .expect_err("empty provider should reject run request");
    let _ = err;
    let after_provider = host.read_state::<SessionState>()?;
    ensure!(
        after_provider == baseline,
        "invalid provider request must not mutate session state"
    );

    let err = host
        .send_event(&run_requested_event_with_config(3, "openai", ""))
        .expect_err("empty model should reject run request");
    let _ = err;
    let after_model = host.read_state::<SessionState>()?;
    ensure!(
        after_model == baseline,
        "invalid model request must not mutate session state"
    );

    println!("   validation checks: empty provider/model rejected");
    Ok(())
}

fn run_requested_event_with_config(
    observed_at_ns: u64,
    provider: &str,
    model: &str,
) -> SessionIngress {
    session_event(
        observed_at_ns,
        SessionIngressKind::RunRequested {
            input_ref: fake_hash('a'),
            run_overrides: Some(SessionConfig {
                provider: provider.into(),
                model: model.into(),
                reasoning_effort: None,
                max_tokens: Some(512),
                workspace_binding: None,
                default_prompt_pack: None,
                default_prompt_refs: Some(vec![fake_hash('e')]),
                default_tool_profile: None,
                default_tool_enable: Some(vec!["host.session.open".into()]),
                default_tool_disable: None,
                default_tool_force: None,
            }),
        },
    )
}

fn session_event(observed_at_ns: u64, ingress: SessionIngressKind) -> SessionIngress {
    SessionIngress {
        session_id: SessionId(SESSION_ID.into()),
        observed_at_ns,
        ingress,
    }
}

fn fake_hash(ch: char) -> String {
    let mut out = String::from("sha256:");
    for _ in 0..64 {
        out.push(ch);
    }
    out
}
