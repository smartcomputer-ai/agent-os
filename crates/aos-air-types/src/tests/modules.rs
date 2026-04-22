use serde_json::json;
use std::panic::{self, AssertUnwindSafe};

use super::assert_json_schema;
use crate::{DefModule, DefOp, ModuleRuntime, OpKind, SchemaRef, WorkflowDeterminism, WorkflowOp};

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
fn parses_workflow_op_with_effect_allowlist() {
    let op_json = json!({
        "$kind": "defop",
        "name": "com.acme/order.step@1",
        "op_kind": "workflow",
        "workflow": {
            "state": "com.acme/State@1",
            "event": "com.acme/Event@1",
            "key_schema": "com.acme/OrderId@1",
            "effects_emitted": ["sys/timer.set@1"]
        },
        "impl": {
            "module": "com.acme/order_wasm@1",
            "entrypoint": "order_step"
        }
    });
    assert_json_schema(crate::schemas::DEFOP, &op_json);
    let op: DefOp = serde_json::from_value(op_json).expect("parse op");
    assert_eq!(op.op_kind, OpKind::Workflow);
    let workflow = op.workflow.expect("workflow op");
    assert_eq!(workflow.state.as_str(), "com.acme/State@1");
    assert_eq!(workflow.effects_emitted, vec!["sys/timer.set@1"]);
}

#[test]
fn workflow_op_struct_round_trip() {
    let workflow = WorkflowOp {
        context: None,
        state: SchemaRef::new("com.acme/State@1").unwrap(),
        event: SchemaRef::new("com.acme/Event@1").unwrap(),
        annotations: None,
        key_schema: None,
        effects_emitted: Vec::new(),
        determinism: WorkflowDeterminism::Strict,
    };
    let json = serde_json::to_value(&workflow).expect("serialize");
    let round_trip: WorkflowOp = serde_json::from_value(json).expect("deserialize");
    assert_eq!(round_trip.effects_emitted.len(), 0);
}

#[test]
fn workflow_op_requires_effects_emitted() {
    let op_json = json!({
        "$kind": "defop",
        "name": "com.acme/order.step@1",
        "op_kind": "workflow",
        "workflow": {
            "state": "com.acme/State@1",
            "event": "com.acme/Event@1"
        },
        "impl": {
            "module": "com.acme/order_wasm@1",
            "entrypoint": "order_step"
        }
    });
    assert!(
        panic::catch_unwind(AssertUnwindSafe(|| assert_json_schema(
            crate::schemas::DEFOP,
            &op_json
        )))
        .is_err(),
        "canonical workflow ops must include effects_emitted"
    );
}

#[test]
fn parses_effect_op() {
    let op_json = json!({
        "$kind": "defop",
        "name": "com.acme/slack.post@1",
        "op_kind": "effect",
        "effect": {
            "params": "com.acme/SlackPostParams@1",
            "receipt": "com.acme/SlackPostReceipt@1"
        },
        "impl": {
            "module": "com.acme/order_bundle@1",
            "entrypoint": "orders.effects:post_to_slack"
        }
    });
    assert_json_schema(crate::schemas::DEFOP, &op_json);
    let op: DefOp = serde_json::from_value(op_json).expect("parse op");
    assert_eq!(op.op_kind, OpKind::Effect);
    assert_eq!(
        op.effect.unwrap().params.as_str(),
        "com.acme/SlackPostParams@1"
    );
}
