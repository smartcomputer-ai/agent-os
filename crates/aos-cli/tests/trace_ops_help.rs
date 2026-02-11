#[test]
fn trace_find_help_mentions_query_flags() {
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("aos"))
        .args(["trace-find", "--help"])
        .output()
        .expect("run trace-find --help");
    assert!(output.status.success(), "trace-find --help should succeed");
    let text = String::from_utf8_lossy(&output.stdout);

    for needle in ["--schema", "--correlate-by", "--value", "--max-results"] {
        assert!(
            text.contains(needle),
            "trace-find help output should contain '{needle}'"
        );
    }
}

#[test]
fn trace_diagnose_help_mentions_modes() {
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("aos"))
        .args(["trace-diagnose", "--help"])
        .output()
        .expect("run trace-diagnose --help");
    assert!(
        output.status.success(),
        "trace-diagnose --help should succeed"
    );
    let text = String::from_utf8_lossy(&output.stdout);

    for needle in [
        "--event-hash",
        "--schema",
        "--correlate-by",
        "--value",
        "--window-limit",
    ] {
        assert!(
            text.contains(needle),
            "trace-diagnose help output should contain '{needle}'"
        );
    }
}
