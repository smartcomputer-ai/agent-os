use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use aos_air_exec::{
    Env as ExprEnv, Value as ExprValue, ValueKey, ValueMap as ExecValueMap,
    ValueSet as ExecValueSet, eval_expr,
};
use aos_air_types::plan_literals::{SchemaIndex, canonicalize_literal, validate_literal};
use aos_air_types::{
    DefPlan, EmptyObject, Expr, ExprOrValue, HashRef, PlanEdge, PlanStep, PlanStepKind, TypeExpr,
    TypePrimitive, TypePrimitiveInt, ValueBool, ValueBytes, ValueDec128, ValueDurationNs,
    ValueHash, ValueInt, ValueList, ValueLiteral, ValueMap, ValueMapEntry, ValueNat, ValueNull,
    ValueRecord, ValueSet, ValueText, ValueTimeNs, ValueUuid, ValueVariant, catalog::EffectCatalog,
};
use aos_effects::EffectIntent;
use aos_wasm_abi::DomainEvent;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_cbor::{self, Value as CborValue};

use crate::effects::EffectManager;
use crate::error::KernelError;

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
}

pub struct PlanInstance {
    pub id: u64,
    pub name: String,
    pub plan: DefPlan,
    pub env: ExprEnv,
    pub completed: bool,
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
    reducer_schemas: Arc<HashMap<String, ReducerSchema>>,
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
        reducer_schemas: Arc<HashMap<String, ReducerSchema>>,
        correlation: Option<(Vec<u8>, ExprValue)>,
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
            reducer_schemas,
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
            for step_id in emit_ready {
                if let Some(PlanStep {
                    kind: PlanStepKind::EmitEffect(emit),
                    ..
                }) = self.step_map.get(&step_id)
                {
                    let params_value =
                        eval_expr_or_value(&emit.params, &self.env, "plan effect eval error")?;
                    let params_cbor =
                        aos_cbor::to_canonical_cbor(&expr_value_to_cbor_value(&params_value))
                            .map_err(|err| KernelError::Manifest(err.to_string()))?;
                    let intent = effects.enqueue_plan_effect(
                        &self.name,
                        &emit.kind,
                        &emit.cap,
                        params_cbor,
                    )?;
                    outcome.intents_enqueued.push(intent.clone());
                    let handle = emit.bind.effect_id_as.clone();
                    self.effect_handles
                        .insert(handle.clone(), intent.intent_hash);
                    let handle_value = ExprValue::Text(handle.clone());
                    self.env.vars.insert(handle.clone(), handle_value.clone());
                    let mut record = IndexMap::new();
                    record.insert("handle".into(), handle_value);
                    record.insert(
                        "intent_hash".into(),
                        ExprValue::Bytes(intent.intent_hash.to_vec()),
                    );
                    record.insert("params".into(), params_value);
                    self.record_step_value(&step_id, ExprValue::Record(record));
                    match self.complete_step(&step_id)? {
                        Some(err) => {
                            outcome.completed = true;
                            self.completed = true;
                            outcome.plan_error = Some(err);
                            return Ok(outcome);
                        }
                        None => {}
                    }
                    progressed = true;
                }
            }
            if progressed {
                continue;
            }

            for step_id in ready_steps {
                let step = self.step_map.get(&step_id).expect("step must exist");
                match &step.kind {
                    PlanStepKind::Assign(assign) => {
                        let value =
                            eval_expr_or_value(&assign.expr, &self.env, "plan assign eval error")?;
                        self.env.vars.insert(assign.bind.var.clone(), value.clone());
                        self.record_step_value(&step_id, value);
                        match self.complete_step(&step_id)? {
                            Some(err) => {
                                outcome.completed = true;
                                self.completed = true;
                                outcome.plan_error = Some(err);
                                return Ok(outcome);
                            }
                            None => {}
                        }
                        progressed = true;
                        break;
                    }
                    PlanStepKind::EmitEffect(_) => {
                        // Already handled above.
                        continue;
                    }
                    PlanStepKind::AwaitReceipt(await_step) => {
                        if let Some(value) = self.receipt_values.remove(&step_id) {
                            self.env
                                .vars
                                .insert(await_step.bind.var.clone(), value.clone());
                            self.record_step_value(&step_id, value);
                            match self.complete_step(&step_id)? {
                                Some(err) => {
                                    outcome.completed = true;
                                    self.completed = true;
                                    outcome.plan_error = Some(err);
                                    return Ok(outcome);
                                }
                                None => {}
                            }
                            progressed = true;
                            break;
                        }

                        let handle_expr = await_step.for_expr.clone();
                        let intent_hash =
                            self.register_receipt_wait(step_id.clone(), &handle_expr)?;
                        outcome.waiting_receipts.push(intent_hash);
                        waiting_registered = true;
                    }
                    PlanStepKind::AwaitEvent(await_event) => {
                        if self.correlation_id.is_some() && await_event.where_clause.is_none() {
                            return Err(KernelError::Manifest(
                                "await_event requires a where predicate when correlate_by is set"
                                    .into(),
                            ));
                        }
                        if let Some(value) = self.event_value.take() {
                            self.env
                                .vars
                                .insert(await_event.bind.var.clone(), value.clone());
                            self.record_step_value(&step_id, value);
                            self.event_wait = None;
                            match self.complete_step(&step_id)? {
                                Some(err) => {
                                    outcome.completed = true;
                                    self.completed = true;
                                    outcome.plan_error = Some(err);
                                    return Ok(outcome);
                                }
                                None => {}
                            }
                            progressed = true;
                            break;
                        }

                        self.event_wait = Some(EventWait {
                            step_id: step_id.clone(),
                            schema: await_event.event.as_str().to_string(),
                            where_clause: await_event.where_clause.clone(),
                        });
                        self.step_states
                            .insert(step_id.clone(), StepState::WaitingEvent);
                        outcome.waiting_event = Some(await_event.event.as_str().to_string());
                        return Ok(outcome);
                    }
                    PlanStepKind::RaiseEvent(raise) => {
                        let metadata =
                            self.reducer_schemas.get(&raise.reducer).ok_or_else(|| {
                                KernelError::Manifest(format!(
                                    "reducer '{}' not found for raise_event",
                                    raise.reducer
                                ))
                            })?;
                        let value = eval_expr_or_value(
                            &raise.event,
                            &self.env,
                            "plan raise_event eval error",
                        )?;
                        let mut event_literal = expr_value_to_literal(&value).map_err(|err| {
                            KernelError::Manifest(format!("plan raise_event literal error: {err}"))
                        })?;
                        canonicalize_literal(
                            &mut event_literal,
                            &metadata.event_schema,
                            &self.schema_index,
                        )
                        .map_err(|err| {
                            KernelError::Manifest(format!(
                                "plan raise_event canonicalization error: {err}"
                            ))
                        })?;
                        validate_literal(
                            &event_literal,
                            &metadata.event_schema,
                            &metadata.event_schema_name,
                            &self.schema_index,
                        )
                        .map_err(|err| {
                            KernelError::Manifest(format!(
                                "plan raise_event validation error: {err}"
                            ))
                        })?;
                        let canonical_value = literal_to_value(&event_literal).map_err(|err| {
                            KernelError::Manifest(format!(
                                "plan raise_event value encode error: {err}"
                            ))
                        })?;
                        let payload_cbor = expr_value_to_cbor_value(&canonical_value);
                        let payload_bytes = serde_cbor::to_vec(&payload_cbor).map_err(|err| {
                            KernelError::Manifest(format!("plan raise_event encode error: {err}"))
                        })?;

                        let (key_bytes, key_record_value) = match (&metadata.key_schema, &raise.key)
                        {
                            (Some(schema), Some(expr)) => {
                                let key_value = eval_expr(expr, &self.env).map_err(|err| {
                                    KernelError::Manifest(format!(
                                        "plan raise_event key eval error: {err}"
                                    ))
                                })?;
                                let mut key_literal =
                                    expr_value_to_literal(&key_value).map_err(|err| {
                                        KernelError::Manifest(format!(
                                            "plan raise_event key literal error: {err}"
                                        ))
                                    })?;
                                canonicalize_literal(&mut key_literal, schema, &self.schema_index)
                                    .map_err(|err| {
                                        KernelError::Manifest(format!(
                                            "plan raise_event key canonicalization error: {err}"
                                        ))
                                    })?;
                                validate_literal(
                                    &key_literal,
                                    schema,
                                    &metadata.event_schema_name,
                                    &self.schema_index,
                                )
                                .map_err(|err| {
                                    KernelError::Manifest(format!(
                                        "plan raise_event key validation error: {err}"
                                    ))
                                })?;
                                let canonical_key =
                                    literal_to_value(&key_literal).map_err(|err| {
                                        KernelError::Manifest(format!(
                                            "plan raise_event key value error: {err}"
                                        ))
                                    })?;
                                let canonical_key_cbor = expr_value_to_cbor_value(&canonical_key);
                                let canonical_key_bytes = serde_cbor::to_vec(&canonical_key_cbor)
                                    .map_err(|err| {
                                    KernelError::Manifest(format!(
                                        "plan raise_event key encode error: {err}"
                                    ))
                                })?;
                                (Some(canonical_key_bytes), Some(canonical_key))
                            }
                            (Some(_), None) => {
                                return Err(KernelError::Manifest(format!(
                                    "reducer '{}' requires key but raise_event omitted it",
                                    raise.reducer
                                )));
                            }
                            (None, Some(_)) => {
                                return Err(KernelError::Manifest(format!(
                                    "reducer '{}' is not keyed but raise_event provided key",
                                    raise.reducer
                                )));
                            }
                            (None, None) => (None, None),
                        };

                        let mut event =
                            DomainEvent::new(metadata.event_schema_name.clone(), payload_bytes);
                        if let Some(bytes) = key_bytes {
                            event.key = Some(bytes);
                        }
                        outcome.raised_events.push(event);

                        let mut record = IndexMap::new();
                        record.insert(
                            "schema".into(),
                            ExprValue::Text(metadata.event_schema_name.clone()),
                        );
                        record.insert("value".into(), canonical_value.clone());
                        if let Some(key_value) = key_record_value {
                            record.insert("key".into(), key_value);
                        }
                        self.record_step_value(&step_id, ExprValue::Record(record));
                        match self.complete_step(&step_id)? {
                            Some(err) => {
                                outcome.completed = true;
                                self.completed = true;
                                outcome.plan_error = Some(err);
                                return Ok(outcome);
                            }
                            None => {}
                        }
                    }
                    PlanStepKind::End(end) => {
                        match (&self.plan.output, &end.result) {
                            (Some(_), None) => {
                                return Err(KernelError::Manifest(
                                    "plan declares output schema but end result missing".into(),
                                ));
                            }
                            (None, Some(_)) => {
                                return Err(KernelError::Manifest(
                                    "plan without output schema cannot return a result".into(),
                                ));
                            }
                            _ => {}
                        }

                        if let Some(result_expr) = &end.result {
                            let mut value = eval_expr_or_value(
                                result_expr,
                                &self.env,
                                "plan end result eval error",
                            )?;

                            if let Some(schema_ref) = &self.plan.output {
                                let schema_name = schema_ref.as_str();
                                let schema =
                                    self.schema_index.get(schema_name).ok_or_else(|| {
                                        KernelError::Manifest(format!(
                                            "output schema '{schema_name}' not found for plan '{}'",
                                            self.plan.name
                                        ))
                                    })?;
                                let mut literal = expr_value_to_literal(&value).map_err(|err| {
                                    KernelError::Manifest(format!(
                                        "plan end result literal error: {err}"
                                    ))
                                })?;
                                canonicalize_literal(&mut literal, schema, &self.schema_index)
                                    .map_err(|err| {
                                        KernelError::Manifest(format!(
                                            "plan end result canonicalization error: {err}"
                                        ))
                                    })?;
                                validate_literal(&literal, schema, schema_name, &self.schema_index)
                                    .map_err(|err| {
                                        KernelError::Manifest(format!(
                                            "plan end result validation error: {err}"
                                        ))
                                    })?;
                                value = literal_to_value(&literal).map_err(|err| {
                                    KernelError::Manifest(format!(
                                        "plan end result decode error: {err}"
                                    ))
                                })?;
                                let payload_bytes = serde_cbor::to_vec(&value).map_err(|err| {
                                    KernelError::Manifest(format!(
                                        "plan end result encode error: {err}"
                                    ))
                                })?;
                                outcome.result_schema = Some(schema_name.to_string());
                                outcome.result_cbor = Some(payload_bytes);
                            }

                            self.record_step_value(&step_id, value.clone());
                            outcome.result = Some(value);
                        } else {
                            self.record_step_value(&step_id, ExprValue::Unit);
                        }

                        self.completed = true;
                        outcome.completed = true;
                        if let Some(err) = self.enforce_invariants()? {
                            self.completed = true;
                            outcome.plan_error = Some(err);
                            return Ok(outcome);
                        }
                        return Ok(outcome);
                    }
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
        reducer_schemas: Arc<HashMap<String, ReducerSchema>>,
    ) -> Self {
        let mut instance = PlanInstance::new(
            snapshot.id,
            plan,
            snapshot.env.plan_input.clone(),
            schema_index,
            reducer_schemas,
            None,
        );
        instance.env = snapshot.env;
        instance.completed = snapshot.completed;
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

    pub fn deliver_receipt(
        &mut self,
        intent_hash: [u8; 32],
        payload: &[u8],
    ) -> Result<bool, KernelError> {
        if let Some(wait) = self.receipt_waits.remove(&intent_hash) {
            let value = match serde_cbor::from_slice(payload) {
                Ok(v) => v,
                Err(_) => ExprValue::Bytes(payload.to_vec()),
            };
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
                let value = serde_cbor::from_slice(&event.value)
                    .unwrap_or(ExprValue::Bytes(event.value.clone()));
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

    fn ready_steps(&self) -> Result<Vec<String>, KernelError> {
        let mut ready = Vec::new();
        for id in &self.step_order {
            if matches!(self.step_states[id], StepState::Pending)
                && self.predecessors_satisfied(id)?
            {
                ready.push(id.clone());
            }
        }
        Ok(ready)
    }

    fn predecessors_satisfied(&self, step_id: &str) -> Result<bool, KernelError> {
        if let Some(deps) = self.predecessors.get(step_id) {
            for dep in deps {
                if !matches!(self.step_states.get(&dep.pred), Some(StepState::Completed)) {
                    return Ok(false);
                }
                if let Some(expr) = &dep.guard {
                    let value = eval_expr(expr, &self.env)
                        .map_err(|err| KernelError::Manifest(format!("guard eval error: {err}")))?;
                    if !value_to_bool(value)? {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }

    fn mark_completed(&mut self, step_id: &str) {
        self.step_states
            .insert(step_id.to_string(), StepState::Completed);
        if self
            .step_states
            .values()
            .all(|s| matches!(s, StepState::Completed))
        {
            self.completed = true;
        }
    }

    fn all_steps_completed(&self) -> bool {
        self.step_states
            .values()
            .all(|state| matches!(state, StepState::Completed))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityResolver;
    use crate::policy::AllowAllPolicy;
    use aos_air_types::plan_literals::SchemaIndex;
    use aos_air_types::{
        CapType, EffectKind, EmptyObject, Expr, ExprConst, ExprOp, ExprOpCode, ExprRecord, ExprRef,
        PlanBind, PlanBindEffect, PlanEdge, PlanStep, PlanStepAssign, PlanStepAwaitEvent,
        PlanStepAwaitReceipt, PlanStepEmitEffect, PlanStepEnd, PlanStepKind, PlanStepRaiseEvent,
        SchemaRef, TypeExpr, TypePrimitive, TypePrimitiveInt, TypePrimitiveText, TypeRecord,
        ValueInt, ValueLiteral, ValueRecord, ValueText,
    };
    use aos_effects::CapabilityGrant;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn base_plan(steps: Vec<PlanStep>) -> DefPlan {
        DefPlan {
            name: "test/plan@1".into(),
            input: aos_air_types::SchemaRef::new("test/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps,
            edges: vec![],
            required_caps: vec!["cap".into()],
            allowed_effects: vec![EffectKind::http_request()],
            invariants: vec![],
        }
    }

    fn default_env() -> ExprValue {
        ExprValue::Record(IndexMap::new())
    }

    fn test_effect_manager() -> EffectManager {
        let grants = vec![
            (
                CapabilityGrant {
                    name: "cap".into(),
                    cap: "sys/http.out@1".into(),
                    params_cbor: Vec::new(),
                    expiry_ns: None,
                    budget: None,
                },
                CapType::http_out(),
            ),
            (
                CapabilityGrant {
                    name: "cap_http".into(),
                    cap: "sys/http.out@1".into(),
                    params_cbor: Vec::new(),
                    expiry_ns: None,
                    budget: None,
                },
                CapType::http_out(),
            ),
        ];
        let resolver = CapabilityResolver::from_runtime_grants(grants);
        let effect_catalog = Arc::new(EffectCatalog::from_defs(
            aos_air_types::builtins::builtin_effects()
                .iter()
                .map(|b| b.effect.clone()),
        ));
        EffectManager::new(
            resolver,
            Box::new(AllowAllPolicy),
            effect_catalog,
            builtin_schema_index(),
            None,
            None,
        )
    }

    fn empty_schema_index() -> Arc<SchemaIndex> {
        Arc::new(SchemaIndex::new(HashMap::new()))
    }

    fn builtin_schema_index() -> Arc<SchemaIndex> {
        let mut map = HashMap::new();
        for builtin in aos_air_types::builtins::builtin_schemas() {
            map.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
        }
        Arc::new(SchemaIndex::new(map))
    }

    fn empty_reducer_schemas() -> Arc<HashMap<String, ReducerSchema>> {
        Arc::new(HashMap::new())
    }

    fn schema_index_with_output() -> Arc<SchemaIndex> {
        let mut map = HashMap::new();
        map.insert(
            "test/Out@1".into(),
            TypeExpr::Primitive(TypePrimitive::Int(TypePrimitiveInt {
                int: EmptyObject {},
            })),
        );
        Arc::new(SchemaIndex::new(map))
    }

    fn new_plan_instance(plan: DefPlan) -> PlanInstance {
        PlanInstance::new(
            1,
            plan,
            default_env(),
            empty_schema_index(),
            empty_reducer_schemas(),
            None,
        )
    }

    fn http_params_value_literal(tag: &str) -> ValueLiteral {
        ValueLiteral::Record(ValueRecord {
            record: IndexMap::from([
                (
                    "method".into(),
                    ValueLiteral::Text(ValueText { text: "GET".into() }),
                ),
                (
                    "url".into(),
                    ValueLiteral::Text(ValueText {
                        text: format!("https://example.com/{tag}"),
                    }),
                ),
                (
                    "headers".into(),
                    ValueLiteral::Map(ValueMap { map: vec![] }),
                ),
                (
                    "body_ref".into(),
                    ValueLiteral::Null(ValueNull {
                        null: EmptyObject::default(),
                    }),
                ),
            ]),
        })
    }

    fn http_params_literal(tag: &str) -> ExprOrValue {
        ExprOrValue::Literal(http_params_value_literal(tag))
    }

    fn plan_instance_with_schema(plan: DefPlan, schema_index: Arc<SchemaIndex>) -> PlanInstance {
        PlanInstance::new(
            1,
            plan,
            default_env(),
            schema_index,
            empty_reducer_schemas(),
            None,
        )
    }

    fn reducer_schema_map(
        reducer: &str,
        event_schema_name: &str,
        event_schema: TypeExpr,
        key_schema: Option<TypeExpr>,
    ) -> Arc<HashMap<String, ReducerSchema>> {
        Arc::new(HashMap::from([(
            reducer.to_string(),
            ReducerSchema {
                event_schema_name: event_schema_name.to_string(),
                event_schema,
                key_schema,
            },
        )]))
    }

    /// Assign steps should synchronously write to the plan environment.
    #[test]
    fn assign_step_updates_env() {
        let steps = vec![PlanStep {
            id: "assign".into(),
            kind: PlanStepKind::Assign(aos_air_types::PlanStepAssign {
                expr: Expr::Const(ExprConst::Int { int: 42 }).into(),
                bind: aos_air_types::PlanBind {
                    var: "answer".into(),
                },
            }),
        }];
        let mut plan = new_plan_instance(base_plan(steps));
        let mut effects = test_effect_manager();
        let outcome = plan.tick(&mut effects).unwrap();
        assert!(outcome.completed);
        assert_eq!(plan.env.vars.get("answer").unwrap(), &ExprValue::Int(42));
    }

    #[test]
    fn assign_step_accepts_literal_value() {
        let steps = vec![PlanStep {
            id: "assign_lit".into(),
            kind: PlanStepKind::Assign(PlanStepAssign {
                expr: ValueLiteral::Text(ValueText {
                    text: "literal".into(),
                })
                .into(),
                bind: PlanBind {
                    var: "greeting".into(),
                },
            }),
        }];
        let mut plan = new_plan_instance(base_plan(steps));
        let mut effects = test_effect_manager();
        let outcome = plan.tick(&mut effects).unwrap();
        assert!(outcome.completed);
        assert_eq!(
            plan.env.vars.get("greeting"),
            Some(&ExprValue::Text("literal".into()))
        );
    }

    /// `emit_effect` should enqueue an intent and record the effect handle for later awaits.
    #[test]
    fn emit_effect_enqueues_intent() {
        let steps = vec![PlanStep {
            id: "emit".into(),
            kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                kind: EffectKind::http_request(),
                params: http_params_literal("data"),
                cap: "cap".into(),
                bind: PlanBindEffect {
                    effect_id_as: "req".into(),
                },
            }),
        }];
        let mut plan = new_plan_instance(base_plan(steps));
        let mut effects = test_effect_manager();
        let outcome = plan.tick(&mut effects).unwrap();
        assert!(outcome.completed);
        assert_eq!(effects.drain().len(), 1);
        assert!(plan.effect_handles.contains_key("req"));
    }

    #[test]
    fn emit_effect_accepts_literal_params() {
        let params_literal = ValueLiteral::Record(ValueRecord {
            record: IndexMap::from([
                (
                    "url".into(),
                    ValueLiteral::Text(ValueText {
                        text: "https://example.com/literal".into(),
                    }),
                ),
                (
                    "method".into(),
                    ValueLiteral::Text(ValueText { text: "GET".into() }),
                ),
                (
                    "headers".into(),
                    ValueLiteral::Map(ValueMap { map: vec![] }),
                ),
                (
                    "body_ref".into(),
                    ValueLiteral::Null(ValueNull {
                        null: EmptyObject::default(),
                    }),
                ),
            ]),
        });
        let steps = vec![PlanStep {
            id: "emit".into(),
            kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                kind: EffectKind::http_request(),
                params: params_literal.into(),
                cap: "cap".into(),
                bind: PlanBindEffect {
                    effect_id_as: "req".into(),
                },
            }),
        }];
        let mut plan = new_plan_instance(base_plan(steps));
        let mut effects = test_effect_manager();
        let outcome = plan.tick(&mut effects).unwrap();
        assert!(outcome.completed);
        assert_eq!(effects.drain().len(), 1);
    }

    /// Plans must block on `await_receipt` until the referenced effect handle is fulfilled.
    #[test]
    fn await_receipt_waits_and_resumes() {
        let steps = vec![
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: http_params_literal("data"),
                    cap: "cap".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "await".into(),
                kind: PlanStepKind::AwaitReceipt(aos_air_types::PlanStepAwaitReceipt {
                    for_expr: Expr::Const(ExprConst::Text { text: "req".into() }),
                    bind: aos_air_types::PlanBind { var: "rcpt".into() },
                }),
            },
        ];
        let mut plan = new_plan_instance(base_plan(steps));
        let mut effects = test_effect_manager();
        let first = plan.tick(&mut effects).unwrap();
        assert_eq!(first.waiting_receipts.len(), 1);
        let hash = first.waiting_receipts[0];
        assert!(plan.deliver_receipt(hash, b"\x01").unwrap());
        let second = plan.tick(&mut effects).unwrap();
        assert!(second.completed);
        assert!(plan.env.vars.contains_key("rcpt"));
    }

    #[test]
    fn fan_out_multiple_receipts_resume_out_of_order() {
        let steps = vec![
            PlanStep {
                id: "emit_a".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: http_params_literal("alpha"),
                    cap: "cap".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "handle_a".into(),
                    },
                }),
            },
            PlanStep {
                id: "emit_b".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: http_params_literal("beta"),
                    cap: "cap".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "handle_b".into(),
                    },
                }),
            },
            PlanStep {
                id: "emit_c".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: http_params_literal("gamma"),
                    cap: "cap".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "handle_c".into(),
                    },
                }),
            },
            PlanStep {
                id: "await_a".into(),
                kind: PlanStepKind::AwaitReceipt(aos_air_types::PlanStepAwaitReceipt {
                    for_expr: Expr::Const(ExprConst::Text {
                        text: "handle_a".into(),
                    }),
                    bind: PlanBind {
                        var: "rcpt_a".into(),
                    },
                }),
            },
            PlanStep {
                id: "await_b".into(),
                kind: PlanStepKind::AwaitReceipt(aos_air_types::PlanStepAwaitReceipt {
                    for_expr: Expr::Const(ExprConst::Text {
                        text: "handle_b".into(),
                    }),
                    bind: PlanBind {
                        var: "rcpt_b".into(),
                    },
                }),
            },
            PlanStep {
                id: "await_c".into(),
                kind: PlanStepKind::AwaitReceipt(aos_air_types::PlanStepAwaitReceipt {
                    for_expr: Expr::Const(ExprConst::Text {
                        text: "handle_c".into(),
                    }),
                    bind: PlanBind {
                        var: "rcpt_c".into(),
                    },
                }),
            },
            PlanStep {
                id: "finish".into(),
                kind: PlanStepKind::End(PlanStepEnd { result: None }),
            },
        ];
        let mut plan = base_plan(steps);
        plan.edges.extend([
            PlanEdge {
                from: "emit_a".into(),
                to: "await_a".into(),
                when: None,
            },
            PlanEdge {
                from: "emit_b".into(),
                to: "await_b".into(),
                when: None,
            },
            PlanEdge {
                from: "emit_c".into(),
                to: "await_c".into(),
                when: None,
            },
            PlanEdge {
                from: "await_a".into(),
                to: "finish".into(),
                when: None,
            },
            PlanEdge {
                from: "await_b".into(),
                to: "finish".into(),
                when: None,
            },
            PlanEdge {
                from: "await_c".into(),
                to: "finish".into(),
                when: None,
            },
        ]);
        let mut plan = new_plan_instance(plan);
        let mut effects = test_effect_manager();
        let first = plan.tick(&mut effects).unwrap();
        let mut hashes = first.waiting_receipts.clone();
        hashes.sort();
        assert_eq!(hashes.len(), 3);
        assert_eq!(effects.drain().len(), 3);

        assert!(plan.deliver_receipt(hashes[1], b"\x02").unwrap());
        let mut effects = test_effect_manager();
        let out_after_first = plan.tick(&mut effects).unwrap();
        assert!(!out_after_first.completed);

        assert!(plan.deliver_receipt(hashes[0], b"\x03").unwrap());
        let mut effects = test_effect_manager();
        let out_after_second = plan.tick(&mut effects).unwrap();
        assert!(!out_after_second.completed);

        assert!(plan.deliver_receipt(hashes[2], b"\x04").unwrap());
        let mut effects = test_effect_manager();
        let final_outcome = plan.tick(&mut effects).unwrap();
        assert!(final_outcome.completed);
    }

    /// `await_event` pauses the plan until a matching schema arrives and binds it into the env.
    #[test]
    fn await_event_waits_for_schema() {
        let steps = vec![PlanStep {
            id: "await".into(),
            kind: PlanStepKind::AwaitEvent(aos_air_types::PlanStepAwaitEvent {
                event: aos_air_types::SchemaRef::new("com.test/Evt@1").unwrap(),
                where_clause: None,
                bind: aos_air_types::PlanBind { var: "evt".into() },
            }),
        }];
        let mut plan = new_plan_instance(base_plan(steps));
        let mut effects = test_effect_manager();
        let outcome = plan.tick(&mut effects).unwrap();
        assert_eq!(outcome.waiting_event, Some("com.test/Evt@1".into()));
        let event = DomainEvent::new(
            "com.test/Evt@1",
            serde_cbor::to_vec(&ExprValue::Int(5)).unwrap(),
        );
        assert!(plan.deliver_event(&event).unwrap());
        let outcome2 = plan.tick(&mut effects).unwrap();
        assert!(outcome2.completed);
        assert!(plan.env.vars.contains_key("evt"));
    }

    /// Guarded edges should prevent downstream steps from running when the guard is false.
    #[test]
    fn guard_blocks_step_until_true() {
        let mut plan = base_plan(vec![PlanStep {
            id: "end".into(),
            kind: PlanStepKind::End(PlanStepEnd { result: None }),
        }]);
        plan.edges.push(PlanEdge {
            from: "start".into(),
            to: "end".into(),
            when: Some(Expr::Const(ExprConst::Bool { bool: false })),
        });
        let mut instance = new_plan_instance(plan);
        let mut effects = test_effect_manager();
        let outcome = instance.tick(&mut effects).unwrap();
        assert!(!outcome.completed);
    }

    /// Raising an event should surface a DomainEvent with the serialized payload.
    #[test]
    fn raise_event_produces_domain_event() {
        let reducer = "com.test/Reducer@1";
        let event_schema = TypeExpr::Record(TypeRecord {
            record: IndexMap::from([(
                "value".into(),
                TypeExpr::Primitive(TypePrimitive::Int(TypePrimitiveInt {
                    int: EmptyObject {},
                })),
            )]),
        });
        let key_schema = TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
            text: EmptyObject {},
        }));
        let reducer_schemas =
            reducer_schema_map(reducer, "com.test/Evt@1", event_schema, Some(key_schema));
        let steps = vec![PlanStep {
            id: "raise".into(),
            kind: PlanStepKind::RaiseEvent(aos_air_types::PlanStepRaiseEvent {
                reducer: reducer.into(),
                key: Some(Expr::Const(ExprConst::Text {
                    text: "cell-1".into(),
                })),
                event: Expr::Record(aos_air_types::ExprRecord {
                    record: IndexMap::from([(
                        "value".into(),
                        Expr::Const(ExprConst::Int { int: 9 }),
                    )]),
                })
                .into(),
            }),
        }];
        let mut plan = PlanInstance::new(
            1,
            base_plan(steps),
            default_env(),
            empty_schema_index(),
            reducer_schemas,
            None,
        );
        let mut effects = test_effect_manager();
        let outcome = plan.tick(&mut effects).unwrap();
        assert_eq!(outcome.raised_events.len(), 1);
        assert_eq!(outcome.raised_events[0].schema, "com.test/Evt@1");
        let expected_key =
            serde_cbor::to_vec(&CborValue::Text("cell-1".into())).expect("encode key");
        assert_eq!(outcome.raised_events[0].key.as_ref(), Some(&expected_key));
    }

    #[test]
    fn raise_event_accepts_literal_payload() {
        let reducer = "com.test/Reducer@1";
        let event_schema = TypeExpr::Record(TypeRecord {
            record: IndexMap::from([(
                "value".into(),
                TypeExpr::Primitive(TypePrimitive::Int(TypePrimitiveInt {
                    int: EmptyObject {},
                })),
            )]),
        });
        let reducer_schemas = reducer_schema_map(reducer, "com.test/Literal@1", event_schema, None);
        let literal_event = ValueLiteral::Record(ValueRecord {
            record: IndexMap::from([("value".into(), ValueLiteral::Int(ValueInt { int: 3 }))]),
        });
        let steps = vec![PlanStep {
            id: "raise".into(),
            kind: PlanStepKind::RaiseEvent(PlanStepRaiseEvent {
                reducer: reducer.into(),
                event: literal_event.into(),
                key: None,
            }),
        }];
        let mut plan = PlanInstance::new(
            1,
            base_plan(steps),
            default_env(),
            empty_schema_index(),
            reducer_schemas,
            None,
        );
        let mut effects = test_effect_manager();
        let outcome = plan.tick(&mut effects).unwrap();
        assert!(outcome.completed);
        assert_eq!(outcome.raised_events.len(), 1);
        assert_eq!(outcome.raised_events[0].schema, "com.test/Literal@1");
    }

    #[test]
    fn step_values_are_available_via_step_refs() {
        let steps = vec![
            PlanStep {
                id: "first".into(),
                kind: PlanStepKind::Assign(PlanStepAssign {
                    expr: Expr::Record(ExprRecord {
                        record: IndexMap::from([(
                            "status".into(),
                            Expr::Const(ExprConst::Text { text: "ok".into() }),
                        )]),
                    })
                    .into(),
                    bind: PlanBind {
                        var: "first_state".into(),
                    },
                }),
            },
            PlanStep {
                id: "second".into(),
                kind: PlanStepKind::Assign(PlanStepAssign {
                    expr: Expr::Ref(ExprRef {
                        reference: "@step:first.status".into(),
                    })
                    .into(),
                    bind: PlanBind {
                        var: "copied".into(),
                    },
                }),
            },
        ];
        let mut plan = base_plan(steps);
        plan.edges.push(PlanEdge {
            from: "first".into(),
            to: "second".into(),
            when: None,
        });
        let mut instance = new_plan_instance(plan);
        let mut effects = test_effect_manager();
        let outcome = instance.tick(&mut effects).unwrap();
        assert!(outcome.completed);
        assert_eq!(
            instance.env.vars.get("copied"),
            Some(&ExprValue::Text("ok".into()))
        );
    }

    #[test]
    fn await_event_where_clause_filters_events() {
        let steps = vec![
            PlanStep {
                id: "await".into(),
                kind: PlanStepKind::AwaitEvent(PlanStepAwaitEvent {
                    event: SchemaRef::new("com.test/Evt@1").unwrap(),
                    where_clause: Some(Expr::Op(ExprOp {
                        op: ExprOpCode::Eq,
                        args: vec![
                            Expr::Ref(ExprRef {
                                reference: "@event.correlation_id".into(),
                            }),
                            Expr::Const(ExprConst::Text {
                                text: "match".into(),
                            }),
                        ],
                    })),
                    bind: PlanBind { var: "evt".into() },
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd { result: None }),
            },
        ];
        let mut plan = base_plan(steps);
        plan.edges.push(PlanEdge {
            from: "await".into(),
            to: "end".into(),
            when: None,
        });
        let mut instance = new_plan_instance(plan);
        let mut effects = test_effect_manager();
        let outcome = instance.tick(&mut effects).unwrap();
        assert_eq!(outcome.waiting_event, Some("com.test/Evt@1".into()));

        let mismatch_event = DomainEvent::new(
            "com.test/Evt@1",
            serde_cbor::to_vec(&ExprValue::Record(IndexMap::from([(
                "correlation_id".into(),
                ExprValue::Text("nope".into()),
            )])))
            .unwrap(),
        );
        assert!(!instance.deliver_event(&mismatch_event).unwrap());

        let match_event = DomainEvent::new(
            "com.test/Evt@1",
            serde_cbor::to_vec(&ExprValue::Record(IndexMap::from([(
                "correlation_id".into(),
                ExprValue::Text("match".into()),
            )])))
            .unwrap(),
        );
        assert!(instance.deliver_event(&match_event).unwrap());
        let outcome2 = instance.tick(&mut effects).unwrap();
        assert!(outcome2.completed);
        assert_eq!(
            instance.env.vars.get("evt"),
            Some(&ExprValue::Record(IndexMap::from([(
                "correlation_id".into(),
                ExprValue::Text("match".into()),
            )])))
        );
    }

    #[test]
    fn await_event_requires_predicate_when_correlated() {
        let steps = vec![PlanStep {
            id: "await".into(),
            kind: PlanStepKind::AwaitEvent(PlanStepAwaitEvent {
                event: SchemaRef::new("com.test/Evt@1").unwrap(),
                where_clause: None,
                bind: PlanBind { var: "evt".into() },
            }),
        }];

        let plan = base_plan(steps);
        let correlation_value = ExprValue::Text("corr".into());
        let mut instance = PlanInstance::new(
            1,
            plan,
            default_env(),
            empty_schema_index(),
            empty_reducer_schemas(),
            Some((b"corr".to_vec(), correlation_value)),
        );
        let mut effects = test_effect_manager();
        let err = instance.tick(&mut effects).unwrap_err();
        assert!(matches!(err, KernelError::Manifest(msg) if msg.contains("where predicate")));
    }

    #[test]
    fn end_step_returns_result_when_schema_declared() {
        let steps = vec![PlanStep {
            id: "end".into(),
            kind: PlanStepKind::End(PlanStepEnd {
                result: Some(Expr::Const(ExprConst::Int { int: 7 }).into()),
            }),
        }];
        let mut plan = base_plan(steps);
        plan.output = Some(SchemaRef::new("test/Out@1").unwrap());
        let mut instance = plan_instance_with_schema(plan, schema_index_with_output());
        let mut effects = test_effect_manager();
        let outcome = instance.tick(&mut effects).unwrap();
        assert!(outcome.completed);
        assert_eq!(outcome.result, Some(ExprValue::Int(7)));
        assert_eq!(outcome.result_schema, Some("test/Out@1".into()));
        assert!(outcome.result_cbor.is_some());
    }

    #[test]
    fn end_step_requires_result_when_schema_present() {
        let steps = vec![PlanStep {
            id: "end".into(),
            kind: PlanStepKind::End(PlanStepEnd { result: None }),
        }];
        let mut plan = base_plan(steps);
        plan.output = Some(SchemaRef::new("test/Out@1").unwrap());
        let mut instance = plan_instance_with_schema(plan, schema_index_with_output());
        let mut effects = test_effect_manager();
        let err = instance.tick(&mut effects).unwrap_err();
        assert!(matches!(err, KernelError::Manifest(msg) if msg.contains("output schema")));
    }

    #[test]
    fn end_step_cannot_return_without_schema() {
        let steps = vec![PlanStep {
            id: "end".into(),
            kind: PlanStepKind::End(PlanStepEnd {
                result: Some(Expr::Const(ExprConst::Nat { nat: 1 }).into()),
            }),
        }];
        let plan = base_plan(steps);
        let mut instance = new_plan_instance(plan);
        let mut effects = test_effect_manager();
        let err = instance.tick(&mut effects).unwrap_err();
        assert!(matches!(err, KernelError::Manifest(msg) if msg.contains("without output schema")));
    }

    #[test]
    fn end_step_result_must_match_output_schema_shape() {
        let steps = vec![PlanStep {
            id: "end".into(),
            kind: PlanStepKind::End(PlanStepEnd {
                result: Some(
                    Expr::Const(ExprConst::Text {
                        text: "oops".into(),
                    })
                    .into(),
                ),
            }),
        }];
        let mut plan = base_plan(steps);
        plan.output = Some(SchemaRef::new("test/Out@1").unwrap());
        let mut instance = plan_instance_with_schema(plan, schema_index_with_output());
        let mut effects = test_effect_manager();
        let err = instance.tick(&mut effects).unwrap_err();
        assert!(matches!(err, KernelError::Manifest(msg) if msg.contains("validation error")));
    }

    #[test]
    fn invariant_violation_errors_out_plan() {
        let steps = vec![
            PlanStep {
                id: "set_ok".into(),
                kind: PlanStepKind::Assign(PlanStepAssign {
                    expr: Expr::Const(ExprConst::Int { int: 5 }).into(),
                    bind: PlanBind { var: "val".into() },
                }),
            },
            PlanStep {
                id: "set_bad".into(),
                kind: PlanStepKind::Assign(PlanStepAssign {
                    expr: Expr::Const(ExprConst::Int { int: 20 }).into(),
                    bind: PlanBind { var: "val".into() },
                }),
            },
        ];
        let mut plan = base_plan(steps);
        plan.edges.push(PlanEdge {
            from: "set_ok".into(),
            to: "set_bad".into(),
            when: None,
        });
        plan.invariants.push(Expr::Op(ExprOp {
            op: ExprOpCode::Lt,
            args: vec![
                Expr::Ref(ExprRef {
                    reference: "@var:val".into(),
                }),
                Expr::Const(ExprConst::Int { int: 10 }),
            ],
        }));
        let mut instance = new_plan_instance(plan);
        let mut effects = test_effect_manager();
        let outcome = instance.tick(&mut effects).unwrap();
        assert!(outcome.completed);
        assert!(matches!(
            outcome.plan_error.as_ref().map(|e| e.code.as_str()),
            Some("invariant_violation")
        ));
    }

    #[test]
    fn snapshot_restores_waiting_receipt_state() {
        let steps = vec![
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: http_params_literal("payload"),
                    cap: "cap".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "await".into(),
                kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                    for_expr: Expr::Const(ExprConst::Text { text: "req".into() }),
                    bind: PlanBind { var: "resp".into() },
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd { result: None }),
            },
        ];
        let mut plan_def = base_plan(steps);
        plan_def.edges.extend([
            PlanEdge {
                from: "emit".into(),
                to: "await".into(),
                when: None,
            },
            PlanEdge {
                from: "await".into(),
                to: "end".into(),
                when: None,
            },
        ]);
        let schema_index = empty_schema_index();
        let reducer_schemas = empty_reducer_schemas();
        let mut instance = PlanInstance::new(
            1,
            plan_def.clone(),
            default_env(),
            schema_index.clone(),
            reducer_schemas.clone(),
            None,
        );
        let mut effects = test_effect_manager();
        let first = instance.tick(&mut effects).unwrap();
        let mut hash = first
            .waiting_receipts
            .first()
            .copied()
            .expect("waiting receipt");
        let snapshot = instance.snapshot();

        let mut restored =
            PlanInstance::from_snapshot(snapshot, plan_def, schema_index, reducer_schemas);
        hash[0] ^= 0xAA;
        restored.override_pending_receipt_hash(hash);
        assert_eq!(restored.pending_receipt_hash(), Some(hash));
        assert!(restored.deliver_receipt(hash, b"\x01").unwrap());
        let mut effects = test_effect_manager();
        let outcome = restored.tick(&mut effects).unwrap();
        assert!(outcome.completed);
        assert!(restored.receipt_waits.is_empty());
    }
}
