use aos_air_types::{AirNode, ModuleRuntime, TypeExpr, TypePrimitive};
use aos_wasm_sdk::{AirSchema, AirSchemaExport, AirType, AirWorkflowExport};

aos_wasm_sdk::aos_air_exports! {
    const TEST_SCHEMA_AIR_NODES_JSON = [TaskSubmitted];
}

aos_wasm_sdk::aos_air_exports! {
    const TEST_ALL_AIR_NODES_JSON = {
        schemas: [TaskSubmitted],
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
    #[aos(schema_ref = TaskSubmitted)]
    parent: String,
    optional_parent: Option<TaskSubmitted>,
    #[aos(air_type = "time")]
    observed_at_ns: u64,
    #[aos(air_type = "hash")]
    output_ref: Option<String>,
    labels: Vec<String>,
    retry_count: Option<u64>,
}

#[derive(AirSchema)]
#[aos(schema = "demo/NewtypeId@1", air_type = "uuid")]
#[allow(dead_code)]
struct NewtypeId(String);

#[derive(AirSchema)]
#[aos(schema = "demo/RichRecord@1")]
#[allow(dead_code)]
struct RichRecord {
    #[aos(air_type = "hash")]
    refs: Option<Vec<String>>,
    #[aos(map_key_air_type = "nat", air_type = "hash")]
    refs_by_index: std::collections::BTreeMap<u64, String>,
    #[aos(schema_ref = aos_wasm_sdk::PendingEffectSet<String>)]
    pending: aos_wasm_sdk::PendingEffectSet<String>,
}

#[derive(AirType)]
#[allow(dead_code)]
enum InlineCommandKind {
    Steer { text: String },
    Noop,
}

#[derive(AirSchema)]
#[aos(schema = "demo/InlineCommand@1")]
#[allow(dead_code)]
struct InlineCommand {
    #[aos(inline)]
    command: InlineCommandKind,
}

#[derive(AirSchema)]
#[aos(schema = "demo/CounterState@1")]
#[allow(dead_code)]
struct CounterState {
    value: u64,
}

#[derive(AirSchema)]
#[aos(schema = "demo/CounterEvent@1")]
#[allow(dead_code)]
struct CounterEvent {
    delta: i64,
}

#[derive(AirSchema)]
#[aos(schema = "demo/CounterId@1", air_type = "uuid")]
#[allow(dead_code)]
struct CounterId(String);

#[derive(AirSchema)]
#[aos(schema = "demo/Event@1")]
#[allow(dead_code)]
enum DemoEvent {
    Created,
    Submitted(TaskSubmitted),
    Failed {
        code: String,
        detail: String,
    },
    #[aos(schema_ref = TaskSubmitted)]
    ExternalNoop,
}

#[derive(AirSchema)]
#[aos(schema = "demo/SysEvent@1")]
#[allow(dead_code)]
enum SysEvent {
    Receipt(aos_wasm_sdk::EffectReceiptEnvelope),
    BlobPut(aos_wasm_sdk::BlobPutParams),
}

#[aos_wasm_sdk::air_workflow(
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

#[aos_wasm_sdk::air_workflow(
    name = "demo/TypedCounter@1",
    module = "demo/TypedCounter_wasm@1",
    state = CounterState,
    event = CounterEvent,
    context = aos_wasm_sdk::WorkflowContext,
    key_schema = CounterId,
    effects = [aos_wasm_sdk::TimerSetParams]
)]
#[allow(dead_code)]
struct TypedCounterWorkflow;

#[test]
fn derive_air_schema_emits_parseable_defschema() {
    let node: AirNode =
        serde_json::from_str(&TaskSubmitted::air_schema_json()).expect("parse generated AIR");
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
        serde_json::from_str(&Composite::air_schema_json()).expect("parse generated AIR");
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
        record.record.get("optional_parent"),
        Some(TypeExpr::Option(_))
    ));
    assert!(matches!(
        record.record.get("observed_at_ns"),
        Some(TypeExpr::Primitive(TypePrimitive::Time(_)))
    ));
    assert!(matches!(
        record.record.get("output_ref"),
        Some(TypeExpr::Option(_))
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
fn derive_air_schema_supports_newtype_and_raw_type_overrides() {
    let node: AirNode =
        serde_json::from_str(&NewtypeId::air_schema_json()).expect("parse generated AIR");
    let AirNode::Defschema(schema) = node else {
        panic!("expected defschema");
    };
    assert!(matches!(
        schema.ty,
        TypeExpr::Primitive(TypePrimitive::Uuid(_))
    ));

    let node: AirNode =
        serde_json::from_str(&RichRecord::air_schema_json()).expect("parse generated AIR");
    let AirNode::Defschema(schema) = node else {
        panic!("expected defschema");
    };
    let TypeExpr::Record(record) = schema.ty else {
        panic!("expected record schema");
    };
    assert!(matches!(
        record.record.get("refs"),
        Some(TypeExpr::Option(option))
            if matches!(option.option.as_ref(), TypeExpr::List(list)
                if matches!(list.list.as_ref(), TypeExpr::Primitive(TypePrimitive::Hash(_))))
    ));
    assert!(matches!(
        record.record.get("refs_by_index"),
        Some(TypeExpr::Map(map))
            if matches!(map.map.key, aos_air_types::TypeMapKey::Nat(_))
                && matches!(map.map.value.as_ref(), TypeExpr::Primitive(TypePrimitive::Hash(_)))
    ));
    let Some(TypeExpr::Ref(pending_ref)) = record.record.get("pending") else {
        panic!("expected pending ref");
    };
    assert_eq!(pending_ref.reference.as_str(), "sys/PendingEffectSetText@1");
}

#[test]
fn derive_air_schema_supports_unit_and_ref_variants() {
    let node: AirNode =
        serde_json::from_str(&DemoEvent::air_schema_json()).expect("parse generated AIR");
    let AirNode::Defschema(schema) = node else {
        panic!("expected defschema");
    };
    let TypeExpr::Variant(variant) = schema.ty else {
        panic!("expected variant schema");
    };
    assert!(matches!(
        variant.variant.get("Created"),
        Some(TypeExpr::Primitive(TypePrimitive::Unit(_)))
    ));
    assert!(matches!(
        variant.variant.get("Submitted"),
        Some(TypeExpr::Ref(_))
    ));
    assert!(matches!(
        variant.variant.get("Failed"),
        Some(TypeExpr::Record(record))
            if matches!(
                record.record.get("code"),
                Some(TypeExpr::Primitive(TypePrimitive::Text(_)))
            )
    ));
    assert!(matches!(
        variant.variant.get("ExternalNoop"),
        Some(TypeExpr::Ref(_))
    ));
}

#[test]
fn derive_air_schema_supports_inline_field_types() {
    let node: AirNode =
        serde_json::from_str(&InlineCommand::air_schema_json()).expect("parse generated AIR");
    let AirNode::Defschema(schema) = node else {
        panic!("expected defschema");
    };
    let TypeExpr::Record(record) = schema.ty else {
        panic!("expected record schema");
    };
    assert!(matches!(
        record.record.get("command"),
        Some(TypeExpr::Variant(variant))
            if matches!(
                variant.variant.get("Steer"),
                Some(TypeExpr::Record(record))
                    if matches!(
                        record.record.get("text"),
                        Some(TypeExpr::Primitive(TypePrimitive::Text(_)))
                    )
            )
    ));
}

#[test]
fn derive_air_schema_infers_sys_refs_from_sdk_types() {
    let node: AirNode =
        serde_json::from_str(&SysEvent::air_schema_json()).expect("parse generated AIR");
    let AirNode::Defschema(schema) = node else {
        panic!("expected defschema");
    };
    let TypeExpr::Variant(variant) = schema.ty else {
        panic!("expected variant schema");
    };
    let Some(TypeExpr::Ref(receipt_ref)) = variant.variant.get("Receipt") else {
        panic!("expected receipt ref");
    };
    assert_eq!(
        receipt_ref.reference.as_str(),
        "sys/EffectReceiptEnvelope@1"
    );
    let Some(TypeExpr::Ref(blob_ref)) = variant.variant.get("BlobPut") else {
        panic!("expected blob ref");
    };
    assert_eq!(blob_ref.reference.as_str(), "sys/BlobPutParams@1");
}

#[test]
fn aos_air_exports_collects_generated_json_constants() {
    assert_eq!(TEST_SCHEMA_AIR_NODES_JSON.len(), 1);
    assert!(TEST_SCHEMA_AIR_NODES_JSON[0].contains(r#""name":"demo/TaskSubmitted@1""#));
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
fn aos_workflow_accepts_typed_schema_and_effect_refs() {
    let node: AirNode = serde_json::from_str(&TypedCounterWorkflow::air_workflow_json())
        .expect("parse generated AIR");
    let AirNode::Defworkflow(workflow) = node else {
        panic!("expected defworkflow");
    };
    assert_eq!(workflow.name, "demo/TypedCounter@1");
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
}

#[test]
fn aos_air_exports_collects_schema_and_workflow_json_constants() {
    assert_eq!(TEST_ALL_AIR_NODES_JSON.len(), 3);
    assert!(TEST_ALL_AIR_NODES_JSON[0].contains(r#""name":"demo/TaskSubmitted@1""#));
    assert!(TEST_ALL_AIR_NODES_JSON[1].contains(r#""name":"demo/Counter_wasm@1""#));
    assert!(TEST_ALL_AIR_NODES_JSON[2].contains(r#""name":"demo/Counter@1""#));
}
