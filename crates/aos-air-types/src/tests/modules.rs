use serde_json::json;
use std::panic::{self, AssertUnwindSafe};

use super::assert_json_schema;
use crate::{DefModule, ModuleAbi, ModuleKind, SchemaRef, WorkflowAbi};

#[test]
fn parses_workflow_module_with_cap_slots() {
    let module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/Workflow@1",
        "module_kind": "workflow",
        "wasm_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "abi": {
            "workflow": {
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
    assert_eq!(module.name, "com.acme/Workflow@1");
    assert!(matches!(module.module_kind, ModuleKind::Workflow));
    let workflow = module.abi.workflow.expect("workflow abi");
    assert_eq!(workflow.state.as_str(), "com.acme/State@1");
}

#[test]
fn rejects_module_with_unknown_kind() {
    let bad_module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/Workflow@1",
        "module_kind": "plan",
        "wasm_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "abi": {}
    });
    assert!(serde_json::from_value::<DefModule>(bad_module_json).is_err());
}

#[test]
fn workflow_abi_struct_round_trip() {
    let abi = ModuleAbi {
        workflow: Some(WorkflowAbi {
            context: None,
            state: SchemaRef::new("com.acme/State@1").unwrap(),
            event: SchemaRef::new("com.acme/Event@1").unwrap(),
            annotations: None,
            effects_emitted: vec![crate::EffectKind::http_request()],
            cap_slots: indexmap::IndexMap::new(),
        }),
        pure: None,
    };
    let json = serde_json::to_value(&abi).expect("serialize");
    let round_trip: ModuleAbi = serde_json::from_value(json).expect("deserialize");
    assert!(round_trip.workflow.is_some());
}

#[test]
fn workflow_module_with_annotations_and_key_schema_validates() {
    let module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/Workflow@2",
        "module_kind": "workflow",
        "wasm_hash": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        "key_schema": "com.acme/Key@1",
        "abi": {
            "workflow": {
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
    assert_eq!(
        module.key_schema.as_ref().unwrap().as_str(),
        "com.acme/Key@1"
    );
    let workflow = module.abi.workflow.expect("workflow abi");
    assert_eq!(workflow.effects_emitted.len(), 2);
    assert_eq!(workflow.cap_slots.len(), 2);
}

#[test]
fn workflow_module_without_abi_is_rejected_by_schema() {
    let module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/Workflow@1",
        "module_kind": "workflow",
        "wasm_hash": "sha256:2222222222222222222222222222222222222222222222222222222222222222",
        "abi": {}
    });
    assert!(
        panic::catch_unwind(AssertUnwindSafe(|| assert_json_schema(
            crate::schemas::DEFMODULE,
            &module_json
        )))
        .is_err(),
        "schema should reject workflow modules missing workflow ABI"
    );
}

#[test]
fn parses_pure_module() {
    let module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/Pure@1",
        "module_kind": "pure",
        "wasm_hash": "sha256:4444444444444444444444444444444444444444444444444444444444444444",
        "abi": {
            "pure": {
                "input": "com.acme/Input@1",
                "output": "com.acme/Output@1",
                "context": "sys/PureContext@1"
            }
        }
    });
    assert_json_schema(crate::schemas::DEFMODULE, &module_json);
    let module: DefModule = serde_json::from_value(module_json).expect("parse module");
    assert!(matches!(module.module_kind, ModuleKind::Pure));
    let pure = module.abi.pure.expect("pure abi");
    assert_eq!(pure.input.as_str(), "com.acme/Input@1");
}

#[test]
fn pure_module_without_abi_is_rejected_by_schema() {
    let module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/Pure@1",
        "module_kind": "pure",
        "wasm_hash": "sha256:5555555555555555555555555555555555555555555555555555555555555555",
        "abi": {}
    });
    assert!(
        panic::catch_unwind(AssertUnwindSafe(|| assert_json_schema(
            crate::schemas::DEFMODULE,
            &module_json
        )))
        .is_err(),
        "schema should reject pure modules missing pure ABI"
    );
}

#[test]
fn cap_slot_names_must_match_pattern() {
    let module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/Workflow@1",
        "module_kind": "workflow",
        "wasm_hash": "sha256:3333333333333333333333333333333333333333333333333333333333333333",
        "abi": {
            "workflow": {
                "state": "com.acme/State@1",
                "event": "com.acme/Event@1",
                "cap_slots": {
                    "1invalid": "http.out"
                }
            }
        }
    });
    assert!(
        panic::catch_unwind(AssertUnwindSafe(|| assert_json_schema(
            crate::schemas::DEFMODULE,
            &module_json
        )))
        .is_err(),
        "patternProperties should reject leading digit"
    );
}
