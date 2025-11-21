use serde_json::json;
use std::panic::{self, AssertUnwindSafe};

use super::assert_json_schema;
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
    assert_json_schema(crate::schemas::MANIFEST, &manifest_json);
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

#[test]
fn manifest_with_defaults_routing_and_triggers_validates() {
    let manifest_json = json!({
        "$kind": "manifest",
        "schemas": [{"name": "com.acme/Schema@1", "hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}],
        "modules": [{"name": "com.acme/Reducer@1", "hash": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}],
        "plans": [{"name": "com.acme/Plan@1", "hash": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}],
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
            "events": [{
                "event": "com.acme/Event@1",
                "reducer": "com.acme/Reducer@1",
                "key_field": "id"
            }],
            "inboxes": [{
                "source": "mailbox://alerts",
                "reducer": "com.acme/Reducer@1"
            }]
        },
        "triggers": [{
            "event": "com.acme/Event@1",
            "plan": "com.acme/Plan@1",
            "correlate_by": "id"
        }]
    });
    assert_json_schema(crate::schemas::MANIFEST, &manifest_json);
    let manifest: Manifest = serde_json::from_value(manifest_json).expect("manifest");
    assert!(manifest.defaults.as_ref().unwrap().policy.is_some());
    assert_eq!(manifest.module_bindings.len(), 1);
    assert_eq!(manifest.triggers.len(), 1);
}

#[test]
fn module_binding_requires_slots_schema() {
    let manifest_json = json!({
        "$kind": "manifest",
        "schemas": [],
        "modules": [],
        "plans": [],
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
        "schemas": [],
        "modules": [],
        "plans": [],
        "caps": [],
        "policies": [],
        "secrets": [{
            "alias": "payments/stripe",
            "version": 1,
            "binding_id": "stripe:prod",
            "expected_digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "policy": {
                "allowed_caps": ["stripe_cap"],
                "allowed_plans": ["com.acme/Plan@1"]
            }
        }]
    });
    assert_json_schema(crate::schemas::MANIFEST, &manifest_json);
    let manifest: Manifest = serde_json::from_value(manifest_json.clone()).expect("manifest json");
    assert_eq!(manifest.secrets.len(), 1);
    let round = serde_json::to_value(manifest).expect("serialize");
    assert_eq!(
        round["secrets"][0]["alias"],
        manifest_json["secrets"][0]["alias"]
    );
}
