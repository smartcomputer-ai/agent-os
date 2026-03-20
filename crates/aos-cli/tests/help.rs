use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;

#[test]
fn top_level_help_lists_resource_roots() {
    let mut cmd = cargo_bin_cmd!("aos");
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("profile"))
        .stdout(predicate::str::contains("local"))
        .stdout(predicate::str::contains("universe"))
        .stdout(predicate::str::contains("world"))
        .stdout(predicate::str::contains("workspace"))
        .stdout(predicate::str::contains("cas"))
        .stdout(predicate::str::contains("ops"));
}

#[test]
fn governance_help_only_exposes_submit_commands() {
    let mut cmd = cargo_bin_cmd!("aos");
    cmd.args(["world", "gov", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("propose"))
        .stdout(predicate::str::contains("shadow"))
        .stdout(predicate::str::contains("approve"))
        .stdout(predicate::str::contains("apply"))
        .stdout(predicate::str::contains(" ls ").not())
        .stdout(predicate::str::contains(" get ").not());
}

#[test]
fn world_help_uses_admin_and_create_without_upload() {
    let mut cmd = cargo_bin_cmd!("aos");
    cmd.args(["world", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("create"))
        .stdout(predicate::str::contains("admin"))
        .stdout(predicate::str::contains("set").not());

    cargo_bin_cmd!("aos")
        .args(["world", "upload", "--help"])
        .assert()
        .failure();

    cargo_bin_cmd!("aos")
        .args(["world", "pause", "--help"])
        .assert()
        .failure();
}

#[test]
fn local_help_lists_lifecycle_commands() {
    let mut cmd = cargo_bin_cmd!("aos");
    cmd.args(["local", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("up"))
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("down"))
        .stdout(predicate::str::contains("use"));
}

#[test]
fn local_status_reports_stopped_without_runtime_state() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let mut cmd = cargo_bin_cmd!("aos");
    cmd.args([
        "--json",
        "local",
        "status",
        "--root",
        temp.path().to_str().unwrap(),
    ]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("\"running\":false"))
        .stdout(predicate::str::contains("\"healthy\":false"));
}

#[test]
fn profile_help_lists_select_command() {
    let mut cmd = cargo_bin_cmd!("aos");
    cmd.args(["profile", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("select"))
        .stdout(predicate::str::contains("set"));
}

#[test]
fn universe_create_help_lists_select_flag() {
    let mut cmd = cargo_bin_cmd!("aos");
    cmd.args(["universe", "create", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--select"));
}
