use serde_json::json;
use std::panic::{self, AssertUnwindSafe};

use super::assert_json_schema;
use crate::{DefModule, ModuleAbi, ModuleKind, ReducerAbi, SchemaRef};

#[test]
fn parses_reducer_module_with_cap_slots() {
    let module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/Reducer@1",
        "module_kind": "reducer",
        "wasm_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "abi": {
            "reducer": {
                "state": "com.acme/State@1",
                "event": "com.acme/Event@1",
                "effects_emitted": ["http.request"],
                "cap_slots": {
                    "http": "http.out"
                }
            }
        }
    });
    assert_json_schema(crate::schemas::DEFMODULE, &module_json);
    let module: DefModule = serde_json::from_value(module_json).expect("parse module");
    assert_eq!(module.name, "com.acme/Reducer@1");
    assert!(matches!(module.module_kind, ModuleKind::Reducer));
    let reducer = module.abi.reducer.expect("reducer abi");
    assert_eq!(reducer.state.as_str(), "com.acme/State@1");
}

#[test]
fn rejects_module_with_unknown_kind() {
    let bad_module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/Reducer@1",
        "module_kind": "plan",
        "wasm_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "abi": {}
    });
    assert!(serde_json::from_value::<DefModule>(bad_module_json).is_err());
}

#[test]
fn reducer_abi_struct_round_trip() {
    let abi = ModuleAbi {
        reducer: Some(ReducerAbi {
            state: SchemaRef::new("com.acme/State@1").unwrap(),
            event: SchemaRef::new("com.acme/Event@1").unwrap(),
            annotations: None,
            effects_emitted: vec![crate::EffectKind::HttpRequest],
            cap_slots: indexmap::IndexMap::new(),
        }),
    };
    let json = serde_json::to_value(&abi).expect("serialize");
    let round_trip: ModuleAbi = serde_json::from_value(json).expect("deserialize");
    assert!(round_trip.reducer.is_some());
}

#[test]
fn reducer_module_with_annotations_and_key_schema_validates() {
    let module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/Reducer@2",
        "module_kind": "reducer",
        "wasm_hash": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        "key_schema": "com.acme/Key@1",
        "abi": {
            "reducer": {
                "state": "com.acme/State@1",
                "event": "com.acme/Event@1",
                "annotations": "com.acme/Annotations@1",
                "effects_emitted": ["http.request", "timer.set"],
                "cap_slots": {
                    "http": "http.out",
                    "timer": "timer"
                }
            }
        }
    });
    assert_json_schema(crate::schemas::DEFMODULE, &module_json);
    let module: DefModule = serde_json::from_value(module_json).expect("module json");
    assert_eq!(module.key_schema.as_ref().unwrap().as_str(), "com.acme/Key@1");
    let reducer = module.abi.reducer.expect("reducer abi");
    assert_eq!(reducer.effects_emitted.len(), 2);
    assert_eq!(reducer.cap_slots.len(), 2);
}

#[test]
fn reducer_module_without_abi_is_rejected_by_schema() {
    let module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/Reducer@1",
        "module_kind": "reducer",
        "wasm_hash": "sha256:2222222222222222222222222222222222222222222222222222222222222222",
        "abi": {}
    });
    assert!(
        panic::catch_unwind(AssertUnwindSafe(|| assert_json_schema(crate::schemas::DEFMODULE, &module_json))).is_err(),
        "schema should reject reducer modules missing reducer ABI"
    );
}

#[test]
fn cap_slot_names_must_match_pattern() {
    let module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/Reducer@1",
        "module_kind": "reducer",
        "wasm_hash": "sha256:3333333333333333333333333333333333333333333333333333333333333333",
        "abi": {
            "reducer": {
                "state": "com.acme/State@1",
                "event": "com.acme/Event@1",
                "cap_slots": {
                    "1invalid": "http.out"
                }
            }
        }
    });
    assert!(
        panic::catch_unwind(AssertUnwindSafe(|| assert_json_schema(crate::schemas::DEFMODULE, &module_json))).is_err(),
        "patternProperties should reject leading digit"
    );
}
