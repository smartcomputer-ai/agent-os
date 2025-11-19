use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn fetch_notify_cli_runs() {
    if !wasm_target_installed() {
        eprintln!("skipping fetch-notify CLI test (missing wasm target)");
        return;
    }
    run_cli_example("fetch-notify", "Fetch & Notify demo");
}

#[test]
fn aggregator_cli_runs() {
    if !wasm_target_installed() {
        eprintln!("skipping aggregator CLI test (missing wasm target)");
        return;
    }
    run_cli_example("aggregator", "Aggregator demo");
}

fn run_cli_example(subcommand: &str, expected_snippet: &str) {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("aos-examples"));
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
