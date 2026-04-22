use serde_json::json;
use std::panic::{self, AssertUnwindSafe};

use super::assert_json_schema;
use crate::{Manifest, NamedRef};

#[test]
fn manifest_json_round_trip() {
    let manifest_json = json!({
        "$kind": "manifest",
        "air_version": "2",
        "schemas": [{"name": "com.acme/Schema@1", "hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}],
        "modules": [],
        "ops": [],
        "secrets": []
    });
    assert_json_schema(crate::schemas::MANIFEST, &manifest_json);
    let manifest: Manifest = serde_json::from_value(manifest_json.clone()).expect("manifest json");
    let round = serde_json::to_value(manifest).expect("serialize");
    assert_eq!(round["schemas"], manifest_json["schemas"]);
}

#[test]
fn named_ref_requires_hash() {
    assert!(serde_json::from_value::<NamedRef>(json!({"name": "com.acme/Schema@1"})).is_err());
}

#[test]
fn manifest_with_routing_and_subscriptions_validates() {
    let manifest_json = json!({
        "$kind": "manifest",
        "air_version": "2",
        "schemas": [{"name": "com.acme/Schema@1", "hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}],
        "modules": [{"name": "com.acme/order_wasm@1", "hash": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}],
        "ops": [{"name": "com.acme/order.step@1", "hash": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}],
        "routing": {
            "subscriptions": [{
                "event": "com.acme/Event@1",
                "op": "com.acme/order.step@1",
                "key_field": "id"
            }]
        }
    });
    assert_json_schema(crate::schemas::MANIFEST, &manifest_json);
    let manifest: Manifest = serde_json::from_value(manifest_json).expect("manifest");
    let routing = manifest.routing.expect("routing");
    assert_eq!(routing.subscriptions.len(), 1);
    assert_eq!(routing.subscriptions[0].op, "com.acme/order.step@1");
}

#[test]
fn manifest_rejects_v1_effects_field() {
    let manifest_json = json!({
        "$kind": "manifest",
        "air_version": "2",
        "schemas": [],
        "modules": [],
        "ops": [],
        "effects": []
    });
    assert!(
        panic::catch_unwind(AssertUnwindSafe(|| assert_json_schema(
            crate::schemas::MANIFEST,
            &manifest_json
        )))
        .is_err(),
        "schema should reject legacy manifest.effects"
    );
}

#[test]
fn manifest_rejects_v1_routing_module() {
    let manifest_json = json!({
        "$kind": "manifest",
        "air_version": "2",
        "schemas": [],
        "modules": [],
        "ops": [],
        "routing": {
            "subscriptions": [{
                "event": "com.acme/Event@1",
                "module": "com.acme/Workflow@1"
            }]
        }
    });
    assert!(
        panic::catch_unwind(AssertUnwindSafe(|| assert_json_schema(
            crate::schemas::MANIFEST,
            &manifest_json
        )))
        .is_err(),
        "schema should reject routing subscriptions by module"
    );
}

#[test]
fn manifest_rejects_authority_fields() {
    let manifest_json = json!({
        "$kind": "manifest",
        "air_version": "2",
        "schemas": [],
        "modules": [],
        "ops": [],
        "caps": [],
        "policies": [],
        "defaults": {
            "policy": "com.acme/Policy@1",
            "cap_grants": []
        },
        "module_bindings": {
            "com.acme/Workflow@1": {}
        }
    });
    assert!(
        panic::catch_unwind(AssertUnwindSafe(|| assert_json_schema(
            crate::schemas::MANIFEST,
            &manifest_json
        )))
        .is_err(),
        "schema should reject legacy cap/policy authority fields"
    );
}

#[test]
fn manifest_with_secrets_round_trip() {
    let manifest_json = json!({
        "$kind": "manifest",
        "air_version": "2",
        "schemas": [],
        "modules": [],
        "ops": [],
        "secrets": [{
            "name": "payments/stripe@1",
            "hash": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        }]
    });
    assert_json_schema(crate::schemas::MANIFEST, &manifest_json);
    let manifest: Manifest = serde_json::from_value(manifest_json.clone()).expect("manifest json");
    assert_eq!(manifest.secrets.len(), 1);
    let round = serde_json::to_value(manifest).expect("serialize");
    assert_eq!(round["secrets"], manifest_json["secrets"]);
}
