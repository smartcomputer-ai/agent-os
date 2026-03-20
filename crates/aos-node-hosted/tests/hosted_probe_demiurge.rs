use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use uuid::Uuid;

fn cluster_is_reachable() -> bool {
    let cluster_file = std::env::var_os("FDB_CLUSTER_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/local/etc/foundationdb/fdb.cluster"));
    let cluster_line = match fs::read_to_string(&cluster_file) {
        Ok(contents) => contents
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string(),
        Err(_) => return false,
    };
    let Some(coord_part) = cluster_line.split('@').nth(1) else {
        return false;
    };
    let Some(first_coord) = coord_part.split(',').next() else {
        return false;
    };
    let addresses: Vec<_> = match first_coord.to_socket_addrs() {
        Ok(addresses) => addresses.collect(),
        Err(_) => return false,
    };
    addresses
        .into_iter()
        .any(|address| TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn dotenv_contains_key(path: &Path, key: &str) -> bool {
    match dotenvy::from_path_iter(path) {
        Ok(iter) => iter
            .filter_map(Result::ok)
            .any(|(candidate, value)| candidate == key && !value.trim().is_empty()),
        Err(_) => false,
    }
}

fn openai_key_available() -> bool {
    std::env::var_os("OPENAI_API_KEY").is_some()
        || dotenv_contains_key(&workspace_root().join(".env"), "OPENAI_API_KEY")
        || dotenv_contains_key(
            &workspace_root().join("worlds/demiurge/.env"),
            "OPENAI_API_KEY",
        )
}

#[test]
fn hosted_probe_demiurge_smoke_runs_when_prereqs_are_available() {
    if !cluster_is_reachable() {
        eprintln!("skipping hosted Demiurge smoke because no local FDB cluster is reachable");
        return;
    }
    if !openai_key_available() {
        eprintln!("skipping hosted Demiurge smoke because OPENAI_API_KEY is unavailable");
        return;
    }

    let universe = format!("demiurge-smoke-{}", &Uuid::new_v4().to_string()[..8]);
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("hosted_probe"))
        .args([
            "demiurge-smoke",
            "--universe",
            &universe,
            "--task",
            "Reply with the single word ok. Do not modify files.",
            "--model",
            "gpt-5-mini",
            "--max-cycles",
            "160",
            "--sleep-ms",
            "100",
        ])
        .output()
        .expect("run hosted_probe demiurge-smoke");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: Value = serde_json::from_slice(&output.stdout).expect("decode hosted probe json");
    assert_eq!(report["mode"], "demiurge_task");
    assert_eq!(report["task_state"]["finished"], true);
    assert!(
        report["world_handle"]
            .as_str()
            .is_some_and(|value| value.starts_with("demiurge-"))
    );
    assert_eq!(
        report["task_state"]["status"].as_str(),
        Some("demiurge/finished")
    );
}
