use serde_json::json;

use crate::{Manifest, NamedRef};

#[test]
fn manifest_json_round_trip() {
    let manifest_json = json!({
        "$kind": "manifest",
        "schemas": [{"name": "com.acme/Schema@1", "hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}],
        "modules": [],
        "plans": [{"name": "com.acme/Plan@1", "hash": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}],
        "caps": [],
        "policies": [],
        "triggers": []
    });
    let manifest: Manifest = serde_json::from_value(manifest_json.clone()).expect("manifest json");
    assert_eq!(manifest.plans.len(), 1);
    let round = serde_json::to_value(manifest).expect("serialize");
    assert_eq!(round["schemas"], manifest_json["schemas"]);
    assert_eq!(round["plans"], manifest_json["plans"]);
}

#[test]
fn named_ref_requires_hash() {
    assert!(serde_json::from_value::<NamedRef>(json!({"name": "com.acme/Plan@1"})).is_err());
}
