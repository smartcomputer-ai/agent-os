use serde_json::json;
use std::panic::{self, AssertUnwindSafe};

use super::assert_json_schema;
use crate::{Manifest, NamedRef};

#[test]
fn manifest_json_round_trip() {
    let manifest_json = json!({
        "$kind": "manifest",
        "air_version": "1",
        "schemas": [{"name": "com.acme/Schema@1", "hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}],
        "modules": [],
        "effects": [],
        "caps": [],
        "policies": [],
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
fn manifest_with_defaults_routing_and_subscriptions_validates() {
    let manifest_json = json!({
        "$kind": "manifest",
        "air_version": "1",
        "schemas": [{"name": "com.acme/Schema@1", "hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}],
        "modules": [{"name": "com.acme/Reducer@1", "hash": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}],
        "effects": [],
        "caps": [{"name": "com.acme/Cap@1", "hash": "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}],
        "policies": [{"name": "com.acme/Policy@1", "hash": "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"}],
        "defaults": {
            "policy": "com.acme/Policy@1",
            "cap_grants": [
                {
                    "name": "cap_http",
                    "cap": "com.acme/http@1",
                    "params": {"record": {}}
                }
            ]
        },
        "module_bindings": {
            "com.acme/Reducer@1": {
                "slots": {
                    "http": "cap_http"
                }
            }
        },
        "routing": {
            "subscriptions": [{
                "event": "com.acme/Event@1",
                "module": "com.acme/Reducer@1",
                "key_field": "id"
            }],
            "inboxes": [{
                "source": "mailbox://alerts",
                "module": "com.acme/Reducer@1"
            }]
        }
    });
    assert_json_schema(crate::schemas::MANIFEST, &manifest_json);
    let manifest: Manifest = serde_json::from_value(manifest_json).expect("manifest");
    assert!(manifest.defaults.as_ref().expect("defaults").policy.is_some());
    assert_eq!(manifest.module_bindings.len(), 1);
    let routing = manifest.routing.expect("routing");
    assert_eq!(routing.subscriptions.len(), 1);
}

#[test]
fn manifest_event_alias_maps_to_subscriptions() {
    let manifest_json = json!({
        "$kind": "manifest",
        "air_version": "1",
        "schemas": [{"name": "com.acme/Event@1", "hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}],
        "modules": [],
        "effects": [],
        "caps": [],
        "policies": [],
        "routing": {
            "events": [{
                "event": "com.acme/Event@1",
                "reducer": "com.acme/Reducer@1"
            }]
        }
    });
    assert_json_schema(crate::schemas::MANIFEST, &manifest_json);
    let manifest: Manifest = serde_json::from_value(manifest_json).expect("manifest");
    let routing = manifest.routing.expect("routing");
    assert_eq!(routing.subscriptions.len(), 1);
    assert_eq!(routing.subscriptions[0].module.as_str(), "com.acme/Reducer@1");
}

#[test]
fn module_binding_requires_slots_schema() {
    let manifest_json = json!({
        "$kind": "manifest",
        "air_version": "1",
        "schemas": [],
        "modules": [],
        "caps": [],
        "policies": [],
        "module_bindings": {
            "com.acme/Reducer@1": {}
        }
    });
    assert!(
        panic::catch_unwind(AssertUnwindSafe(|| assert_json_schema(
            crate::schemas::MANIFEST,
            &manifest_json
        )))
        .is_err(),
        "schema should require slots object inside module binding"
    );
}

#[test]
fn manifest_with_secrets_round_trip() {
    let manifest_json = json!({
        "$kind": "manifest",
        "air_version": "1",
        "schemas": [],
        "modules": [],
        "effects": [],
        "caps": [],
        "policies": [],
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
