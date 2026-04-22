use aos_air_types::{AirNode, TypeExpr, TypePrimitive};
use aos_wasm_sdk::{AirSchema, AirSchemaExport};

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
