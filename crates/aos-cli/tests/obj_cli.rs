use assert_cmd::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn obj_ls_without_daemon_returns_empty_with_notice() {
    let tmp = TempDir::new().expect("tmpdir");
    let world = tmp.path();
    fs::create_dir_all(world.join(".aos")).unwrap();
    fs::create_dir_all(world.join("air")).unwrap();

    let assert = std::process::Command::new(assert_cmd::cargo::cargo_bin!("aos"))
        .current_dir(world)
        .args([
            "--world",
            world.to_str().unwrap(),
            "obj",
            "ls",
            "--json",
        ])
        .assert()
        .success();

    let output = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&output).expect("json");

    let arr = json["data"].as_array().cloned().unwrap_or_default();
    assert_eq!(arr.len(), 0, "objects list should be empty without daemon");
    let warnings = json["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap_or_default().contains("object listing requires daemon")),
        "expected daemon notice"
    );
}

#[test]
fn obj_ls_versions_flag_ignored_in_batch_with_warning() {
    let tmp = TempDir::new().expect("tmpdir");
    let world = tmp.path();
    fs::create_dir_all(world.join(".aos")).unwrap();
    fs::create_dir_all(world.join("air")).unwrap();

    let assert = std::process::Command::new(assert_cmd::cargo::cargo_bin!("aos"))
        .current_dir(world)
        .args([
            "--world",
            world.to_str().unwrap(),
            "obj",
            "ls",
            "--versions",
            "--json",
        ])
        .assert()
        .success();

    let output = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&output).expect("json");
    let warnings = json["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings
            .iter()
            .any(|w| w
                .as_str()
                .unwrap_or_default()
                .contains("filters require daemon-side catalog")),
        "expected warning about versions filter"
    );
}
