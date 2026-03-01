use assert_cmd::prelude::*;
use once_cell::sync::Lazy;
use predicates::prelude::*;
use std::process::Command;
use std::sync::Mutex;

static CLI_SMOKE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

const CLI_SMOKE_TESTS: &[&str] = &[
    "counter",
    "hello-timer",
    "blob-echo",
    "fetch-notify",
    "aggregator",
    "chain-comp",
    "safe-upgrade",
    "llm-summarizer",
    "agent-session",
    "agent-tools",
    "trace-failure-classification",
    "workflow-runtime-hardening",
    "all-agent",
];

#[test]
#[ignore = "CLI smoke tests are opt-in to keep default test runs fast"]
fn counter_cli_runs() {
    run_cli_smoke("counter");
}

#[test]
#[ignore = "CLI smoke tests are opt-in to keep default test runs fast"]
fn hello_timer_cli_runs() {
    run_cli_smoke("hello-timer");
}

#[test]
#[ignore = "CLI smoke tests are opt-in to keep default test runs fast"]
fn blob_echo_cli_runs() {
    run_cli_smoke("blob-echo");
}

#[test]
#[ignore = "CLI smoke tests are opt-in to keep default test runs fast"]
fn fetch_notify_cli_runs() {
    run_cli_smoke("fetch-notify");
}

#[test]
#[ignore = "CLI smoke tests are opt-in to keep default test runs fast"]
fn aggregator_cli_runs() {
    run_cli_smoke("aggregator");
}

#[test]
#[ignore = "CLI smoke tests are opt-in to keep default test runs fast"]
fn chain_comp_cli_runs() {
    run_cli_smoke("chain-comp");
}

#[test]
#[ignore = "CLI smoke tests are opt-in to keep default test runs fast"]
fn safe_upgrade_cli_runs() {
    run_cli_smoke("safe-upgrade");
}

#[test]
#[ignore = "CLI smoke tests are opt-in to keep default test runs fast"]
fn llm_summarizer_cli_runs() {
    run_cli_smoke("llm-summarizer");
}

#[test]
#[ignore = "CLI smoke tests are opt-in to keep default test runs fast"]
fn agent_session_cli_runs() {
    run_cli_smoke("agent-session");
}

#[test]
#[ignore = "CLI smoke tests are opt-in to keep default test runs fast"]
fn agent_tools_cli_runs() {
    run_cli_smoke("agent-tools");
}

#[test]
#[ignore = "CLI smoke tests are opt-in to keep default test runs fast"]
fn trace_failure_classification_cli_runs() {
    run_cli_smoke("trace-failure-classification");
}

#[test]
#[ignore = "CLI smoke tests are opt-in to keep default test runs fast"]
fn workflow_runtime_hardening_cli_runs() {
    run_cli_smoke("workflow-runtime-hardening");
}

#[test]
#[ignore = "CLI smoke tests are opt-in to keep default test runs fast"]
fn all_agent_cli_runs_sdk_lane() {
    run_cli_example("all-agent", "(agent-session)");
}

fn run_cli_smoke(subcommand: &str) {
    if !wasm_target_installed() {
        eprintln!("skipping {subcommand} CLI test (missing wasm target)");
        return;
    }
    assert!(
        CLI_SMOKE_TESTS.contains(&subcommand),
        "unknown CLI example '{subcommand}'"
    );
    run_cli_example(subcommand, &format!("({subcommand})"));
}

fn run_cli_example(subcommand: &str, expected_snippet: &str) {
    let _guard = CLI_SMOKE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace crates/")
        .parent()
        .expect("workspace root");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("aos-smoke"));
    cmd.current_dir(workspace_root)
        .arg(subcommand)
        .env("RUST_LOG", "error");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains(expected_snippet));
}

fn wasm_target_installed() -> bool {
    Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("wasm32-unknown-unknown"))
        .unwrap_or(false)
}
