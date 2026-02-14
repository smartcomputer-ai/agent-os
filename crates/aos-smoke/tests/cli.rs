use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

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
    "agent-failure-classification",
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
fn agent_failure_classification_cli_runs() {
    run_cli_smoke("agent-failure-classification");
}

#[test]
#[ignore = "CLI smoke tests are opt-in to keep default test runs fast"]
fn all_agent_cli_runs_sdk_lane() {
    run_cli_smoke("all-agent");
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
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("aos-smoke"));
    cmd.arg(subcommand).env("RUST_LOG", "error");
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
