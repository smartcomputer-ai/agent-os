use serde_json::json;
use std::panic::{self, AssertUnwindSafe};

use super::assert_json_schema;
use crate::{
    AirNode, DefEffect, DefModule, DefWorkflow, ModuleRuntime, SchemaRef, WorkflowDeterminism,
};

#[test]
fn parses_wasm_module_runtime() {
    let module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/order_wasm@1",
        "runtime": {
            "kind": "wasm",
            "artifact": {
                "kind": "wasm_module",
                "hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
            }
        }
    });
    assert_json_schema(crate::schemas::DEFMODULE, &module_json);
    let module: DefModule = serde_json::from_value(module_json).expect("parse module");
    assert_eq!(module.name, "com.acme/order_wasm@1");
    assert!(matches!(module.runtime, ModuleRuntime::Wasm { .. }));
}

#[test]
fn parses_wasm_module_runtime_without_hash_as_placeholder() {
    let module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/order_wasm@1",
        "runtime": {
            "kind": "wasm",
            "artifact": {
                "kind": "wasm_module"
            }
        }
    });
    assert_json_schema(crate::schemas::DEFMODULE, &module_json);
    let module: DefModule = serde_json::from_value(module_json).expect("parse module");
    let ModuleRuntime::Wasm {
        artifact: crate::WasmArtifact::WasmModule { hash },
    } = module.runtime
    else {
        panic!("expected wasm module");
    };
    assert_eq!(
        hash.as_str(),
        "sha256:0000000000000000000000000000000000000000000000000000000000000000"
    );
}

#[test]
fn parses_builtin_module_runtime() {
    let module_json = json!({
        "$kind": "defmodule",
        "name": "sys/builtin_effects@1",
        "runtime": { "kind": "builtin" }
    });
    assert_json_schema(crate::schemas::DEFMODULE, &module_json);
    let module: DefModule = serde_json::from_value(module_json).expect("parse module");
    assert!(matches!(module.runtime, ModuleRuntime::Builtin { .. }));
}

#[test]
fn rejects_wasm_module_with_python_artifact() {
    let bad_module_json = json!({
        "$kind": "defmodule",
        "name": "com.acme/Workflow@1",
        "runtime": {
            "kind": "wasm",
            "artifact": {
                "kind": "python_bundle",
                "root_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
            }
        }
    });
    assert!(
        panic::catch_unwind(AssertUnwindSafe(|| assert_json_schema(
            crate::schemas::DEFMODULE,
            &bad_module_json
        )))
        .is_err(),
        "schema should reject artifact kind that runtime cannot load"
    );
}

#[test]
fn parses_workflow_with_effect_allowlist() {
    let workflow_json = json!({
        "$kind": "defworkflow",
        "name": "com.acme/order.step@1",
        "state": "com.acme/State@1",
        "event": "com.acme/Event@1",
        "key_schema": "com.acme/OrderId@1",
        "effects_emitted": ["sys/timer.set@1"],
        "impl": {
            "module": "com.acme/order_wasm@1",
            "entrypoint": "order_step"
        }
    });
    assert_json_schema(crate::schemas::DEFWORKFLOW, &workflow_json);
    let workflow: DefWorkflow = serde_json::from_value(workflow_json).expect("parse workflow");
    assert_eq!(workflow.state.as_str(), "com.acme/State@1");
    assert_eq!(workflow.effects_emitted, vec!["sys/timer.set@1"]);
}

#[test]
fn workflow_struct_round_trip() {
    let workflow = DefWorkflow {
        name: "com.acme/order.step@1".into(),
        context: None,
        state: SchemaRef::new("com.acme/State@1").unwrap(),
        event: SchemaRef::new("com.acme/Event@1").unwrap(),
        annotations: None,
        key_schema: None,
        effects_emitted: Vec::new(),
        determinism: WorkflowDeterminism::Strict,
        implementation: crate::Impl {
            module: "com.acme/order_wasm@1".into(),
            entrypoint: "order_step".into(),
        },
    };
    let json = serde_json::to_value(&workflow).expect("serialize");
    let round_trip: DefWorkflow = serde_json::from_value(json).expect("deserialize");
    assert_eq!(round_trip.effects_emitted.len(), 0);
}

#[test]
fn workflow_requires_effects_emitted() {
    let workflow_json = json!({
        "$kind": "defworkflow",
        "name": "com.acme/order.step@1",
        "state": "com.acme/State@1",
        "event": "com.acme/Event@1",
        "impl": {
            "module": "com.acme/order_wasm@1",
            "entrypoint": "order_step"
        }
    });
    assert!(
        panic::catch_unwind(AssertUnwindSafe(|| assert_json_schema(
            crate::schemas::DEFWORKFLOW,
            &workflow_json
        )))
        .is_err(),
        "canonical workflows must include effects_emitted"
    );
}

#[test]
fn rejects_defop_root_form() {
    let op_json = json!({
        "$kind": "defop",
        "name": "com.acme/order.step@1",
        "op_kind": "workflow",
        "workflow": {
            "state": "com.acme/State@1",
            "event": "com.acme/Event@1",
            "effects_emitted": []
        },
        "impl": {
            "module": "com.acme/order_wasm@1",
            "entrypoint": "order_step"
        }
    });
    assert!(
        serde_json::from_value::<AirNode>(op_json).is_err(),
        "defop is not a public AIR v2 root form"
    );
}

#[test]
fn parses_effect() {
    let effect_json = json!({
        "$kind": "defeffect",
        "name": "com.acme/slack.post@1",
        "params": "com.acme/SlackPostParams@1",
        "receipt": "com.acme/SlackPostReceipt@1",
        "impl": {
            "module": "com.acme/order_bundle@1",
            "entrypoint": "orders.effects:post_to_slack"
        }
    });
    assert_json_schema(crate::schemas::DEFEFFECT, &effect_json);
    let effect: DefEffect = serde_json::from_value(effect_json).expect("parse effect");
    assert_eq!(effect.params.as_str(), "com.acme/SlackPostParams@1");
}
