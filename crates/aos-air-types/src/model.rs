use crate::{HashRef, SchemaRef};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub type Name = String;
pub type VarName = String;
pub type StepId = String;
pub type CapGrantName = String;

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text_type() -> TypeExpr {
        TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText { text: EmptyObject::default() }))
    }

    #[test]
    fn type_expr_matches_schema_shape() {
        let mut record = IndexMap::new();
        record.insert("id".to_string(), text_type());
        record.insert(
            "tags".to_string(),
            TypeExpr::Set(TypeSet { set: Box::new(text_type()) }),
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
                Expr::Const(ExprConst::Text { text: "hello".into() }),
                Expr::Const(ExprConst::Text { text: "world".into() }),
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
    fn plan_step_round_trip() {
        let json_value = json!({
            "id": "emit",
            "op": "emit_effect",
            "kind": "http.request",
            "params": {"record": {}},
            "cap": "http_cap",
            "bind": {"effect_id_as": "req"}
        });
        let step: PlanStep = serde_json::from_value(json_value.clone()).expect("deserialize");
        let back = serde_json::to_value(step).expect("serialize");
        assert_eq!(json_value, back);
    }

    #[test]
    fn value_literal_serialization() {
        let mut record = IndexMap::new();
        record.insert(
            "name".into(),
            ValueLiteral::Text(ValueText { text: "demo".into() }),
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
}

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
pub struct ExprRef {
    #[serde(rename = "ref")]
    pub reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExprConst {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    Defplan(DefPlan),
    Defcap(DefCap),
    Defpolicy(DefPolicy),
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
    pub module_kind: ModuleKind,
    pub wasm_hash: HashRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_schema: Option<SchemaRef>,
    pub abi: ModuleAbi,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ModuleKind {
    Reducer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleAbi {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reducer: Option<ReducerAbi>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReducerAbi {
    pub state: SchemaRef,
    pub event: SchemaRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<SchemaRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects_emitted: Vec<EffectKind>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub cap_slots: IndexMap<VarName, CapType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefPlan {
    pub name: Name,
    pub input: SchemaRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<SchemaRef>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub locals: IndexMap<VarName, SchemaRef>,
    pub steps: Vec<PlanStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<PlanEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_caps: Vec<CapGrantName>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_effects: Vec<EffectKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invariants: Vec<Expr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanEdge {
    pub from: StepId,
    pub to: StepId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Expr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: StepId,
    #[serde(flatten)]
    pub kind: PlanStepKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlanStepKind {
    RaiseEvent(PlanStepRaiseEvent),
    EmitEffect(PlanStepEmitEffect),
    AwaitReceipt(PlanStepAwaitReceipt),
    AwaitEvent(PlanStepAwaitEvent),
    Assign(PlanStepAssign),
    End(PlanStepEnd),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepRaiseEvent {
    pub reducer: Name,
    pub event: Expr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Expr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepEmitEffect {
    pub kind: EffectKind,
    pub params: Expr,
    pub cap: CapGrantName,
    pub bind: PlanBindEffect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanBindEffect {
    #[serde(rename = "effect_id_as")]
    pub effect_id_as: VarName,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepAwaitReceipt {
    #[serde(rename = "for")]
    pub for_expr: Expr,
    pub bind: PlanBind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepAwaitEvent {
    pub event: SchemaRef,
    #[serde(rename = "where", default, skip_serializing_if = "Option::is_none")]
    pub where_clause: Option<Expr>,
    pub bind: PlanBind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepAssign {
    pub expr: Expr,
    pub bind: PlanBind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepEnd {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Expr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanBind {
    #[serde(rename = "as")]
    pub var: VarName,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefCap {
    pub name: Name,
    pub cap_type: CapType,
    pub schema: TypeExpr,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CapType {
    #[serde(rename = "http.out")]
    HttpOut,
    #[serde(rename = "fs.blob")]
    FsBlob,
    Timer,
    #[serde(rename = "llm.basic")]
    LlmBasic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefPolicy {
    pub name: Name,
    pub rules: Vec<PolicyRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    pub when: PolicyMatch,
    pub decision: PolicyDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PolicyDecision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PolicyMatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_kind: Option<EffectKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap_name: Option<CapGrantName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_kind: Option<OriginKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_name: Option<Name>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OriginKind {
    Plan,
    Reducer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schemas: Vec<NamedRef>,
    pub modules: Vec<NamedRef>,
    pub plans: Vec<NamedRef>,
    pub caps: Vec<NamedRef>,
    pub policies: Vec<NamedRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaults: Option<ManifestDefaults>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub module_bindings: IndexMap<Name, ModuleBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<Routing>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<Trigger>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedRef {
    pub name: Name,
    pub hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<Name>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cap_grants: Vec<CapGrant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleBinding {
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub slots: IndexMap<VarName, CapGrantName>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Routing {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<RoutingEvent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inboxes: Vec<InboxRoute>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingEvent {
    pub event: SchemaRef,
    pub reducer: Name,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_field: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxRoute {
    pub source: String,
    pub reducer: Name,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trigger {
    pub event: SchemaRef,
    pub plan: Name,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlate_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapGrant {
    pub name: CapGrantName,
    pub cap: Name,
    pub params: ValueLiteral,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<CapGrantBudget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapGrantBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cents: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum EffectKind {
    #[serde(rename = "http.request")]
    HttpRequest,
    #[serde(rename = "fs.blob.put")]
    FsBlobPut,
    #[serde(rename = "fs.blob.get")]
    FsBlobGet,
    #[serde(rename = "timer.set")]
    TimerSet,
    #[serde(rename = "llm.generate")]
    LlmGenerate,
}
