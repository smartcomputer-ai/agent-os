use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn blob_echo_returns_data_to_reducer() {
    if !wasm_target_installed() {
        eprintln!("skipping blob-echo data test (missing wasm target)");
        return;
    }
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("aos-examples"));
    cmd.arg("blob-echo").env("RUST_LOG", "error");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("data_ok=true"));
}

fn wasm_target_installed() -> bool {
    Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("wasm32-unknown-unknown"))
        .unwrap_or(false)
}
