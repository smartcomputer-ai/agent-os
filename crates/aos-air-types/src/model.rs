use crate::{HashRef, SchemaRef, SecretRef};
use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value as JsonValue;

pub type Name = String;
pub type VarName = String;
pub type SecretAlias = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ValueLiteral {
    Null(ValueNull),
    Bool(ValueBool),
    Int(ValueInt),
    Nat(ValueNat),
    Dec128(ValueDec128),
    Bytes(ValueBytes),
    Text(ValueText),
    TimeNs(ValueTimeNs),
    DurationNs(ValueDurationNs),
    Hash(ValueHash),
    Uuid(ValueUuid),
    List(ValueList),
    Set(ValueSet),
    Map(ValueMap),
    Record(ValueRecord),
    Variant(ValueVariant),
    SecretRef(SecretRef),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueNull {
    pub null: EmptyObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueBool {
    pub bool: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueInt {
    pub int: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueNat {
    pub nat: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueDec128 {
    pub dec128: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueBytes {
    pub bytes_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueText {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueTimeNs {
    pub time_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueDurationNs {
    pub duration_ns: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueHash {
    pub hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueUuid {
    pub uuid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueList {
    pub list: Vec<ValueLiteral>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueSet {
    pub set: Vec<ValueLiteral>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueMap {
    pub map: Vec<ValueMapEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueRecord {
    pub record: IndexMap<String, ValueLiteral>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueMapEntry {
    pub key: ValueLiteral,
    pub value: ValueLiteral,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueVariant {
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Box<ValueLiteral>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefSecret {
    pub name: Name,
    pub binding_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_digest: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TypeExpr {
    Primitive(TypePrimitive),
    Record(TypeRecord),
    Variant(TypeVariant),
    List(TypeList),
    Set(TypeSet),
    Map(TypeMap),
    Option(TypeOption),
    Ref(TypeRef),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TypePrimitive {
    Bool(TypePrimitiveBool),
    Int(TypePrimitiveInt),
    Nat(TypePrimitiveNat),
    Dec128(TypePrimitiveDec128),
    Bytes(TypePrimitiveBytes),
    Text(TypePrimitiveText),
    Time(TypePrimitiveTime),
    Duration(TypePrimitiveDuration),
    Hash(TypePrimitiveHash),
    Uuid(TypePrimitiveUuid),
    Unit(TypePrimitiveUnit),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypePrimitiveBool {
    pub bool: EmptyObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypePrimitiveInt {
    pub int: EmptyObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypePrimitiveNat {
    pub nat: EmptyObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypePrimitiveDec128 {
    pub dec128: EmptyObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypePrimitiveBytes {
    pub bytes: EmptyObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypePrimitiveText {
    pub text: EmptyObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypePrimitiveTime {
    pub time: EmptyObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypePrimitiveDuration {
    pub duration: EmptyObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypePrimitiveHash {
    pub hash: EmptyObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypePrimitiveUuid {
    pub uuid: EmptyObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypePrimitiveUnit {
    pub unit: EmptyObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeRecord {
    pub record: IndexMap<String, TypeExpr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeVariant {
    pub variant: IndexMap<String, TypeExpr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeList {
    pub list: Box<TypeExpr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeSet {
    pub set: Box<TypeExpr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeMap {
    pub map: TypeMapEntry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeMapEntry {
    pub key: TypeMapKey,
    pub value: Box<TypeExpr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TypeMapKey {
    Int(TypePrimitiveInt),
    Nat(TypePrimitiveNat),
    Text(TypePrimitiveText),
    Uuid(TypePrimitiveUuid),
    Hash(TypePrimitiveHash),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeOption {
    pub option: Box<TypeExpr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeRef {
    #[serde(rename = "ref")]
    pub reference: SchemaRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmptyObject {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Expr {
    Ref(ExprRef),
    Const(ExprConst),
    Op(ExprOp),
    Record(ExprRecord),
    List(ExprList),
    Set(ExprSet),
    Map(ExprMap),
    Variant(ExprVariant),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExprOrValue {
    Expr(Expr),
    Literal(ValueLiteral),
    Json(JsonValue),
}

impl From<Expr> for ExprOrValue {
    fn from(expr: Expr) -> Self {
        ExprOrValue::Expr(expr)
    }
}

impl From<&Expr> for ExprOrValue {
    fn from(expr: &Expr) -> Self {
        ExprOrValue::Expr(expr.clone())
    }
}

impl From<ValueLiteral> for ExprOrValue {
    fn from(value: ValueLiteral) -> Self {
        ExprOrValue::Literal(value)
    }
}

impl From<&ValueLiteral> for ExprOrValue {
    fn from(value: &ValueLiteral) -> Self {
        ExprOrValue::Literal(value.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExprRef {
    #[serde(rename = "ref")]
    pub reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExprConst {
    Null { null: EmptyObject },
    Bool { bool: bool },
    Int { int: i64 },
    Nat { nat: u64 },
    Dec128 { dec128: String },
    Text { text: String },
    Bytes { bytes_b64: String },
    Time { time_ns: u64 },
    Duration { duration_ns: i64 },
    Hash { hash: HashRef },
    Uuid { uuid: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExprOp {
    pub op: ExprOpCode,
    pub args: Vec<Expr>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExprOpCode {
    Len,
    Get,
    Has,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Not,
    Concat,
    Hash,
    HashBytes,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    StartsWith,
    EndsWith,
    Contains,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExprRecord {
    pub record: IndexMap<String, Expr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExprList {
    pub list: Vec<Expr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExprSet {
    pub set: Vec<Expr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExprMap {
    pub map: Vec<ExprMapEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExprMapEntry {
    pub key: Expr,
    pub value: Expr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExprVariant {
    pub variant: VariantExpr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantExpr {
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Box<Expr>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "$kind", rename_all = "lowercase")]
pub enum AirNode {
    Defschema(DefSchema),
    Defmodule(DefModule),
    Defworkflow(DefWorkflow),
    Defeffect(DefEffect),
    Defsecret(DefSecret),
    Manifest(Manifest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefSchema {
    pub name: Name,
    #[serde(rename = "type")]
    pub ty: TypeExpr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefModule {
    pub name: Name,
    pub runtime: ModuleRuntime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModuleRuntime {
    Wasm {
        artifact: WasmArtifact,
    },
    Python {
        python: String,
        artifact: PythonArtifact,
    },
    Builtin {},
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WasmArtifact {
    WasmModule {
        #[serde(default = "placeholder_wasm_hash")]
        hash: HashRef,
    },
}

fn placeholder_wasm_hash() -> HashRef {
    HashRef::new("sha256:0000000000000000000000000000000000000000000000000000000000000000")
        .expect("valid placeholder wasm hash")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PythonArtifact {
    PythonBundle {
        root_hash: HashRef,
    },
    WorkspaceRoot {
        root_hash: HashRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefWorkflow {
    pub name: Name,
    pub state: SchemaRef,
    pub event: SchemaRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<SchemaRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<SchemaRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_schema: Option<SchemaRef>,
    pub effects_emitted: Vec<Name>,
    #[serde(
        default = "default_workflow_determinism",
        skip_serializing_if = "is_strict_workflow_determinism"
    )]
    pub determinism: WorkflowDeterminism,
    #[serde(rename = "impl")]
    pub implementation: Impl,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefEffect {
    pub name: Name,
    pub params: SchemaRef,
    pub receipt: SchemaRef,
    #[serde(rename = "impl")]
    pub implementation: Impl,
}

fn default_workflow_determinism() -> WorkflowDeterminism {
    WorkflowDeterminism::Strict
}

fn is_strict_workflow_determinism(value: &WorkflowDeterminism) -> bool {
    matches!(value, WorkflowDeterminism::Strict)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowDeterminism {
    Strict,
    Checked,
    DecisionLog,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Impl {
    pub module: Name,
    pub entrypoint: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RootKind {
    Defschema,
    Defmodule,
    Defworkflow,
    Defeffect,
    Defsecret,
    Manifest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DefKind {
    Defschema,
    Defmodule,
    Defworkflow,
    Defeffect,
    Defsecret,
}

pub const CURRENT_AIR_VERSION: &str = "2";

#[derive(Debug, Clone, Serialize)]
pub struct Manifest {
    pub air_version: String,
    pub schemas: Vec<NamedRef>,
    pub modules: Vec<NamedRef>,
    /// Temporary runtime-only compatibility storage. Not part of public AIR v2.
    #[serde(default, skip)]
    pub ops: Vec<NamedRef>,
    #[serde(default)]
    pub workflows: Vec<NamedRef>,
    #[serde(default)]
    pub effects: Vec<NamedRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<NamedRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<Routing>,
}

impl<'de> Deserialize<'de> for Manifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ManifestWire {
            #[serde(rename = "$kind", default)]
            kind: Option<String>,
            air_version: String,
            schemas: Vec<NamedRef>,
            modules: Vec<NamedRef>,
            workflows: Vec<NamedRef>,
            effects: Vec<NamedRef>,
            #[serde(default)]
            secrets: Vec<NamedRef>,
            #[serde(default)]
            routing: Option<Routing>,
            #[serde(default)]
            ops: Option<de::IgnoredAny>,
        }

        let wire = ManifestWire::deserialize(deserializer)?;
        if let Some(kind) = wire.kind
            && kind != "manifest"
        {
            return Err(de::Error::custom(format!(
                "invalid manifest $kind '{kind}'"
            )));
        }
        if wire.ops.is_some() {
            return Err(de::Error::custom(
                "manifest.ops is not part of AIR v2; use workflows/effects",
            ));
        }
        Ok(Self {
            air_version: wire.air_version,
            schemas: wire.schemas,
            modules: wire.modules,
            ops: Vec::new(),
            workflows: wire.workflows,
            effects: wire.effects,
            secrets: wire.secrets,
            routing: wire.routing,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedRef {
    pub name: Name,
    pub hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Routing {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<RoutingSubscription>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingSubscription {
    pub event: SchemaRef,
    pub workflow: Name,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_field: Option<String>,
}

pub type RoutingEvent = RoutingSubscription;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text_type() -> TypeExpr {
        TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
            text: EmptyObject::default(),
        }))
    }

    #[test]
    fn type_expr_matches_schema_shape() {
        let mut record = IndexMap::new();
        record.insert("id".to_string(), text_type());
        record.insert(
            "tags".to_string(),
            TypeExpr::Set(TypeSet {
                set: Box::new(text_type()),
            }),
        );
        let ty = TypeExpr::Record(TypeRecord { record });
        let value = serde_json::to_value(ty).expect("serialize");
        assert_eq!(
            value,
            json!({
                "record": {
                    "id": {"text": {}},
                    "tags": {"set": {"text": {}}}
                }
            })
        );
    }

    #[test]
    fn expr_serializes_to_expected_shape() {
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::Concat,
            args: vec![
                Expr::Const(ExprConst::Text {
                    text: "hello".into(),
                }),
                Expr::Const(ExprConst::Text {
                    text: "world".into(),
                }),
            ],
        });
        let value = serde_json::to_value(expr).unwrap();
        assert_eq!(
            value,
            json!({
                "op": "concat",
                "args": [
                    {"text": "hello"},
                    {"text": "world"}
                ]
            })
        );
    }

    #[test]
    fn value_literal_serialization() {
        let mut record = IndexMap::new();
        record.insert(
            "name".into(),
            ValueLiteral::Text(ValueText {
                text: "demo".into(),
            }),
        );
        record.insert(
            "flags".into(),
            ValueLiteral::Set(ValueSet {
                set: vec![ValueLiteral::Text(ValueText { text: "a".into() })],
            }),
        );
        let value = ValueLiteral::Record(ValueRecord { record });
        let json_value = serde_json::to_value(&value).expect("serialize");
        assert_eq!(
            json_value,
            json!({
                "record": {
                    "name": {"text": "demo"},
                    "flags": {"set": [{"text": "a"}]}
                }
            })
        );
        let round_trip: ValueLiteral = serde_json::from_value(json_value).expect("deserialize");
        matches!(round_trip, ValueLiteral::Record(_));
    }

    #[test]
    fn manifest_round_trip() {
        let manifest_json = json!({
            "$kind": "manifest",
            "air_version": "2",
            "schemas": [
                {
                    "name": "com.acme/Order@1",
                    "hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                }
            ],
            "modules": [
                {
                    "name": "com.acme/order_wasm@1",
                    "hash": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                }
            ],
            "workflows": [
                {
                    "name": "com.acme/order.step@1",
                    "hash": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                }
            ],
            "effects": [],
            "routing": {
                "subscriptions": [
                    {
                        "event": "com.acme/OrderCreated@1",
                        "workflow": "com.acme/order.step@1"
                    }
                ]
            }
        });

        let node: AirNode = serde_json::from_value(manifest_json.clone()).expect("deserialize");
        let round_trip = serde_json::to_value(node).expect("serialize");
        assert_eq!(manifest_json, round_trip);
    }

    #[test]
    fn defeffect_round_trip() {
        let effect_json = json!({
            "$kind": "defeffect",
            "name": "com.acme/send@1",
            "params": "com.acme/SendParams@1",
            "receipt": "com.acme/SendReceipt@1",
            "impl": {
                "module": "com.acme/send_adapter@1",
                "entrypoint": "send"
            }
        });

        let node: AirNode = serde_json::from_value(effect_json.clone()).expect("deserialize");
        let round_trip = serde_json::to_value(node).expect("serialize");
        assert_eq!(effect_json, round_trip);
    }
}
