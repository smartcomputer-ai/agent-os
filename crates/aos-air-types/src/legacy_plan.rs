use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{CapGrantName, EffectKind, Expr, ExprOrValue, Name, SchemaRef, VarName};

pub type StepId = String;

// Legacy plan/trigger data types retained for transitional kernel compatibility.
// These are no longer part of the active AIR manifest/node model.
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
    SpawnPlan(PlanStepSpawnPlan),
    AwaitPlan(PlanStepAwaitPlan),
    SpawnForEach(PlanStepSpawnForEach),
    AwaitPlansAll(PlanStepAwaitPlansAll),
    Assign(PlanStepAssign),
    End(PlanStepEnd),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepRaiseEvent {
    pub event: SchemaRef,
    pub value: ExprOrValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepEmitEffect {
    pub kind: EffectKind,
    pub params: ExprOrValue,
    pub cap: CapGrantName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<ExprOrValue>,
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
pub struct PlanStepSpawnPlan {
    pub plan: Name,
    pub input: ExprOrValue,
    pub bind: PlanBindHandle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepAwaitPlan {
    #[serde(rename = "for")]
    pub for_expr: Expr,
    pub bind: PlanBind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepSpawnForEach {
    pub plan: Name,
    pub inputs: ExprOrValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fanout: Option<u64>,
    pub bind: PlanBindHandles,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepAwaitPlansAll {
    pub handles: Expr,
    pub bind: PlanBindResults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepAssign {
    pub expr: ExprOrValue,
    pub bind: PlanBind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepEnd {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ExprOrValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanBind {
    #[serde(rename = "as")]
    pub var: VarName,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanBindHandle {
    pub handle_as: VarName,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanBindHandles {
    pub handles_as: VarName,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanBindResults {
    pub results_as: VarName,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trigger {
    pub event: SchemaRef,
    pub plan: Name,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlate_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Expr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_expr: Option<ExprOrValue>,
}
