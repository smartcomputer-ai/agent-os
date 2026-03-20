use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use serde_json::{Value, json};

fn write_config(path: &std::path::Path, value: Value) {
    fs::write(
        path,
        serde_json::to_vec_pretty(&value).expect("encode config"),
    )
    .expect("write config");
}

fn read_config(path: &std::path::Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read config")).expect("decode config")
}

#[test]
fn profile_select_only_updates_current_profile() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let config_path = temp.path().join("cli.json");
    write_config(
        &config_path,
        json!({
            "current_profile": "alpha",
            "profiles": {
                "alpha": {
                    "kind": "remote",
                    "api": "http://127.0.0.1:1",
                    "universe": "alpha-u",
                    "world": "alpha-w"
                },
                "beta": {
                    "kind": "remote",
                    "api": "http://127.0.0.1:2",
                    "universe": "beta-u",
                    "world": "beta-w"
                }
            }
        }),
    );

    cargo_bin_cmd!("aos")
        .args([
            "--config",
            config_path.to_str().expect("config path"),
            "--json",
            "profile",
            "select",
            "beta",
        ])
        .assert()
        .success();

    let config = read_config(&config_path);
    assert_eq!(
        config.get("current_profile").and_then(Value::as_str),
        Some("beta")
    );
    assert_eq!(
        config
            .get("profiles")
            .and_then(|profiles| profiles.get("alpha"))
            .and_then(|profile| profile.get("universe"))
            .and_then(Value::as_str),
        Some("alpha-u")
    );
    assert_eq!(
        config
            .get("profiles")
            .and_then(|profiles| profiles.get("beta"))
            .and_then(|profile| profile.get("world"))
            .and_then(Value::as_str),
        Some("beta-w")
    );
}

#[test]
fn profile_set_updates_target_profile_without_switching_current() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let config_path = temp.path().join("cli.json");
    write_config(
        &config_path,
        json!({
            "current_profile": "alpha",
            "profiles": {
                "alpha": {
                    "kind": "remote",
                    "api": "http://127.0.0.1:1",
                    "universe": "alpha-u",
                    "world": "alpha-w"
                },
                "beta": {
                    "kind": "remote",
                    "api": "http://127.0.0.1:2",
                    "universe": "beta-u",
                    "world": "beta-w"
                }
            }
        }),
    );

    cargo_bin_cmd!("aos")
        .args([
            "--config",
            config_path.to_str().expect("config path"),
            "--json",
            "profile",
            "set",
            "--profile",
            "beta",
            "--universe",
            "next-u",
            "--world",
            "next-w",
        ])
        .assert()
        .success();

    let config = read_config(&config_path);
    assert_eq!(
        config.get("current_profile").and_then(Value::as_str),
        Some("alpha")
    );
    assert_eq!(
        config
            .get("profiles")
            .and_then(|profiles| profiles.get("beta"))
            .and_then(|profile| profile.get("universe"))
            .and_then(Value::as_str),
        Some("next-u")
    );
    assert_eq!(
        config
            .get("profiles")
            .and_then(|profiles| profiles.get("beta"))
            .and_then(|profile| profile.get("world"))
            .and_then(Value::as_str),
        Some("next-w")
    );
}

#[test]
fn profile_set_without_mutation_flags_fails() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let config_path = temp.path().join("cli.json");
    write_config(
        &config_path,
        json!({
            "current_profile": "alpha",
            "profiles": {
                "alpha": {
                    "kind": "remote",
                    "api": "http://127.0.0.1:1",
                    "universe": "alpha-u",
                    "world": "alpha-w"
                }
            }
        }),
    );

    cargo_bin_cmd!("aos")
        .args([
            "--config",
            config_path.to_str().expect("config path"),
            "p",
            "set",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "profile set requires at least one of --universe, --world, or --kind",
        ));
}

#[test]
fn profile_clear_without_flags_deselects_current_profile_only() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let config_path = temp.path().join("cli.json");
    write_config(
        &config_path,
        json!({
            "current_profile": "alpha",
            "profiles": {
                "alpha": {
                    "kind": "remote",
                    "api": "http://127.0.0.1:1",
                    "universe": "alpha-u",
                    "world": "alpha-w"
                }
            }
        }),
    );

    cargo_bin_cmd!("aos")
        .args([
            "--config",
            config_path.to_str().expect("config path"),
            "--json",
            "p",
            "clear",
        ])
        .assert()
        .success();

    let config = read_config(&config_path);
    assert_eq!(config.get("current_profile").and_then(Value::as_str), None);
    assert_eq!(
        config
            .get("profiles")
            .and_then(|profiles| profiles.get("alpha"))
            .and_then(|profile| profile.get("universe"))
            .and_then(Value::as_str),
        Some("alpha-u")
    );
    assert_eq!(
        config
            .get("profiles")
            .and_then(|profiles| profiles.get("alpha"))
            .and_then(|profile| profile.get("world"))
            .and_then(Value::as_str),
        Some("alpha-w")
    );
}

#[test]
fn profile_clear_selector_flags_update_profile_fields() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let config_path = temp.path().join("cli.json");
    write_config(
        &config_path,
        json!({
            "current_profile": "alpha",
            "profiles": {
                "alpha": {
                    "kind": "remote",
                    "api": "http://127.0.0.1:1",
                    "universe": "alpha-u",
                    "world": "alpha-w"
                }
            }
        }),
    );

    cargo_bin_cmd!("aos")
        .args([
            "--config",
            config_path.to_str().expect("config path"),
            "--json",
            "p",
            "clear",
            "--clear-universe",
            "--clear-world",
        ])
        .assert()
        .success();

    let config = read_config(&config_path);
    assert_eq!(
        config.get("current_profile").and_then(Value::as_str),
        Some("alpha")
    );
    assert_eq!(
        config
            .get("profiles")
            .and_then(|profiles| profiles.get("alpha"))
            .and_then(|profile| profile.get("universe"))
            .and_then(Value::as_str),
        None
    );
    assert_eq!(
        config
            .get("profiles")
            .and_then(|profiles| profiles.get("alpha"))
            .and_then(|profile| profile.get("world"))
            .and_then(Value::as_str),
        None
    );
}

#[test]
fn profile_show_returns_only_selected_profile() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let config_path = temp.path().join("cli.json");
    write_config(
        &config_path,
        json!({
            "current_profile": "alpha",
            "profiles": {
                "alpha": {
                    "kind": "remote",
                    "api": "http://127.0.0.1:1",
                    "universe": "alpha-u",
                    "world": "alpha-w"
                },
                "beta": {
                    "kind": "remote",
                    "api": "http://127.0.0.1:2",
                    "universe": "beta-u",
                    "world": "beta-w"
                }
            }
        }),
    );

    cargo_bin_cmd!("aos")
        .args([
            "--config",
            config_path.to_str().expect("config path"),
            "p",
            "show",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\": \"alpha\""))
        .stdout(predicate::str::contains("\"api\": \"http://127.0.0.1:1\""))
        .stdout(predicate::str::contains("\"api\": \"http://127.0.0.1:2\"").not())
        .stdout(predicate::str::contains("\"current_profile\"").not())
        .stdout(predicate::str::contains("\"profiles\"").not());
}

#[test]
fn profile_show_without_selection_prints_nothing() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let config_path = temp.path().join("cli.json");
    write_config(
        &config_path,
        json!({
            "current_profile": null,
            "profiles": {
                "alpha": {
                    "kind": "remote",
                    "api": "http://127.0.0.1:1",
                    "universe": "alpha-u",
                    "world": "alpha-w"
                }
            }
        }),
    );

    cargo_bin_cmd!("aos")
        .args([
            "--config",
            config_path.to_str().expect("config path"),
            "p",
            "show",
        ])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}
