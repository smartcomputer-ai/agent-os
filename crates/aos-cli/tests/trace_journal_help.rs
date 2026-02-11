#[test]
fn trace_help_mentions_debug_flags() {
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("aos"))
        .args(["trace", "--help"])
        .output()
        .expect("run trace --help");
    assert!(output.status.success(), "trace --help should succeed");
    let text = String::from_utf8_lossy(&output.stdout);

    for needle in [
        "--event-hash",
        "--schema",
        "--correlate-by",
        "--value",
        "--follow",
        "--out",
        "--window-limit",
    ] {
        assert!(
            text.contains(needle),
            "trace help output should contain '{needle}'"
        );
    }
}

#[test]
fn journal_tail_help_mentions_filters() {
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("aos"))
        .args(["journal", "tail", "--help"])
        .output()
        .expect("run journal tail --help");
    assert!(
        output.status.success(),
        "journal tail --help should succeed"
    );
    let text = String::from_utf8_lossy(&output.stdout);

    for needle in ["--from", "--limit", "--kinds", "--out"] {
        assert!(
            text.contains(needle),
            "journal tail help output should contain '{needle}'"
        );
    }
}
