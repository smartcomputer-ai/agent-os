#[test]
fn help_describes_subcommands_and_examples() {
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("aos-node-hosted"))
        .arg("--help")
        .output()
        .expect("run aos-node-hosted --help");
    assert!(output.status.success(), "--help should succeed");
    let text = String::from_utf8_lossy(&output.stdout);

    for needle in [
        "worker",
        "control",
        "node",
        "Examples:",
        "--universe-id",
        "--bind",
    ] {
        assert!(
            text.contains(needle),
            "help output should contain '{needle}'"
        );
    }
}

#[test]
fn worker_command_defaults_to_all_universes() {
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("aos-node-hosted"))
        .args(["worker", "--help"])
        .output()
        .expect("run aos-node-hosted worker --help");
    assert!(output.status.success(), "worker help should succeed");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("omit to supervise all universes"));
}
