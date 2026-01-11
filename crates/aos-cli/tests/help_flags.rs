#[test]
fn help_mentions_new_flags_and_nouns() {
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("aos"))
        .arg("--help")
        .output()
        .expect("run help");
    assert!(output.status.success(), "--help should succeed");
    let text = String::from_utf8_lossy(&output.stdout);

    // Check a couple of important flags and nouns.
    for needle in ["--no-meta", "--pretty", "event", "blob"] {
        assert!(
            text.contains(needle),
            "help output should contain '{needle}'"
        );
    }
}
