use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;

#[test]
fn top_level_help_lists_resource_roots() {
    let mut cmd = cargo_bin_cmd!("aos");
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("profile"))
        .stdout(predicate::str::contains("node"))
        .stdout(predicate::str::contains("local").not())
        .stdout(predicate::str::contains("hosted").not())
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
fn world_help_exposes_world_only_commands() {
    let mut cmd = cargo_bin_cmd!("aos");
    cmd.args(["world", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("create"))
        .stdout(predicate::str::contains("patch"))
        .stdout(predicate::str::contains("admin").not());

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
fn node_help_lists_lifecycle_commands() {
    let mut cmd = cargo_bin_cmd!("aos");
    cmd.args(["node", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("up"))
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("down"))
        .stdout(predicate::str::contains("use"));
}

#[test]
fn node_up_help_lists_backend_and_background_flags() {
    let mut cmd = cargo_bin_cmd!("aos");
    cmd.args(["node", "up", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--journal-backend"))
        .stdout(predicate::str::contains("--blob-backend"))
        .stdout(predicate::str::contains("--background"))
        .stdout(predicate::str::contains("--foreground").not());
}

#[test]
fn node_status_reports_stopped_without_runtime_state() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let mut cmd = cargo_bin_cmd!("aos");
    cmd.args([
        "--json",
        "node",
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
fn old_node_product_commands_are_not_public_aliases() {
    cargo_bin_cmd!("aos")
        .arg("local")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
    cargo_bin_cmd!("aos")
        .arg("hosted")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
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
fn universe_help_is_secret_only() {
    let mut cmd = cargo_bin_cmd!("aos");
    cmd.args(["universe", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("secret"))
        .stdout(predicate::str::contains("create").not());
}
