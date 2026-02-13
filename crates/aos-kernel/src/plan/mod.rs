use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use aos_air_exec::{
    Env as ExprEnv, Value as ExprValue, ValueKey, ValueMap as ExecValueMap,
    ValueSet as ExecValueSet, eval_expr,
};
use aos_air_types::plan_literals::{SchemaIndex, canonicalize_literal, validate_literal};
use aos_air_types::{
    DefPlan, EmptyObject, Expr, ExprOrValue, HashRef, PlanEdge, PlanStep, PlanStepKind, TypeExpr,
    TypePrimitive, TypePrimitiveInt, TypePrimitiveText, TypeRecord, ValueBool, ValueBytes,
    ValueDec128, ValueDurationNs, ValueHash, ValueInt, ValueList, ValueLiteral, ValueMap,
    ValueMapEntry, ValueNat, ValueNull, ValueRecord, ValueSet, ValueText, ValueTimeNs, ValueUuid,
    ValueVariant, catalog::EffectCatalog, value_normalize::normalize_cbor_by_name,
};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effects::EffectIntent;
use aos_wasm_abi::DomainEvent;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_cbor::{self, Value as CborValue};

use crate::capability::CapGrantResolution;
use crate::effects::EffectManager;
use crate::error::KernelError;
use crate::event::IngressStamp;
use crate::schema_value::cbor_to_expr_value;

mod step_handlers;
use self::step_handlers::StepTickControl;

#[derive(Default)]
pub struct PlanRegistry {
    plans: HashMap<String, DefPlan>,
}

impl PlanRegistry {
    pub fn register(&mut self, plan: DefPlan) {
        self.plans.insert(plan.name.clone(), plan);
    }

    pub fn get(&self, name: &str) -> Option<&DefPlan> {
        self.plans.get(name)
    }
}

#[derive(Clone)]
pub struct ReducerSchema {
    pub event_schema_name: String,
    pub event_schema: TypeExpr,
    pub key_schema: Option<TypeExpr>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Dependency {
    pred: String,
    guard: Option<aos_air_types::Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepState {
    Pending,
    WaitingReceipt,
    WaitingEvent,
    Completed,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepReadiness {
    Ready,
    Blocked,
    Skip,
}

pub struct PlanInstance {
    pub id: u64,
    pub name: String,
    pub plan: DefPlan,
    pub env: ExprEnv,
    pub completed: bool,
    context: Option<PlanContext>,
    effect_handles: HashMap<String, [u8; 32]>,
    receipt_waits: BTreeMap<[u8; 32], ReceiptWait>,
    receipt_values: HashMap<String, ExprValue>,
    event_wait: Option<EventWait>,
    event_value: Option<ExprValue>,
    correlation_id: Option<Vec<u8>>,
    step_map: HashMap<String, PlanStep>,
    step_order: Vec<String>,
    predecessors: HashMap<String, Vec<Dependency>>,
    step_states: HashMap<String, StepState>,
    schema_index: Arc<SchemaIndex>,
    cap_handles: Arc<HashMap<String, CapGrantResolution>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReceiptWait {
    pub step_id: String,
    pub intent_hash: [u8; 32],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventWait {
    pub step_id: String,
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub where_clause: Option<Expr>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanContext {
    pub logical_now_ns: u64,
    pub journal_height: u64,
    pub manifest_hash: String,
}

impl PlanContext {
    pub fn from_stamp(stamp: &IngressStamp) -> Self {
        Self {
            logical_now_ns: stamp.logical_now_ns,
            journal_height: stamp.journal_height,
            manifest_hash: stamp.manifest_hash.clone(),
        }
    }
}

#[derive(Default, Debug)]
pub struct PlanTickOutcome {
    pub raised_events: Vec<DomainEvent>,
    pub waiting_receipts: Vec<[u8; 32]>,
    pub waiting_event: Option<String>,
    pub completed: bool,
    pub intents_enqueued: Vec<EffectIntent>,
    pub result: Option<ExprValue>,
    pub result_schema: Option<String>,
    pub result_cbor: Option<Vec<u8>>,
    pub plan_error: Option<PlanError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanError {
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanInstanceSnapshot {
    pub id: u64,
    pub name: String,
    pub env: ExprEnv,
    pub completed: bool,
    pub context: Option<PlanContext>,
    pub effect_handles: Vec<(String, [u8; 32])>,
    pub receipt_waits: Vec<ReceiptWait>,
    pub receipt_values: Vec<(String, ExprValue)>,
    pub event_wait: Option<EventWait>,
    pub event_value: Option<ExprValue>,
    pub correlation_id: Option<Vec<u8>>,
    pub step_states: Vec<(String, StepState)>,
}

impl PlanInstance {
    pub fn new(
        id: u64,
        plan: DefPlan,
        input: ExprValue,
        schema_index: Arc<SchemaIndex>,
        correlation: Option<(Vec<u8>, ExprValue)>,
        cap_handles: Arc<HashMap<String, CapGrantResolution>>,
    ) -> Self {
        let mut step_map = HashMap::new();
        let mut step_order = Vec::new();
        for step in &plan.steps {
            step_order.push(step.id.clone());
            step_map.insert(step.id.clone(), step.clone());
        }
        let mut predecessors: HashMap<String, Vec<Dependency>> = HashMap::new();
        for PlanEdge { from, to, when } in &plan.edges {
            predecessors
                .entry(to.clone())
                .or_default()
                .push(Dependency {
                    pred: from.clone(),
                    guard: when.clone(),
                });
        }
        let mut step_states = HashMap::new();
        for id in &step_order {
            step_states.insert(id.clone(), StepState::Pending);
        }
        let mut env = ExprEnv {
            plan_input: input,
            vars: IndexMap::new(),
            steps: IndexMap::new(),
            current_event: None,
        };
        let mut correlation_id = None;
        if let Some((bytes, value)) = correlation {
            env.vars.insert("correlation_id".into(), value);
            correlation_id = Some(bytes);
        }
        Self {
            id,
            name: plan.name.clone(),
            plan,
            env,
            completed: false,
            context: None,
            effect_handles: HashMap::new(),
            receipt_waits: BTreeMap::new(),
            receipt_values: HashMap::new(),
            event_wait: None,
            event_value: None,
            correlation_id,
            step_map,
            step_order,
            predecessors,
            step_states,
            schema_index,
            cap_handles,
        }
    }

    pub fn tick(&mut self, effects: &mut EffectManager) -> Result<PlanTickOutcome, KernelError> {
        let mut outcome = PlanTickOutcome::default();
        if self.completed {
            outcome.completed = true;
            return Ok(outcome);
        }

        loop {
            let ready_steps = self.ready_steps()?;
            if ready_steps.is_empty() {
                if !self.receipt_waits.is_empty() {
                    outcome
                        .waiting_receipts
                        .extend(self.receipt_waits.keys().copied());
                    return Ok(outcome);
                }
                if let Some(wait) = &self.event_wait {
                    outcome.waiting_event = Some(wait.schema.clone());
                    return Ok(outcome);
                }
                if self.all_steps_completed() {
                    self.completed = true;
                    outcome.completed = true;
                    if let Some(err) = self.enforce_invariants()? {
                        self.completed = true;
                        outcome.plan_error = Some(err);
                        return Ok(outcome);
                    }
                }
                return Ok(outcome);
            }

            let mut progressed = false;
            let mut waiting_registered = false;

            let emit_ready: Vec<String> = ready_steps
                .iter()
                .filter(|id| matches!(self.step_map[*id].kind, PlanStepKind::EmitEffect(_)))
                .cloned()
                .collect();
            match self.run_emit_ready_steps(&emit_ready, effects, &mut outcome)? {
                StepTickControl::Return => return Ok(outcome),
                StepTickControl::RestartTick => continue,
                StepTickControl::Continue => {}
            }

            for step_id in ready_steps {
                let step = self.step_map.get(&step_id).expect("step must exist").clone();
                match self.process_ready_step(step, &step_id, &mut outcome, &mut waiting_registered)? {
                    StepTickControl::Return => return Ok(outcome),
                    StepTickControl::RestartTick => {
                        progressed = true;
                        break;
                    }
                    StepTickControl::Continue => {}
                }
            }

            if progressed {
                continue;
            }

            if waiting_registered {
                return Ok(outcome);
            }

            if !self.receipt_waits.is_empty() {
                outcome
                    .waiting_receipts
                    .extend(self.receipt_waits.keys().copied());
                return Ok(outcome);
            }

            if self.all_steps_completed() {
                self.completed = true;
                outcome.completed = true;
                self.enforce_invariants()?;
            }
            return Ok(outcome);
        }
    }

    pub fn snapshot(&self) -> PlanInstanceSnapshot {
        PlanInstanceSnapshot {
            id: self.id,
            name: self.name.clone(),
            env: self.env.clone(),
            completed: self.completed,
            context: self.context.clone(),
            effect_handles: self
                .effect_handles
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            receipt_waits: self.receipt_waits.values().cloned().collect(),
            receipt_values: self
                .receipt_values
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            event_wait: self.event_wait.clone(),
            event_value: self.event_value.clone(),
            correlation_id: self.correlation_id.clone(),
            step_states: self
                .step_states
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
        }
    }

    pub fn from_snapshot(
        snapshot: PlanInstanceSnapshot,
        plan: DefPlan,
        schema_index: Arc<SchemaIndex>,
        cap_handles: Arc<HashMap<String, CapGrantResolution>>,
    ) -> Self {
        let mut instance = PlanInstance::new(
            snapshot.id,
            plan,
            snapshot.env.plan_input.clone(),
            schema_index,
            None,
            cap_handles,
        );
        instance.env = snapshot.env;
        instance.completed = snapshot.completed;
        instance.context = snapshot.context;
        instance.effect_handles = snapshot.effect_handles.into_iter().collect();
        instance.receipt_waits = snapshot
            .receipt_waits
            .into_iter()
            .map(|wait| (wait.intent_hash, wait))
            .collect();
        instance.receipt_values = snapshot.receipt_values.into_iter().collect();
        instance.event_wait = snapshot.event_wait;
        instance.event_value = snapshot.event_value;
        instance.correlation_id = snapshot.correlation_id;
        instance.step_states = snapshot.step_states.into_iter().collect();
        instance
    }

    pub fn set_context(&mut self, context: PlanContext) {
        self.context = Some(context);
    }

    pub fn context(&self) -> Option<&PlanContext> {
        self.context.as_ref()
    }

    pub fn deliver_receipt(
        &mut self,
        intent_hash: [u8; 32],
        payload: &[u8],
    ) -> Result<bool, KernelError> {
        if let Some(wait) = self.receipt_waits.remove(&intent_hash) {
            let value = decode_receipt_value(payload);
            self.receipt_values.insert(wait.step_id.clone(), value);
            self.step_states
                .insert(wait.step_id.clone(), StepState::Pending);
            return Ok(true);
        }
        Ok(false)
    }

    pub fn pending_receipt_hash(&self) -> Option<[u8; 32]> {
        self.receipt_waits.keys().next().copied()
    }

    pub fn pending_receipt_hashes(&self) -> Vec<[u8; 32]> {
        self.receipt_waits.keys().copied().collect()
    }

    pub fn override_pending_receipt_hash(&mut self, hash: [u8; 32]) {
        if self.receipt_waits.contains_key(&hash) {
            return;
        }
        if self.receipt_waits.len() == 1 {
            if let Some((_old, mut wait)) = self
                .receipt_waits
                .iter()
                .next()
                .map(|(k, v)| (*k, v.clone()))
            {
                self.receipt_waits.clear();
                wait.intent_hash = hash;
                self.receipt_waits.insert(hash, wait);
            }
        }
    }

    pub fn waiting_on_receipt(&self, hash: [u8; 32]) -> bool {
        self.receipt_waits.contains_key(&hash)
    }

    pub fn deliver_event(&mut self, event: &DomainEvent) -> Result<bool, KernelError> {
        if let Some(wait) = &self.event_wait {
            if wait.schema == event.schema {
                let normalized =
                    normalize_cbor_by_name(&self.schema_index, wait.schema.as_str(), &event.value)
                        .map_err(|err| {
                            KernelError::Manifest(format!(
                                "await_event payload decode error: {err}"
                            ))
                        })?;
                let schema = self.schema_index.get(wait.schema.as_str()).ok_or_else(|| {
                    KernelError::Manifest(format!(
                        "schema '{}' not found for await_event",
                        wait.schema
                    ))
                })?;
                let value = cbor_to_expr_value(&normalized.value, schema, &self.schema_index)?;
                if let Some(predicate) = &wait.where_clause {
                    let prev = self.env.push_event(value.clone());
                    let passes = eval_expr(predicate, &self.env).map_err(|err| {
                        KernelError::Manifest(format!("await_event where eval error: {err}"))
                    })?;
                    self.env.restore_event(prev);
                    if !value_to_bool(passes)? {
                        return Ok(false);
                    }
                }
                self.event_value = Some(value);
                self.step_states
                    .insert(wait.step_id.clone(), StepState::Pending);
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn waiting_event_schema(&self) -> Option<&str> {
        self.event_wait.as_ref().map(|w| w.schema.as_str())
    }

    fn register_receipt_wait(
        &mut self,
        step_id: String,
        handle_expr: &Expr,
    ) -> Result<[u8; 32], KernelError> {
        let handle_value = eval_expr(handle_expr, &self.env)
            .map_err(|err| KernelError::Manifest(format!("plan await eval error: {err}")))?;
        let handle = match handle_value {
            ExprValue::Text(s) => s,
            _ => {
                return Err(KernelError::Manifest(
                    "await_receipt expects handle text".into(),
                ));
            }
        };
        let intent_hash = *self
            .effect_handles
            .get(&handle)
            .ok_or_else(|| KernelError::Manifest(format!("unknown effect handle '{handle}'")))?;
        let wait = ReceiptWait {
            step_id: step_id.clone(),
            intent_hash,
        };
        self.receipt_waits.insert(intent_hash, wait);
        self.step_states.insert(step_id, StepState::WaitingReceipt);
        Ok(intent_hash)
    }

    fn record_step_value(&mut self, step_id: &str, value: ExprValue) {
        self.env.steps.insert(step_id.to_string(), value);
    }

    fn complete_step(&mut self, step_id: &str) -> Result<Option<PlanError>, KernelError> {
        self.mark_completed(step_id);
        self.enforce_invariants()
    }

    fn invariant_violation_error(&self) -> PlanError {
        PlanError {
            code: "invariant_violation".into(),
        }
    }

    fn enforce_invariants(&mut self) -> Result<Option<PlanError>, KernelError> {
        for (idx, invariant) in self.plan.invariants.iter().enumerate() {
            let value = eval_expr(invariant, &self.env).map_err(|err| {
                KernelError::Manifest(format!("plan invariant {idx} eval error: {err}"))
            })?;
            if !value_to_bool(value)? {
                return Ok(Some(self.invariant_violation_error()));
            }
        }
        Ok(None)
    }

    fn ready_steps(&mut self) -> Result<Vec<String>, KernelError> {
        loop {
            let mut ready = Vec::new();
            let mut skip = Vec::new();
            for id in &self.step_order {
                if matches!(self.step_states[id], StepState::Pending) {
                    match self.step_readiness(id)? {
                        StepReadiness::Ready => ready.push(id.clone()),
                        StepReadiness::Skip => skip.push(id.clone()),
                        StepReadiness::Blocked => {}
                    }
                }
            }

            if !ready.is_empty() {
                return Ok(ready);
            }

            if skip.is_empty() {
                return Ok(Vec::new());
            }

            for id in skip {
                self.mark_skipped(&id);
            }

            // Continue to allow skip propagation through descendants before the caller
            // decides whether the plan is waiting or completed.
        }
    }

    fn step_readiness(&self, step_id: &str) -> Result<StepReadiness, KernelError> {
        let Some(deps) = self.predecessors.get(step_id) else {
            return Ok(StepReadiness::Ready);
        };

        let mut active_dep = false;
        let mut pending_dep = false;
        for dep in deps {
            match self.step_states.get(&dep.pred) {
                Some(StepState::Completed) => {
                    let guard_true = if let Some(expr) = &dep.guard {
                        let value = eval_expr(expr, &self.env).map_err(|err| {
                            KernelError::Manifest(format!("guard eval error: {err}"))
                        })?;
                        value_to_bool(value)?
                    } else {
                        true
                    };
                    if guard_true {
                        active_dep = true;
                    }
                }
                Some(StepState::Skipped) => {}
                _ => {
                    pending_dep = true;
                }
            }
        }

        if pending_dep {
            Ok(StepReadiness::Blocked)
        } else if active_dep {
            Ok(StepReadiness::Ready)
        } else {
            Ok(StepReadiness::Skip)
        }
    }

    fn mark_completed(&mut self, step_id: &str) {
        self.step_states
            .insert(step_id.to_string(), StepState::Completed);
        self.refresh_completed_flag();
    }

    fn mark_skipped(&mut self, step_id: &str) {
        self.step_states
            .insert(step_id.to_string(), StepState::Skipped);
        self.refresh_completed_flag();
    }

    fn refresh_completed_flag(&mut self) {
        if self.all_steps_completed() {
            self.completed = true;
        }
    }

    fn all_steps_completed(&self) -> bool {
        self.step_states
            .values()
            .all(|state| matches!(state, StepState::Completed | StepState::Skipped))
    }
}

fn value_to_bool(value: ExprValue) -> Result<bool, KernelError> {
    match value {
        ExprValue::Bool(v) => Ok(v),
        other => Err(KernelError::Manifest(format!(
            "guard expression must return bool, got {:?}",
            other
        ))),
    }
}

fn eval_expr_or_value(
    expr_or_value: &ExprOrValue,
    env: &ExprEnv,
    context: &str,
) -> Result<ExprValue, KernelError> {
    match expr_or_value {
        ExprOrValue::Expr(expr) => {
            eval_expr(expr, env).map_err(|err| KernelError::Manifest(format!("{context}: {err}")))
        }
        ExprOrValue::Literal(literal) => literal_to_value(literal)
            .map_err(|err| KernelError::Manifest(format!("{context}: {err}"))),
        ExprOrValue::Json(_) => Err(KernelError::Manifest(
            "plan literals must be normalized before execution".into(),
        )),
    }
}

fn literal_to_value(literal: &ValueLiteral) -> Result<ExprValue, String> {
    match literal {
        ValueLiteral::Null(_) => Ok(ExprValue::Null),
        ValueLiteral::Bool(v) => Ok(ExprValue::Bool(v.bool)),
        ValueLiteral::Int(v) => Ok(ExprValue::Int(v.int)),
        ValueLiteral::Nat(v) => Ok(ExprValue::Nat(v.nat)),
        ValueLiteral::Dec128(v) => Ok(ExprValue::Dec128(v.dec128.clone())),
        ValueLiteral::Bytes(v) => {
            let bytes = BASE64
                .decode(v.bytes_b64.as_bytes())
                .map_err(|err| format!("invalid bytes literal: {err}"))?;
            Ok(ExprValue::Bytes(bytes))
        }
        ValueLiteral::Text(v) => Ok(ExprValue::Text(v.text.clone())),
        ValueLiteral::TimeNs(v) => Ok(ExprValue::TimeNs(v.time_ns)),
        ValueLiteral::DurationNs(v) => Ok(ExprValue::DurationNs(v.duration_ns)),
        ValueLiteral::Hash(v) => Ok(ExprValue::Hash(v.hash.clone())),
        ValueLiteral::Uuid(v) => Ok(ExprValue::Uuid(v.uuid.clone())),
        ValueLiteral::List(list) => {
            let mut out = Vec::with_capacity(list.list.len());
            for value in &list.list {
                out.push(literal_to_value(value)?);
            }
            Ok(ExprValue::List(out))
        }
        ValueLiteral::Set(set) => {
            let mut out = ExecValueSet::new();
            for item in &set.set {
                out.insert(literal_to_value_key(item)?);
            }
            Ok(ExprValue::Set(out))
        }
        ValueLiteral::Map(map) => {
            let mut out = ExecValueMap::new();
            for entry in &map.map {
                let key = literal_to_value_key(&entry.key)?;
                let value = literal_to_value(&entry.value)?;
                out.insert(key, value);
            }
            Ok(ExprValue::Map(out))
        }
        ValueLiteral::SecretRef(secret) => {
            let mut record = IndexMap::with_capacity(2);
            record.insert("alias".into(), ExprValue::Text(secret.alias.clone()));
            record.insert("version".into(), ExprValue::Nat(secret.version));
            Ok(ExprValue::Record(record))
        }
        ValueLiteral::Record(record) => {
            let mut out = IndexMap::with_capacity(record.record.len());
            for (key, value) in &record.record {
                out.insert(key.clone(), literal_to_value(value)?);
            }
            Ok(ExprValue::Record(out))
        }
        ValueLiteral::Variant(variant) => {
            let mut record = IndexMap::with_capacity(2);
            record.insert("$tag".into(), ExprValue::Text(variant.tag.clone()));
            let value = match &variant.value {
                Some(inner) => literal_to_value(inner)?,
                None => ExprValue::Unit,
            };
            record.insert("$value".into(), value);
            Ok(ExprValue::Record(record))
        }
    }
}

fn literal_to_value_key(literal: &ValueLiteral) -> Result<ValueKey, String> {
    match literal {
        ValueLiteral::Int(v) => Ok(ValueKey::Int(v.int)),
        ValueLiteral::Nat(v) => Ok(ValueKey::Nat(v.nat)),
        ValueLiteral::Text(v) => Ok(ValueKey::Text(v.text.clone())),
        ValueLiteral::Uuid(v) => Ok(ValueKey::Uuid(v.uuid.clone())),
        ValueLiteral::Hash(v) => Ok(ValueKey::Hash(v.hash.as_str().to_string())),
        other => Err(format!(
            "map/set key must be int|nat|text|uuid|hash, got {:?}",
            other
        )),
    }
}

fn expr_value_to_cbor_value(value: &ExprValue) -> CborValue {
    match value {
        ExprValue::Unit | ExprValue::Null => CborValue::Null,
        ExprValue::Bool(v) => CborValue::Bool(*v),
        ExprValue::Int(v) => CborValue::Integer(*v as i128),
        ExprValue::Nat(v) => CborValue::Integer(*v as i128),
        ExprValue::Dec128(v) => CborValue::Text(v.clone()),
        ExprValue::Bytes(bytes) => CborValue::Bytes(bytes.clone()),
        ExprValue::Text(text) => CborValue::Text(text.clone()),
        ExprValue::TimeNs(v) => CborValue::Integer(*v as i128),
        ExprValue::DurationNs(v) => CborValue::Integer(*v as i128),
        ExprValue::Hash(hash) => CborValue::Text(hash.as_str().to_string()),
        ExprValue::Uuid(uuid) => CborValue::Text(uuid.clone()),
        ExprValue::List(list) => CborValue::Array(
            list.iter()
                .map(expr_value_to_cbor_value)
                .collect::<Vec<_>>(),
        ),
        ExprValue::Set(set) => CborValue::Array(
            set.iter()
                .map(expr_value_key_to_cbor_value)
                .collect::<Vec<_>>(),
        ),
        ExprValue::Map(map) => {
            let mut out = BTreeMap::new();
            for (key, value) in map {
                out.insert(
                    expr_value_key_to_cbor_value(key),
                    expr_value_to_cbor_value(value),
                );
            }
            CborValue::Map(out)
        }
        ExprValue::Record(record) => {
            if let Some(tagged) = try_convert_variant_record(record) {
                return tagged;
            }
            let mut out = BTreeMap::new();
            for (key, value) in record {
                out.insert(
                    CborValue::Text(key.clone()),
                    expr_value_to_cbor_value(value),
                );
            }
            CborValue::Map(out)
        }
    }
}

fn idempotency_key_from_value(value: ExprValue) -> Result<[u8; 32], KernelError> {
    match value {
        ExprValue::Hash(hash) => Hash::from_hex_str(hash.as_str())
            .map(|h| *h.as_bytes())
            .map_err(|err| KernelError::IdempotencyKeyInvalid(err.to_string())),
        ExprValue::Text(text) => Hash::from_hex_str(&text)
            .map(|h| *h.as_bytes())
            .map_err(|err| KernelError::IdempotencyKeyInvalid(err.to_string())),
        ExprValue::Bytes(bytes) => Hash::from_bytes(&bytes)
            .map(|h| *h.as_bytes())
            .map_err(|err| {
                KernelError::IdempotencyKeyInvalid(format!("expected 32 bytes, got {}", err.0))
            }),
        other => Err(KernelError::IdempotencyKeyInvalid(format!(
            "expected hash or bytes, got {}",
            other.kind()
        ))),
    }
}

fn try_convert_variant_record(record: &IndexMap<String, ExprValue>) -> Option<CborValue> {
    if record.len() != 2 {
        return None;
    }
    let tag = match record.get("$tag") {
        Some(ExprValue::Text(tag)) => tag.clone(),
        _ => return None,
    };
    let value = record
        .get("$value")
        .map(expr_value_to_cbor_value)
        .unwrap_or(CborValue::Null);
    let mut out = BTreeMap::new();
    out.insert(CborValue::Text(tag), value);
    Some(CborValue::Map(out))
}

fn expr_value_key_to_cbor_value(key: &ValueKey) -> CborValue {
    match key {
        ValueKey::Int(v) => CborValue::Integer(*v as i128),
        ValueKey::Nat(v) => CborValue::Integer(*v as i128),
        ValueKey::Text(text) => CborValue::Text(text.clone()),
        ValueKey::Hash(hash) => CborValue::Text(hash.clone()),
        ValueKey::Uuid(uuid) => CborValue::Text(uuid.clone()),
    }
}

fn decode_receipt_value(payload: &[u8]) -> ExprValue {
    if let Ok(value) = serde_cbor::from_slice::<ExprValue>(payload) {
        return value;
    }
    if let Ok(cbor) = serde_cbor::from_slice::<CborValue>(payload) {
        if let Some(value) = cbor_value_to_expr_value_loose(&cbor) {
            return value;
        }
    }
    ExprValue::Bytes(payload.to_vec())
}

fn cbor_value_to_expr_value_loose(value: &CborValue) -> Option<ExprValue> {
    match value {
        CborValue::Null => Some(ExprValue::Null),
        CborValue::Bool(v) => Some(ExprValue::Bool(*v)),
        CborValue::Integer(v) => {
            if *v >= 0 {
                u64::try_from(*v).ok().map(ExprValue::Nat)
            } else {
                i64::try_from(*v).ok().map(ExprValue::Int)
            }
        }
        CborValue::Bytes(bytes) => Some(ExprValue::Bytes(bytes.clone())),
        CborValue::Text(text) => {
            if let Ok(hash) = HashRef::new(text.clone()) {
                Some(ExprValue::Hash(hash))
            } else {
                Some(ExprValue::Text(text.clone()))
            }
        }
        CborValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(cbor_value_to_expr_value_loose(item)?);
            }
            Some(ExprValue::List(out))
        }
        CborValue::Map(entries) => {
            let all_text = entries
                .iter()
                .all(|(key, _)| matches!(key, CborValue::Text(_)));
            if all_text {
                let mut record = IndexMap::new();
                for (key, value) in entries {
                    let CborValue::Text(field) = key else {
                        continue;
                    };
                    record.insert(field.clone(), cbor_value_to_expr_value_loose(value)?);
                }
                Some(ExprValue::Record(record))
            } else {
                let mut map = ExecValueMap::new();
                for (key, value) in entries {
                    let key = cbor_key_to_value_key(key)?;
                    let value = cbor_value_to_expr_value_loose(value)?;
                    map.insert(key, value);
                }
                Some(ExprValue::Map(map))
            }
        }
        _ => None,
    }
}

fn cbor_key_to_value_key(value: &CborValue) -> Option<ValueKey> {
    match value {
        CborValue::Text(text) => Some(ValueKey::Text(text.clone())),
        CborValue::Integer(v) => {
            if *v >= 0 {
                u64::try_from(*v).ok().map(ValueKey::Nat)
            } else {
                i64::try_from(*v).ok().map(ValueKey::Int)
            }
        }
        _ => None,
    }
}

fn value_key_to_literal(key: &ValueKey) -> ValueLiteral {
    match key {
        ValueKey::Int(v) => ValueLiteral::Int(ValueInt { int: *v }),
        ValueKey::Nat(v) => ValueLiteral::Nat(ValueNat { nat: *v }),
        ValueKey::Text(v) => ValueLiteral::Text(ValueText { text: v.clone() }),
        ValueKey::Hash(v) => ValueLiteral::Hash(ValueHash {
            hash: HashRef::new(v.clone()).expect("hash literal"),
        }),
        ValueKey::Uuid(v) => ValueLiteral::Uuid(ValueUuid { uuid: v.clone() }),
    }
}

fn expr_value_to_literal(value: &ExprValue) -> Result<ValueLiteral, String> {
    match value {
        ExprValue::Unit | ExprValue::Null => Ok(ValueLiteral::Null(ValueNull {
            null: EmptyObject::default(),
        })),
        ExprValue::Bool(v) => Ok(ValueLiteral::Bool(ValueBool { bool: *v })),
        ExprValue::Int(v) => Ok(ValueLiteral::Int(ValueInt { int: *v })),
        ExprValue::Nat(v) => Ok(ValueLiteral::Nat(ValueNat { nat: *v })),
        ExprValue::Dec128(v) => Ok(ValueLiteral::Dec128(ValueDec128 { dec128: v.clone() })),
        ExprValue::Bytes(bytes) => Ok(ValueLiteral::Bytes(ValueBytes {
            bytes_b64: BASE64.encode(bytes),
        })),
        ExprValue::Text(text) => Ok(ValueLiteral::Text(ValueText { text: text.clone() })),
        ExprValue::TimeNs(v) => Ok(ValueLiteral::TimeNs(ValueTimeNs { time_ns: *v })),
        ExprValue::DurationNs(v) => Ok(ValueLiteral::DurationNs(ValueDurationNs {
            duration_ns: *v,
        })),
        ExprValue::Hash(hash) => Ok(ValueLiteral::Hash(ValueHash { hash: hash.clone() })),
        ExprValue::Uuid(uuid) => Ok(ValueLiteral::Uuid(ValueUuid { uuid: uuid.clone() })),
        ExprValue::List(list) => {
            let mut out = Vec::with_capacity(list.len());
            for item in list {
                out.push(expr_value_to_literal(item)?);
            }
            Ok(ValueLiteral::List(ValueList { list: out }))
        }
        ExprValue::Set(set) => {
            let mut out = Vec::with_capacity(set.len());
            for key in set {
                out.push(value_key_to_literal(key));
            }
            Ok(ValueLiteral::Set(ValueSet { set: out }))
        }
        ExprValue::Map(map) => {
            let mut entries = Vec::with_capacity(map.len());
            for (key, val) in map {
                entries.push(ValueMapEntry {
                    key: value_key_to_literal(key),
                    value: expr_value_to_literal(val)?,
                });
            }
            Ok(ValueLiteral::Map(ValueMap { map: entries }))
        }
        ExprValue::Record(record) => {
            if record.len() == 2 && record.contains_key("$tag") && record.contains_key("$value") {
                let tag = match record.get("$tag").expect("tag present") {
                    ExprValue::Text(text) => text.clone(),
                    other => return Err(format!("variant $tag must be text, got {:?}", other)),
                };
                let value_literal = match record.get("$value").expect("value present") {
                    ExprValue::Unit => None,
                    ExprValue::Null => None,
                    other => Some(Box::new(expr_value_to_literal(other)?)),
                };
                Ok(ValueLiteral::Variant(ValueVariant {
                    tag,
                    value: value_literal,
                }))
            } else {
                let mut out = IndexMap::with_capacity(record.len());
                for (key, val) in record {
                    out.insert(key.clone(), expr_value_to_literal(val)?);
                }
                Ok(ValueLiteral::Record(ValueRecord { record: out }))
            }
        }
    }
}

