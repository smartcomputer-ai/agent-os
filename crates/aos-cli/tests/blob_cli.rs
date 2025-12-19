use aos_store::{FsStore, Store};
use assert_cmd::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn blob_get_defaults_to_metadata_without_raw_or_out() {
    let tmp = TempDir::new().expect("tmpdir");
    let world = tmp.path();
    fs::create_dir_all(world.join(".aos")).unwrap();

    // Seed a blob in the store.
    let store = FsStore::open(world).expect("store");
    let data = b"hello-bytes";
    let hash = store.put_blob(data).expect("put blob");

    // Run CLI without --raw/--out; expect metadata + warning, no raw bytes.
    let assert = std::process::Command::new(assert_cmd::cargo::cargo_bin!("aos"))
        .current_dir(world)
        .args([
            "--world",
            world.to_str().unwrap(),
            "blob",
            "get",
            &hash.to_hex(),
            "--json",
        ])
        .assert()
        .success();

    let output = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&output).expect("json");

    assert_eq!(json["data"]["bytes"].as_u64(), Some(data.len() as u64));
    assert!(
        json["data"]["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("--raw")
    );
    let warnings = json["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap_or_default().contains("metadata-only")),
        "expected metadata-only warning"
    );
}
