use serde_json::json;

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
