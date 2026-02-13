use aos_air_exec::Value as ExprValue;
use aos_air_types::plan_literals::{canonicalize_literal, validate_literal};
use aos_air_types::{PlanStep, PlanStepAwaitEvent, PlanStepAwaitReceipt, PlanStepAssign, PlanStepEmitEffect, PlanStepEnd, PlanStepKind, PlanStepRaiseEvent};
use aos_wasm_abi::DomainEvent;
use indexmap::IndexMap;

use crate::effects::EffectManager;
use crate::error::KernelError;

use super::{
    EventWait, PlanInstance, PlanTickOutcome, StepState, eval_expr_or_value, expr_value_to_cbor_value,
    expr_value_to_literal, idempotency_key_from_value, literal_to_value,
};

pub(super) enum StepTickControl {
    Continue,
    RestartTick,
    Return,
}

impl PlanInstance {
    pub(super) fn run_emit_ready_steps(
        &mut self,
        emit_ready: &[String],
        effects: &mut EffectManager,
        outcome: &mut PlanTickOutcome,
    ) -> Result<StepTickControl, KernelError> {
        let mut progressed = false;
        for step_id in emit_ready {
            if let Some(emit) = self.step_map.get(step_id).and_then(|step| match &step.kind {
                PlanStepKind::EmitEffect(emit) => Some(emit.clone()),
                _ => None,
            }) {
                if self.handle_emit_effect_step(step_id, &emit, effects, outcome)? {
                    return Ok(StepTickControl::Return);
                }
                progressed = true;
            }
        }

        if progressed {
            Ok(StepTickControl::RestartTick)
        } else {
            Ok(StepTickControl::Continue)
        }
    }

    pub(super) fn process_ready_step(
        &mut self,
        step: PlanStep,
        step_id: &str,
        outcome: &mut PlanTickOutcome,
        waiting_registered: &mut bool,
    ) -> Result<StepTickControl, KernelError> {
        match &step.kind {
            PlanStepKind::Assign(assign) => self.handle_assign_step(step_id, assign, outcome),
            PlanStepKind::EmitEffect(_) => Ok(StepTickControl::Continue),
            PlanStepKind::AwaitReceipt(await_step) => {
                self.handle_await_receipt_step(step_id, await_step, outcome, waiting_registered)
            }
            PlanStepKind::AwaitEvent(await_event) => {
                self.handle_await_event_step(step_id, await_event, outcome)
            }
            PlanStepKind::RaiseEvent(raise) => self.handle_raise_event_step(step_id, raise, outcome),
            PlanStepKind::End(end) => self.handle_end_step(step_id, end, outcome),
        }
    }

    fn finish_step(
        &mut self,
        step_id: &str,
        outcome: &mut PlanTickOutcome,
    ) -> Result<StepTickControl, KernelError> {
        if let Some(err) = self.complete_step(step_id)? {
            outcome.completed = true;
            self.completed = true;
            outcome.plan_error = Some(err);
            return Ok(StepTickControl::Return);
        }
        Ok(StepTickControl::RestartTick)
    }

    fn handle_assign_step(
        &mut self,
        step_id: &str,
        assign: &PlanStepAssign,
        outcome: &mut PlanTickOutcome,
    ) -> Result<StepTickControl, KernelError> {
        let value = eval_expr_or_value(&assign.expr, &self.env, "plan assign eval error")?;
        self.env.vars.insert(assign.bind.var.clone(), value.clone());
        self.record_step_value(step_id, value);
        self.finish_step(step_id, outcome)
    }

    fn handle_emit_effect_step(
        &mut self,
        step_id: &str,
        emit: &PlanStepEmitEffect,
        effects: &mut EffectManager,
        outcome: &mut PlanTickOutcome,
    ) -> Result<bool, KernelError> {
        let params_value = eval_expr_or_value(&emit.params, &self.env, "plan effect eval error")?;
        let params_cbor = aos_cbor::to_canonical_cbor(&expr_value_to_cbor_value(&params_value))
            .map_err(|err| KernelError::Manifest(err.to_string()))?;
        let idempotency_key = if let Some(key) = &emit.idempotency_key {
            let value = eval_expr_or_value(key, &self.env, "plan effect idempotency eval error")?;
            idempotency_key_from_value(value)?
        } else {
            [0u8; 32]
        };
        let grant = self.cap_handles.get(step_id).ok_or_else(|| KernelError::PlanCapabilityMissing {
            plan: self.name.clone(),
            cap: emit.cap.clone(),
        })?;
        let intent = effects.enqueue_plan_effect_with_grant(
            &self.name,
            &emit.kind,
            grant,
            params_cbor,
            idempotency_key,
        )?;
        outcome.intents_enqueued.push(intent.clone());
        let handle = emit.bind.effect_id_as.clone();
        self.effect_handles.insert(handle.clone(), intent.intent_hash);
        let handle_value = ExprValue::Text(handle.clone());
        self.env.vars.insert(handle.clone(), handle_value.clone());
        let mut record = IndexMap::new();
        record.insert("handle".into(), handle_value);
        record.insert("intent_hash".into(), ExprValue::Bytes(intent.intent_hash.to_vec()));
        record.insert("params".into(), params_value);
        self.record_step_value(step_id, ExprValue::Record(record));

        Ok(matches!(self.finish_step(step_id, outcome)?, StepTickControl::Return))
    }

    fn handle_await_receipt_step(
        &mut self,
        step_id: &str,
        await_step: &PlanStepAwaitReceipt,
        outcome: &mut PlanTickOutcome,
        waiting_registered: &mut bool,
    ) -> Result<StepTickControl, KernelError> {
        if let Some(value) = self.receipt_values.remove(step_id) {
            self.env.vars.insert(await_step.bind.var.clone(), value.clone());
            self.record_step_value(step_id, value);
            return self.finish_step(step_id, outcome);
        }

        let handle_expr = await_step.for_expr.clone();
        let intent_hash = self.register_receipt_wait(step_id.to_string(), &handle_expr)?;
        outcome.waiting_receipts.push(intent_hash);
        *waiting_registered = true;
        Ok(StepTickControl::Continue)
    }

    fn handle_await_event_step(
        &mut self,
        step_id: &str,
        await_event: &PlanStepAwaitEvent,
        outcome: &mut PlanTickOutcome,
    ) -> Result<StepTickControl, KernelError> {
        if self.correlation_id.is_some() && await_event.where_clause.is_none() {
            return Err(KernelError::Manifest(
                "await_event requires a where predicate when correlate_by is set".into(),
            ));
        }
        if let Some(value) = self.event_value.take() {
            self.env.vars.insert(await_event.bind.var.clone(), value.clone());
            self.record_step_value(step_id, value);
            self.event_wait = None;
            return self.finish_step(step_id, outcome);
        }

        self.event_wait = Some(EventWait {
            step_id: step_id.to_string(),
            schema: await_event.event.as_str().to_string(),
            where_clause: await_event.where_clause.clone(),
        });
        self.step_states
            .insert(step_id.to_string(), StepState::WaitingEvent);
        outcome.waiting_event = Some(await_event.event.as_str().to_string());
        Ok(StepTickControl::Return)
    }

    fn handle_raise_event_step(
        &mut self,
        step_id: &str,
        raise: &PlanStepRaiseEvent,
        outcome: &mut PlanTickOutcome,
    ) -> Result<StepTickControl, KernelError> {
        let schema_name = raise.event.as_str();
        let schema = self.schema_index.get(schema_name).ok_or_else(|| {
            KernelError::Manifest(format!("event schema '{}' not found for raise_event", schema_name))
        })?;
        let value = eval_expr_or_value(&raise.value, &self.env, "plan raise_event eval error")?;
        let mut event_literal = expr_value_to_literal(&value)
            .map_err(|err| KernelError::Manifest(format!("plan raise_event literal error: {err}")))?;
        canonicalize_literal(&mut event_literal, schema, &self.schema_index).map_err(|err| {
            KernelError::Manifest(format!("plan raise_event canonicalization error: {err}"))
        })?;
        validate_literal(&event_literal, schema, schema_name, &self.schema_index).map_err(|err| {
            KernelError::Manifest(format!("plan raise_event validation error: {err}"))
        })?;
        let canonical_value = literal_to_value(&event_literal)
            .map_err(|err| KernelError::Manifest(format!("plan raise_event value encode error: {err}")))?;
        let payload_cbor = expr_value_to_cbor_value(&canonical_value);
        let payload_bytes = serde_cbor::to_vec(&payload_cbor)
            .map_err(|err| KernelError::Manifest(format!("plan raise_event encode error: {err}")))?;

        let event = DomainEvent::new(schema_name.to_string(), payload_bytes);
        outcome.raised_events.push(event);

        let mut record = IndexMap::new();
        record.insert("schema".into(), ExprValue::Text(schema_name.to_string()));
        record.insert("value".into(), canonical_value);
        self.record_step_value(step_id, ExprValue::Record(record));
        self.finish_step(step_id, outcome)
    }

    fn handle_end_step(
        &mut self,
        step_id: &str,
        end: &PlanStepEnd,
        outcome: &mut PlanTickOutcome,
    ) -> Result<StepTickControl, KernelError> {
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
            let mut value = eval_expr_or_value(result_expr, &self.env, "plan end result eval error")?;

            if let Some(schema_ref) = &self.plan.output {
                let schema_name = schema_ref.as_str();
                let schema = self.schema_index.get(schema_name).ok_or_else(|| {
                    KernelError::Manifest(format!(
                        "output schema '{schema_name}' not found for plan '{}'",
                        self.plan.name
                    ))
                })?;
                let mut literal = expr_value_to_literal(&value)
                    .map_err(|err| KernelError::Manifest(format!("plan end result literal error: {err}")))?;
                canonicalize_literal(&mut literal, schema, &self.schema_index).map_err(|err| {
                    KernelError::Manifest(format!("plan end result canonicalization error: {err}"))
                })?;
                validate_literal(&literal, schema, schema_name, &self.schema_index).map_err(|err| {
                    KernelError::Manifest(format!("plan end result validation error: {err}"))
                })?;
                value = literal_to_value(&literal)
                    .map_err(|err| KernelError::Manifest(format!("plan end result decode error: {err}")))?;
                let payload_bytes = serde_cbor::to_vec(&value).map_err(|err| {
                    KernelError::Manifest(format!("plan end result encode error: {err}"))
                })?;
                outcome.result_schema = Some(schema_name.to_string());
                outcome.result_cbor = Some(payload_bytes);
            }

            self.record_step_value(step_id, value.clone());
            outcome.result = Some(value);
        } else {
            self.record_step_value(step_id, ExprValue::Unit);
        }

        self.completed = true;
        outcome.completed = true;
        if let Some(err) = self.enforce_invariants()? {
            self.completed = true;
            outcome.plan_error = Some(err);
            return Ok(StepTickControl::Return);
        }
        Ok(StepTickControl::Return)
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::capability::{CapGrantResolution, CapabilityResolver};
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
    use serde_cbor::Value as CborValue;
    use std::collections::{BTreeMap, HashMap};
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

    fn test_capability_resolver() -> CapabilityResolver {
        let cap_params =
            aos_cbor::to_canonical_cbor(&CborValue::Map(BTreeMap::new())).expect("cap params");
        let grants = vec![
            (
                CapabilityGrant {
                    name: "cap".into(),
                    cap: "sys/http.out@1".into(),
                    params_cbor: cap_params.clone(),
                    expiry_ns: None,
                },
                CapType::http_out(),
            ),
            (
                CapabilityGrant {
                    name: "cap_http".into(),
                    cap: "sys/http.out@1".into(),
                    params_cbor: cap_params,
                    expiry_ns: None,
                },
                CapType::http_out(),
            ),
        ];
        CapabilityResolver::from_runtime_grants(grants).expect("grant resolver")
    }

    fn test_effect_manager() -> EffectManager {
        let resolver = test_capability_resolver();
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
            None,
            None,
        )
    }

    fn test_cap_handles(plan: &DefPlan) -> Arc<HashMap<String, CapGrantResolution>> {
        let resolver = test_capability_resolver();
        let mut handles = HashMap::new();
        for step in &plan.steps {
            if let PlanStepKind::EmitEffect(emit) = &step.kind {
                let resolved = resolver
                    .resolve(emit.cap.as_str(), emit.kind.as_str())
                    .expect("cap handle");
                handles.insert(step.id.clone(), resolved);
            }
        }
        Arc::new(handles)
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

    fn schema_index_with_event(name: &str, schema: TypeExpr) -> Arc<SchemaIndex> {
        Arc::new(SchemaIndex::new(HashMap::from([(
            name.to_string(),
            schema,
        )])))
    }

    fn new_plan_instance(plan: DefPlan) -> PlanInstance {
        let cap_handles = test_cap_handles(&plan);
        PlanInstance::new(
            1,
            plan,
            default_env(),
            empty_schema_index(),
            None,
            cap_handles,
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
        let cap_handles = test_cap_handles(&plan);
        PlanInstance::new(1, plan, default_env(), schema_index, None, cap_handles)
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
                idempotency_key: None,
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
                idempotency_key: None,
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
                    idempotency_key: None,
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
                    idempotency_key: None,
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
                    idempotency_key: None,
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
                    idempotency_key: None,
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
        let schema_index = schema_index_with_event(
            "com.test/Evt@1",
            TypeExpr::Primitive(TypePrimitive::Int(TypePrimitiveInt {
                int: EmptyObject {},
            })),
        );
        let mut plan = plan_instance_with_schema(base_plan(steps), schema_index);
        let mut effects = test_effect_manager();
        let outcome = plan.tick(&mut effects).unwrap();
        assert_eq!(outcome.waiting_event, Some("com.test/Evt@1".into()));
        let event = DomainEvent::new("com.test/Evt@1", serde_cbor::to_vec(&5i64).unwrap());
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

    #[test]
    fn skipped_step_does_not_activate_descendants() {
        let steps = vec![
            PlanStep {
                id: "gate".into(),
                kind: PlanStepKind::Assign(PlanStepAssign {
                    expr: Expr::Const(ExprConst::Bool { bool: true }).into(),
                    bind: PlanBind { var: "gate".into() },
                }),
            },
            PlanStep {
                id: "branch".into(),
                kind: PlanStepKind::Assign(PlanStepAssign {
                    expr: Expr::Const(ExprConst::Text { text: "ok".into() }).into(),
                    bind: PlanBind {
                        var: "branch".into(),
                    },
                }),
            },
            PlanStep {
                id: "descendant".into(),
                kind: PlanStepKind::Assign(PlanStepAssign {
                    expr: Expr::Ref(ExprRef {
                        reference: "@var:branch".into(),
                    })
                    .into(),
                    bind: PlanBind {
                        var: "descendant".into(),
                    },
                }),
            },
        ];

        let mut plan = base_plan(steps);
        plan.edges.push(PlanEdge {
            from: "gate".into(),
            to: "branch".into(),
            when: Some(Expr::Const(ExprConst::Bool { bool: false })),
        });
        plan.edges.push(PlanEdge {
            from: "branch".into(),
            to: "descendant".into(),
            when: None,
        });

        let mut instance = new_plan_instance(plan);
        let mut effects = test_effect_manager();
        let outcome = instance.tick(&mut effects).unwrap();

        assert!(outcome.completed);
        assert_eq!(
            instance.step_states.get("branch"),
            Some(&StepState::Skipped)
        );
        assert_eq!(
            instance.step_states.get("descendant"),
            Some(&StepState::Skipped)
        );
        assert!(!instance.env.vars.contains_key("descendant"));
    }

    /// Raising an event should surface a DomainEvent with the serialized payload.
    #[test]
    fn raise_event_produces_domain_event() {
        let event_schema = TypeExpr::Record(TypeRecord {
            record: IndexMap::from([(
                "value".into(),
                TypeExpr::Primitive(TypePrimitive::Int(TypePrimitiveInt {
                    int: EmptyObject {},
                })),
            )]),
        });
        let steps = vec![PlanStep {
            id: "raise".into(),
            kind: PlanStepKind::RaiseEvent(aos_air_types::PlanStepRaiseEvent {
                event: aos_air_types::SchemaRef::new("com.test/Evt@1").unwrap(),
                value: Expr::Record(aos_air_types::ExprRecord {
                    record: IndexMap::from([(
                        "value".into(),
                        Expr::Const(ExprConst::Int { int: 9 }),
                    )]),
                })
                .into(),
            }),
        }];
        let mut plan = plan_instance_with_schema(
            base_plan(steps),
            schema_index_with_event("com.test/Evt@1", event_schema),
        );
        let mut effects = test_effect_manager();
        let outcome = plan.tick(&mut effects).unwrap();
        assert_eq!(outcome.raised_events.len(), 1);
        assert_eq!(outcome.raised_events[0].schema, "com.test/Evt@1");
        assert!(outcome.raised_events[0].key.is_none());
    }

    #[test]
    fn raise_event_accepts_literal_payload() {
        let event_schema = TypeExpr::Record(TypeRecord {
            record: IndexMap::from([(
                "value".into(),
                TypeExpr::Primitive(TypePrimitive::Int(TypePrimitiveInt {
                    int: EmptyObject {},
                })),
            )]),
        });
        let literal_event = ValueLiteral::Record(ValueRecord {
            record: IndexMap::from([("value".into(), ValueLiteral::Int(ValueInt { int: 3 }))]),
        });
        let steps = vec![PlanStep {
            id: "raise".into(),
            kind: PlanStepKind::RaiseEvent(PlanStepRaiseEvent {
                event: SchemaRef::new("com.test/Literal@1").unwrap(),
                value: literal_event.into(),
            }),
        }];
        let mut plan = plan_instance_with_schema(
            base_plan(steps),
            schema_index_with_event("com.test/Literal@1", event_schema),
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
        let event_schema = TypeExpr::Record(TypeRecord {
            record: IndexMap::from([(
                "correlation_id".into(),
                TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                    text: EmptyObject {},
                })),
            )]),
        });
        let schema_index = schema_index_with_event("com.test/Evt@1", event_schema);
        let mut instance = plan_instance_with_schema(plan, schema_index);
        let mut effects = test_effect_manager();
        let outcome = instance.tick(&mut effects).unwrap();
        assert_eq!(outcome.waiting_event, Some("com.test/Evt@1".into()));

        let mismatch_event = DomainEvent::new(
            "com.test/Evt@1",
            serde_cbor::to_vec(&CborValue::Map(BTreeMap::from([(
                CborValue::Text("correlation_id".into()),
                CborValue::Text("nope".into()),
            )])))
            .unwrap(),
        );
        assert!(!instance.deliver_event(&mismatch_event).unwrap());

        let match_event = DomainEvent::new(
            "com.test/Evt@1",
            serde_cbor::to_vec(&CborValue::Map(BTreeMap::from([(
                CborValue::Text("correlation_id".into()),
                CborValue::Text("match".into()),
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
        let cap_handles = test_cap_handles(&plan);
        let mut instance = PlanInstance::new(
            1,
            plan,
            default_env(),
            empty_schema_index(),
            Some((b"corr".to_vec(), correlation_value)),
            cap_handles,
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
                    idempotency_key: None,
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
        let cap_handles = test_cap_handles(&plan_def);
        let mut instance = PlanInstance::new(
            1,
            plan_def.clone(),
            default_env(),
            schema_index.clone(),
            None,
            cap_handles.clone(),
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
            PlanInstance::from_snapshot(snapshot, plan_def, schema_index, cap_handles);
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
