use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;

#[test]
fn air_check_reports_discovered_air_packages() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root");
    let world_root = workspace_root.join("worlds/demiurge");

    let output = cargo_bin_cmd!("aos")
        .args([
            "--json",
            "air",
            "check",
            "--world-root",
            world_root.to_str().expect("world root"),
            "--bin",
            "aos-air-export",
        ])
        .output()
        .expect("run aos air check");

    assert!(
        output.status.success(),
        "air check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("parse stdout");
    let packages = json["data"]["discovered_air_packages"]
        .as_array()
        .expect("packages array");
    let agent = packages
        .iter()
        .find(|package| package["package"] == "aos-agent")
        .expect("aos-agent package");
    assert_eq!(agent["version"], "0.1.0");
    assert!(
        agent["defs_hash"]
            .as_str()
            .expect("defs hash")
            .starts_with("sha256:")
    );
    assert!(
        agent["modules"]
            .as_array()
            .expect("modules")
            .iter()
            .any(|module| module == "aos.agent/SessionWorkflow_wasm@1")
    );
}
