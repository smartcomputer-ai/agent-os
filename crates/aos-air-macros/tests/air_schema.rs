use aos_air_types::{AirNode, ModuleRuntime, TypeExpr, TypePrimitive};
use aos_wasm_sdk::{AirSchema, AirSchemaExport, AirWorkflowExport};

aos_wasm_sdk::aos_air_exports! {
    const TEST_SCHEMA_AIR_NODES_JSON = [TaskSubmitted, Composite];
}

aos_wasm_sdk::aos_air_exports! {
    const TEST_ALL_AIR_NODES_JSON = {
        schemas: [TaskSubmitted, Composite],
        workflows: [CounterWorkflow],
    };
}

#[derive(AirSchema)]
#[aos(schema = "demo/TaskSubmitted@1")]
#[allow(dead_code)]
struct TaskSubmitted {
    task: String,
}

#[derive(AirSchema)]
#[aos(schema = "demo/Composite@1")]
#[allow(dead_code)]
struct Composite {
    #[aos(schema_ref = "demo/TaskSubmitted@1")]
    parent: String,
    #[aos(air_type = "time")]
    observed_at_ns: u64,
    labels: Vec<String>,
    retry_count: Option<u64>,
}

#[aos_air_macros::aos_workflow(
    name = "demo/Counter@1",
    module = "demo/Counter_wasm@1",
    state = "demo/CounterState@1",
    event = "demo/CounterEvent@1",
    context = "sys/WorkflowContext@1",
    key_schema = "demo/CounterId@1",
    effects = ["sys/timer.set@1"]
)]
#[allow(dead_code)]
struct CounterWorkflow;

#[test]
fn derive_air_schema_emits_parseable_defschema() {
    let node: AirNode =
        serde_json::from_str(TaskSubmitted::AIR_SCHEMA_JSON).expect("parse generated AIR");
    let AirNode::Defschema(schema) = node else {
        panic!("expected defschema");
    };
    assert_eq!(schema.name, "demo/TaskSubmitted@1");
    let TypeExpr::Record(record) = schema.ty else {
        panic!("expected record schema");
    };
    assert!(matches!(
        record.record.get("task"),
        Some(TypeExpr::Primitive(TypePrimitive::Text(_)))
    ));
}

#[test]
fn derive_air_schema_supports_first_record_subset() {
    let node: AirNode =
        serde_json::from_str(Composite::AIR_SCHEMA_JSON).expect("parse generated AIR");
    let AirNode::Defschema(schema) = node else {
        panic!("expected defschema");
    };
    let TypeExpr::Record(record) = schema.ty else {
        panic!("expected record schema");
    };
    assert!(matches!(
        record.record.get("parent"),
        Some(TypeExpr::Ref(_))
    ));
    assert!(matches!(
        record.record.get("observed_at_ns"),
        Some(TypeExpr::Primitive(TypePrimitive::Time(_)))
    ));
    assert!(matches!(
        record.record.get("labels"),
        Some(TypeExpr::List(_))
    ));
    assert!(matches!(
        record.record.get("retry_count"),
        Some(TypeExpr::Option(_))
    ));
}

#[test]
fn aos_air_exports_collects_generated_json_constants() {
    assert_eq!(TEST_SCHEMA_AIR_NODES_JSON.len(), 2);
    assert!(TEST_SCHEMA_AIR_NODES_JSON[0].contains(r#""name":"demo/TaskSubmitted@1""#));
    assert!(TEST_SCHEMA_AIR_NODES_JSON[1].contains(r#""name":"demo/Composite@1""#));
}

#[test]
fn aos_workflow_emits_parseable_defmodule() {
    let node: AirNode =
        serde_json::from_str(CounterWorkflow::AIR_MODULE_JSON).expect("parse generated AIR");
    let AirNode::Defmodule(module) = node else {
        panic!("expected defmodule");
    };
    assert_eq!(module.name, "demo/Counter_wasm@1");
    assert!(matches!(module.runtime, ModuleRuntime::Wasm { .. }));
}

#[test]
fn aos_workflow_emits_parseable_defworkflow() {
    let node: AirNode =
        serde_json::from_str(CounterWorkflow::AIR_WORKFLOW_JSON).expect("parse generated AIR");
    let AirNode::Defworkflow(workflow) = node else {
        panic!("expected defworkflow");
    };
    assert_eq!(workflow.name, "demo/Counter@1");
    assert_eq!(workflow.state.as_str(), "demo/CounterState@1");
    assert_eq!(workflow.event.as_str(), "demo/CounterEvent@1");
    assert_eq!(
        workflow.context.as_ref().map(|schema| schema.as_str()),
        Some("sys/WorkflowContext@1")
    );
    assert_eq!(
        workflow.key_schema.as_ref().map(|schema| schema.as_str()),
        Some("demo/CounterId@1")
    );
    assert_eq!(workflow.effects_emitted, vec!["sys/timer.set@1"]);
    assert_eq!(workflow.implementation.module, "demo/Counter_wasm@1");
    assert_eq!(workflow.implementation.entrypoint, "step");
}

#[test]
fn aos_air_exports_collects_schema_and_workflow_json_constants() {
    assert_eq!(TEST_ALL_AIR_NODES_JSON.len(), 4);
    assert!(TEST_ALL_AIR_NODES_JSON[0].contains(r#""name":"demo/TaskSubmitted@1""#));
    assert!(TEST_ALL_AIR_NODES_JSON[1].contains(r#""name":"demo/Composite@1""#));
    assert!(TEST_ALL_AIR_NODES_JSON[2].contains(r#""name":"demo/Counter_wasm@1""#));
    assert!(TEST_ALL_AIR_NODES_JSON[3].contains(r#""name":"demo/Counter@1""#));
}
