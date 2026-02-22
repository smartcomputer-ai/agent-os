#[test]
fn plans_help_mentions_check_and_scaffold() {
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("aos"))
        .args(["plans", "--help"])
        .output()
        .expect("run plans --help");
    assert!(output.status.success(), "plans --help should succeed");
    let text = String::from_utf8_lossy(&output.stdout);

    for needle in ["check", "scaffold"] {
        assert!(
            text.contains(needle),
            "plans help output should contain '{needle}'"
        );
    }
}

#[test]
fn plans_scaffold_help_mentions_profile_and_pack() {
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("aos"))
        .args(["plans", "scaffold", "--help"])
        .output()
        .expect("run plans scaffold --help");
    assert!(
        output.status.success(),
        "plans scaffold --help should succeed"
    );
    let text = String::from_utf8_lossy(&output.stdout);

    for needle in ["--pack", "--profile", "turnkey", "composable-core"] {
        assert!(
            text.contains(needle),
            "plans scaffold help output should contain '{needle}'"
        );
    }
}

#[test]
fn plans_check_help_mentions_map_and_warning_flags() {
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("aos"))
        .args(["plans", "check", "--help"])
        .output()
        .expect("run plans check --help");
    assert!(output.status.success(), "plans check --help should succeed");
    let text = String::from_utf8_lossy(&output.stdout);

    for needle in ["--map", "--fail-on-warning"] {
        assert!(
            text.contains(needle),
            "plans check help output should contain '{needle}'"
        );
    }
}
