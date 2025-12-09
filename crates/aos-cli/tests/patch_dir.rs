use aos_cbor::Hash;
use assert_cmd::prelude::*;
use std::fs;
use tempfile::tempdir;

// Uses the built binary to exercise --patch-dir dry-run and inspect output.
#[test]
fn patch_dir_dry_run_emits_patchdoc_with_base_and_refs() {
    let tmp = tempdir().expect("tmpdir");
    let world = tmp.path().join("world");
    fs::create_dir_all(world.join(".aos")).unwrap();
    fs::create_dir_all(world.join("air")).unwrap();

    // Write a minimal manifest and a defschema into air/ so load_from_assets can find them.
    fs::write(
        world.join("air/manifest.air.json"),
        r#"{
          "$kind":"manifest",
          "air_version":"1",
          "schemas": [ { "name": "com.acme/Added@1", "hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000" } ],
          "modules": [],
          "plans": [],
          "effects": [],
          "caps": [],
          "policies": [],
          "secrets": [],
          "routing": null,
          "triggers": []
        }"#,
    )
    .unwrap();
    fs::write(
        world.join("air/defs.schema.json"),
        r#"[ { "$kind":"defschema", "name":"com.acme/Added@1", "type": { "bool": {} } } ]"#,
    )
    .unwrap();

    // Create a dummy current manifest in the store to derive base hash (world/.aos/manifest.air.cbor).
    let manifest_bytes = fs::read(world.join("air/manifest.air.json")).unwrap();
    fs::write(world.join(".aos/manifest.air.cbor"), &manifest_bytes).unwrap();
    let base_hash = Hash::of_bytes(&manifest_bytes).to_hex();

    // Run CLI in dry-run mode and capture stdout.
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin!("aos"));
    cmd.current_dir(&world)
        .arg("world")
        .arg("gov")
        .arg("propose")
        .arg("--patch-dir")
        .arg("air")
        .arg("--dry-run");
    let output = cmd.assert().success().get_output().stdout.clone();
    let text = String::from_utf8(output).unwrap();

    // Parse and inspect the emitted PatchDocument JSON.
    let doc: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(
        doc["base_manifest_hash"].as_str().unwrap(),
        base_hash,
        "base hash should match current manifest"
    );
    let patches = doc["patches"].as_array().unwrap();
    assert!(
        patches.iter().any(|p| p.get("add_def").is_some()),
        "should include add_def for schema"
    );
    assert!(
        patches.iter().any(|p| p.get("set_manifest_refs").is_some()),
        "should include set_manifest_refs"
    );
}
