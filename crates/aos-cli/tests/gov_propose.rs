use std::fs;
use std::path::PathBuf;

use aos_cli::commands::gov::autofill_patchdoc_hashes;
use aos_cli::opts::{ResolvedDirs, WorldOpts};
use aos_host::manifest_loader::ZERO_HASH_SENTINEL;
use serde_json::json;

// Sanity-check the autofill helper exposed via the gov propose path.
#[test]
fn autofill_fills_zero_hashes_by_default() {
    let mut doc = json!({
        "base_manifest_hash": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        "patches": [
            { "add_def": { "kind": "defschema", "node": { "$kind":"defschema", "name":"demo/Added@1", "type": { "bool": {} } } } },
            { "set_manifest_refs": { "add": [ { "kind":"defschema", "name":"demo/Added@1", "hash": ZERO_HASH_SENTINEL } ] } }
        ]
    });

    autofill_patchdoc_hashes(&mut doc, false).expect("autofill");
    let filled = doc["patches"][1]["set_manifest_refs"]["add"][0]["hash"]
        .as_str()
        .expect("hash present");
    assert_ne!(filled, ZERO_HASH_SENTINEL, "hash should be filled");
    assert!(
        filled.starts_with("sha256:"),
        "hash should be canonical sha256 prefix"
    );
}

#[test]
fn require_hashes_rejects_zero_hashes() {
    let mut doc = json!({
        "base_manifest_hash": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        "patches": [
            { "set_manifest_refs": { "add": [ { "kind":"defschema", "name":"demo/Added@1", "hash": ZERO_HASH_SENTINEL } ] } }
        ]
    });

    let err = autofill_patchdoc_hashes(&mut doc, true).expect_err("should fail");
    assert!(
        err.to_string().contains("hash still zero"),
        "require-hashes should error on zero hashes"
    );
}
