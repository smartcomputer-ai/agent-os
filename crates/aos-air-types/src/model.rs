use crate::{HashRef, SchemaRef};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

pub type Name = String;
pub type VarName = String;
pub type CapGrantName = String;
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct SecretRef {
    pub alias: SecretAlias,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefSecret {
    pub name: Name,
    pub binding_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_digest: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_caps: Vec<CapGrantName>,
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
    Defcap(DefCap),
    Defpolicy(DefPolicy),
    Defsecret(DefSecret),
    Defeffect(DefEffect),
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
    Workflow,
    Pure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleAbi {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<WorkflowAbi>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pure: Option<PureAbi>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowAbi {
    pub state: SchemaRef,
    pub event: SchemaRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<SchemaRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<SchemaRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects_emitted: Vec<EffectKind>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub cap_slots: IndexMap<VarName, CapType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PureAbi {
    pub input: SchemaRef,
    pub output: SchemaRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<SchemaRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefCap {
    pub name: Name,
    pub cap_type: CapType,
    pub schema: TypeExpr,
    #[serde(default = "default_cap_enforcer")]
    pub enforcer: CapEnforcer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapEnforcer {
    pub module: Name,
}

fn default_cap_enforcer() -> CapEnforcer {
    CapEnforcer {
        module: "sys/CapAllowAll@1".to_string(),
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OriginScope {
    Workflow,
    Plan,
    Both,
}

impl OriginScope {
    pub fn allows_plans(self) -> bool {
        matches!(self, OriginScope::Plan | OriginScope::Both)
    }

    pub fn allows_workflows(self) -> bool {
        matches!(self, OriginScope::Workflow | OriginScope::Both)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefEffect {
    pub name: Name,
    pub kind: EffectKind,
    pub params_schema: SchemaRef,
    pub receipt_schema: SchemaRef,
    pub cap_type: CapType,
    pub origin_scope: OriginScope,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct CapType(String);

impl CapType {
    pub const HTTP_OUT: &'static str = "http.out";
    pub const BLOB: &'static str = "blob";
    pub const TIMER: &'static str = "timer";
    pub const LLM_BASIC: &'static str = "llm.basic";
    pub const PROCESS: &'static str = "process";
    pub const SECRET: &'static str = "secret";
    pub const QUERY: &'static str = "query";
    pub const WORKSPACE: &'static str = "workspace";

    pub fn new(cap_type: impl Into<String>) -> Self {
        Self(cap_type.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn http_out() -> Self {
        Self::new(Self::HTTP_OUT)
    }

    pub fn blob() -> Self {
        Self::new(Self::BLOB)
    }

    pub fn timer() -> Self {
        Self::new(Self::TIMER)
    }

    pub fn llm_basic() -> Self {
        Self::new(Self::LLM_BASIC)
    }

    pub fn process() -> Self {
        Self::new(Self::PROCESS)
    }

    pub fn secret() -> Self {
        Self::new(Self::SECRET)
    }

    pub fn query() -> Self {
        Self::new(Self::QUERY)
    }

    pub fn workspace() -> Self {
        Self::new(Self::WORKSPACE)
    }
}

impl<S: Into<String>> From<S> for CapType {
    fn from(value: S) -> Self {
        CapType::new(value)
    }
}

impl std::fmt::Display for CapType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for CapType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(s.to_owned()))
    }
}

impl AsRef<str> for CapType {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
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
    pub cap_type: Option<CapType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_kind: Option<OriginKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_name: Option<Name>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OriginKind {
    Workflow,
    System,
    Governance,
}

pub const CURRENT_AIR_VERSION: &str = "1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub air_version: String,
    pub schemas: Vec<NamedRef>,
    pub modules: Vec<NamedRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<NamedRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effect_bindings: Vec<EffectBinding>,
    pub caps: Vec<NamedRef>,
    pub policies: Vec<NamedRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<SecretEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaults: Option<ManifestDefaults>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub module_bindings: IndexMap<Name, ModuleBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<Routing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectBinding {
    pub kind: EffectKind,
    pub adapter_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedRef {
    pub name: Name,
    pub hash: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SecretEntry {
    Ref(NamedRef),
    Decl(SecretDecl),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<Name>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cap_grants: Vec<CapGrant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretDecl {
    pub alias: SecretAlias,
    pub version: u64,
    pub binding_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_digest: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<SecretPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecretPolicy {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_caps: Vec<CapGrantName>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleBinding {
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub slots: IndexMap<VarName, CapGrantName>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Routing {
    #[serde(default, alias = "events", skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<RoutingSubscription>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inboxes: Vec<InboxRoute>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingSubscription {
    pub event: SchemaRef,
    pub module: Name,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_field: Option<String>,
}

pub type RoutingEvent = RoutingSubscription;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxRoute {
    pub source: String,
    pub workflow: Name,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapGrant {
    pub name: CapGrantName,
    pub cap: Name,
    pub params: ValueLiteral,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry_ns: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct EffectKind(String);

impl EffectKind {
    pub const HTTP_REQUEST: &'static str = "http.request";
    pub const BLOB_PUT: &'static str = "blob.put";
    pub const BLOB_GET: &'static str = "blob.get";
    pub const TIMER_SET: &'static str = "timer.set";
    pub const PROCESS_SESSION_OPEN: &'static str = "process.session.open";
    pub const PROCESS_EXEC: &'static str = "process.exec";
    pub const PROCESS_SESSION_SIGNAL: &'static str = "process.session.signal";
    pub const LLM_GENERATE: &'static str = "llm.generate";
    pub const VAULT_PUT: &'static str = "vault.put";
    pub const VAULT_ROTATE: &'static str = "vault.rotate";
    pub const INTROSPECT_MANIFEST: &'static str = "introspect.manifest";
    pub const INTROSPECT_WORKFLOW_STATE: &'static str = "introspect.workflow_state";
    pub const INTROSPECT_JOURNAL_HEAD: &'static str = "introspect.journal_head";
    pub const INTROSPECT_LIST_CELLS: &'static str = "introspect.list_cells";
    pub const WORKSPACE_RESOLVE: &'static str = "workspace.resolve";
    pub const WORKSPACE_EMPTY_ROOT: &'static str = "workspace.empty_root";
    pub const WORKSPACE_LIST: &'static str = "workspace.list";
    pub const WORKSPACE_READ_REF: &'static str = "workspace.read_ref";
    pub const WORKSPACE_READ_BYTES: &'static str = "workspace.read_bytes";
    pub const WORKSPACE_WRITE_BYTES: &'static str = "workspace.write_bytes";
    pub const WORKSPACE_REMOVE: &'static str = "workspace.remove";
    pub const WORKSPACE_DIFF: &'static str = "workspace.diff";
    pub const WORKSPACE_ANNOTATIONS_GET: &'static str = "workspace.annotations_get";
    pub const WORKSPACE_ANNOTATIONS_SET: &'static str = "workspace.annotations_set";

    pub fn new(kind: impl Into<String>) -> Self {
        Self(kind.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn http_request() -> Self {
        Self::new(Self::HTTP_REQUEST)
    }

    pub fn blob_put() -> Self {
        Self::new(Self::BLOB_PUT)
    }

    pub fn blob_get() -> Self {
        Self::new(Self::BLOB_GET)
    }

    pub fn timer_set() -> Self {
        Self::new(Self::TIMER_SET)
    }

    pub fn process_session_open() -> Self {
        Self::new(Self::PROCESS_SESSION_OPEN)
    }

    pub fn process_exec() -> Self {
        Self::new(Self::PROCESS_EXEC)
    }

    pub fn process_session_signal() -> Self {
        Self::new(Self::PROCESS_SESSION_SIGNAL)
    }

    pub fn llm_generate() -> Self {
        Self::new(Self::LLM_GENERATE)
    }

    pub fn vault_put() -> Self {
        Self::new(Self::VAULT_PUT)
    }

    pub fn vault_rotate() -> Self {
        Self::new(Self::VAULT_ROTATE)
    }

    pub fn introspect_manifest() -> Self {
        Self::new(Self::INTROSPECT_MANIFEST)
    }

    pub fn introspect_workflow_state() -> Self {
        Self::new(Self::INTROSPECT_WORKFLOW_STATE)
    }

    pub fn introspect_journal_head() -> Self {
        Self::new(Self::INTROSPECT_JOURNAL_HEAD)
    }

    pub fn introspect_list_cells() -> Self {
        Self::new(Self::INTROSPECT_LIST_CELLS)
    }

    pub fn workspace_resolve() -> Self {
        Self::new(Self::WORKSPACE_RESOLVE)
    }

    pub fn workspace_empty_root() -> Self {
        Self::new(Self::WORKSPACE_EMPTY_ROOT)
    }

    pub fn workspace_list() -> Self {
        Self::new(Self::WORKSPACE_LIST)
    }

    pub fn workspace_read_ref() -> Self {
        Self::new(Self::WORKSPACE_READ_REF)
    }

    pub fn workspace_read_bytes() -> Self {
        Self::new(Self::WORKSPACE_READ_BYTES)
    }

    pub fn workspace_write_bytes() -> Self {
        Self::new(Self::WORKSPACE_WRITE_BYTES)
    }

    pub fn workspace_remove() -> Self {
        Self::new(Self::WORKSPACE_REMOVE)
    }

    pub fn workspace_diff() -> Self {
        Self::new(Self::WORKSPACE_DIFF)
    }

    pub fn workspace_annotations_get() -> Self {
        Self::new(Self::WORKSPACE_ANNOTATIONS_GET)
    }

    pub fn workspace_annotations_set() -> Self {
        Self::new(Self::WORKSPACE_ANNOTATIONS_SET)
    }
}

impl<S: Into<String>> From<S> for EffectKind {
    fn from(value: S) -> Self {
        EffectKind::new(value)
    }
}

impl std::fmt::Display for EffectKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EffectKind {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(s.to_owned()))
    }
}

impl AsRef<str> for EffectKind {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

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
            "air_version": "1",
            "schemas": [
                {
                    "name": "com.acme/Order@1",
                    "hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                }
            ],
            "modules": [
                {
                    "name": "com.acme/order_workflow@1",
                    "hash": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                }
            ],
            "caps": [],
            "policies": [],
            "routing": {
                "subscriptions": [
                    {
                        "event": "com.acme/OrderCreated@1",
                        "module": "com.acme/order_workflow@1"
                    }
                ]
            }
        });

        let node: AirNode = serde_json::from_value(manifest_json.clone()).expect("deserialize");
        let round_trip = serde_json::to_value(node).expect("serialize");
        assert_eq!(manifest_json, round_trip);
    }
}
