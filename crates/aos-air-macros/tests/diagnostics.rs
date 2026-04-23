use std::fs;
use std::process::Command;

fn cargo_check_failing_source(source: &str) -> String {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root");
    let manifest = format!(
        r#"[package]
name = "aos-air-diagnostic-test"
version = "0.1.0"
edition = "2024"

[dependencies]
aos-wasm-sdk = {{ path = "{}", features = ["air-macros"] }}
"#,
        workspace_root.join("crates/aos-wasm-sdk").display()
    );
    fs::write(temp.path().join("Cargo.toml"), manifest).expect("write manifest");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir src");
    fs::write(temp.path().join("src/main.rs"), source).expect("write source");

    let output = Command::new("cargo")
        .arg("check")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()
        .expect("run cargo check");

    assert!(
        !output.status.success(),
        "diagnostic fixture unexpectedly compiled"
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn air_schema_missing_schema_name_reports_actionable_error() {
    let stderr = cargo_check_failing_source(
        r#"
use aos_wasm_sdk::AirSchema;

#[derive(AirSchema)]
struct MissingSchemaName {
    task: String,
}

fn main() {}
"#,
    );

    assert!(stderr.contains("missing #[aos(schema = \"...\")]"));
    assert!(stderr.contains("MissingSchemaName"));
}

#[test]
fn air_schema_bad_primitive_override_reports_field_error() {
    let stderr = cargo_check_failing_source(
        r#"
use aos_wasm_sdk::AirSchema;

#[derive(AirSchema)]
#[aos(schema = "demo/Bad@1")]
struct Bad {
    #[aos(air_type = "timestamp")]
    observed_at_ns: u64,
}

fn main() {}
"#,
    );

    assert!(stderr.contains("unsupported AIR primitive override 'timestamp'"));
    assert!(stderr.contains("observed_at_ns"));
}
